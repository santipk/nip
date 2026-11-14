use crate::{
    audio::AudioHandle,
    contracts::*,
    storage::{HostIdentity, Invitation, Store},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, VecDeque},
    net::SocketAddr,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use subtle::ConstantTimeEq;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    sync::{Mutex, mpsc},
};

const MAX_FRAME: usize = 65536;
trait WireMessage: Serialize + serde::de::DeserializeOwned {
    const MAX_BYTES: usize;
}
#[derive(Serialize, Deserialize)]
enum Request {
    Authenticate { token: Vec<u8>, name: String },
    Presence { calling: bool, muted: bool },
    Chat(String),
}
#[derive(Clone, Serialize, Deserialize)]
enum Response {
    Welcome(Room),
    Members(Vec<Member>),
    Chat(ChatMessage),
    Error(String),
}
impl WireMessage for Request {
    const MAX_BYTES: usize = MAX_FRAME;
}
impl WireMessage for Response {
    // Welcome carries 100 messages. JSON can encode one character as six bytes.
    const MAX_BYTES: usize = 2 * 1024 * 1024;
}
async fn write_frame<T: WireMessage>(
    stream: &mut quinn::SendStream,
    value: &T,
) -> Result<(), String> {
    let bytes = serde_json::to_vec(value).map_err(|_| "Mensaje inválido")?;
    if bytes.len() > T::MAX_BYTES {
        return Err("Mensaje demasiado grande".into());
    }
    stream
        .write_u32(bytes.len() as u32)
        .await
        .map_err(|_| "Conexión cerrada")?;
    stream
        .write_all(&bytes)
        .await
        .map_err(|_| "Conexión cerrada")?;
    Ok(())
}
async fn read_frame<T: WireMessage>(stream: &mut quinn::RecvStream) -> Result<T, String> {
    let len = stream.read_u32().await.map_err(|_| "Conexión cerrada")? as usize;
    if len == 0 || len > T::MAX_BYTES {
        return Err("Tamaño de mensaje inválido".into());
    }
    let mut bytes = vec![0; len];
    stream
        .read_exact(&mut bytes)
        .await
        .map_err(|_| "Mensaje incompleto")?;
    serde_json::from_slice(&bytes).map_err(|_| "Mensaje inválido".into())
}
struct Peer {
    member: Member,
    connection: quinn::Connection,
    responses: mpsc::Sender<Response>,
}
struct RoomState {
    name: String,
    id: String,
    next_peer: u64,
    next_chat: u64,
    peers: HashMap<u64, Peer>,
    history: VecDeque<ChatMessage>,
}
impl RoomState {
    fn members(&self) -> Vec<Member> {
        let mut peers: Vec<_> = self.peers.values().map(|p| p.member.clone()).collect();
        peers.sort_by_key(|p| p.id);
        peers
    }
    fn broadcast(&mut self, response: Response) {
        for peer in self.peers.values() {
            if peer.responses.try_send(response.clone()).is_err() {
                peer.connection.close(2u32.into(), b"slow client");
            }
        }
    }
    fn presence(&mut self) {
        self.broadcast(Response::Members(self.members()));
    }
    fn chat(&mut self, id: u64, text: String, sent: &mut VecDeque<Instant>) -> Result<(), String> {
        if text.trim().is_empty() || text.chars().count() > 2000 {
            return Err("El mensaje debe contener entre 1 y 2000 caracteres".into());
        }
        let now = Instant::now();
        while sent
            .front()
            .is_some_and(|t| now.duration_since(*t) >= Duration::from_secs(10))
        {
            sent.pop_front();
        }
        if sent.len() >= 5 {
            return Err("Esperá: máximo 5 mensajes cada 10 segundos".into());
        }
        sent.push_back(now);
        let peer = self.peers.get(&id).ok_or("Participante desconectado")?;
        let message = ChatMessage {
            id: self.next_chat,
            sender_id: id,
            sender_name: peer.member.name.clone(),
            text,
            sent_at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
        };
        self.next_chat += 1;
        self.history.push_back(message.clone());
        if self.history.len() > 100 {
            self.history.pop_front();
        }
        self.broadcast(Response::Chat(message));
        Ok(())
    }
}
struct Host {
    endpoint: quinn::Endpoint,
    identity: HostIdentity,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Host {
    fn drop(&mut self) {
        self.endpoint.close(0u32.into(), b"host stopped");
        self.task.abort();
    }
}
impl Host {
    fn start(identity: HostIdentity, address: SocketAddr) -> Result<Self, String> {
        let certificate = rustls::pki_types::CertificateDer::from(identity.certificate.clone());
        let key = rustls::pki_types::PrivatePkcs8KeyDer::from(identity.key.clone());
        let mut config = quinn::ServerConfig::with_single_cert(vec![certificate], key.into())
            .map_err(|_| "Certificado del anfitrión inválido")?;
        let transport = Arc::get_mut(&mut config.transport).unwrap();
        transport.max_concurrent_bidi_streams(1u32.into());
        transport.max_concurrent_uni_streams(0u32.into());
        transport.max_idle_timeout(Some(Duration::from_secs(30).try_into().unwrap()));
        transport.keep_alive_interval(Some(Duration::from_secs(5)));
        let endpoint = quinn::Endpoint::server(config, address)
            .map_err(|_| "No se pudo abrir UDP: verificá el puerto y el firewall")?;
        let accept = endpoint.clone();
        let token = identity.token.clone();
        let state = Arc::new(Mutex::new(RoomState {
            name: identity.name.clone(),
            id: Invitation {
                address: address.to_string(),
                certificate: identity.certificate.clone(),
                token: token.clone(),
            }
            .id(),
            next_peer: 1,
            next_chat: 1,
            peers: HashMap::new(),
            history: VecDeque::new(),
        }));
        let task = tokio::spawn(async move {
            let slots = Arc::new(tokio::sync::Semaphore::new(64));
            while let Some(incoming) = accept.accept().await {
                let Ok(slot) = slots.clone().try_acquire_owned() else {
                    incoming.refuse();
                    continue;
                };
                let state = state.clone();
                let token = token.clone();
                tokio::spawn(async move {
                    let _slot = slot;
                    if let Ok(Ok(connection)) =
                        tokio::time::timeout(Duration::from_secs(5), incoming).await
                    {
                        serve_peer(connection, state, token).await;
                    }
                });
            }
        });
        Ok(Self {
            endpoint,
            identity,
            task,
        })
    }
    fn invitation(&self, address: String) -> Result<Invitation, String> {
        let invite = Invitation {
            address,
            certificate: self.identity.certificate.clone(),
            token: self.identity.token.clone(),
        };
        invite.validate()?;
        Ok(invite)
    }
}
async fn serve_peer(connection: quinn::Connection, state: Arc<Mutex<RoomState>>, token: Vec<u8>) {
    let accepted = tokio::time::timeout(Duration::from_secs(5), connection.accept_bi()).await;
    let Ok(Ok((mut send, mut recv))) = accepted else {
        connection.close(1u32.into(), b"authentication required");
        return;
    };
    let auth = tokio::time::timeout(Duration::from_secs(5), read_frame::<Request>(&mut recv)).await;
    let Ok(Ok(Request::Authenticate {
        token: supplied,
        name,
    })) = auth
    else {
        connection.close(1u32.into(), b"authentication required");
        return;
    };
    if supplied.len() != token.len()
        || !bool::from(token.ct_eq(&supplied))
        || name.trim().is_empty()
        || name.chars().count() > 64
    {
        connection.close(1u32.into(), b"authentication failed");
        return;
    }
    let (responses, mut outgoing) = mpsc::channel(128);
    let id;
    {
        let mut room = state.lock().await;
        id = room.next_peer;
        room.next_peer += 1;
        let welcome = Room {
            id: room.id.clone(),
            name: room.name.clone(),
            self_id: id,
            members: vec![],
            history: room.history.iter().cloned().collect(),
        };
        room.peers.insert(
            id,
            Peer {
                member: Member {
                    id,
                    name,
                    calling: false,
                    muted: false,
                },
                connection: connection.clone(),
                responses: responses.clone(),
            },
        );
        let mut welcome = welcome;
        welcome.members = room.members();
        let _ = responses.try_send(Response::Welcome(welcome));
        room.presence();
    }
    let (incoming_requests, mut requests) = mpsc::channel(32);
    let read_task = tokio::spawn(async move {
        loop {
            let request = read_frame::<Request>(&mut recv).await;
            let failed = request.is_err();
            if incoming_requests.send(request).await.is_err() || failed {
                break;
            }
        }
    });
    let mut sent = VecDeque::new();
    loop {
        tokio::select! {
            result = requests.recv() => { match result {
                Some(Ok(Request::Presence { calling, muted })) => { let mut room = state.lock().await; if let Some(peer) = room.peers.get_mut(&id) { peer.member.calling = calling; peer.member.muted = muted; } room.presence(); }
                Some(Ok(Request::Chat(text))) => { if let Err(error) = state.lock().await.chat(id, text, &mut sent) { if responses.try_send(Response::Error(error)).is_err() { break; } } }
                _ => break,
            } }
            Some(response) = outgoing.recv() => { if !matches!(tokio::time::timeout(Duration::from_secs(5), write_frame(&mut send, &response)).await, Ok(Ok(()))) { break; } }
            packet = connection.read_datagram() => {
                let Ok(packet) = packet else { break; };
                if packet.is_empty() || packet.len() > 1200 { continue; }
                let room = state.lock().await;
                if !room.peers.get(&id).is_some_and(|p| p.member.calling && !p.member.muted) { continue; }
                let mut forwarded = Vec::with_capacity(packet.len()+8); forwarded.extend_from_slice(&id.to_be_bytes()); forwarded.extend_from_slice(&packet);
                for (&target, peer) in &room.peers { if target != id && peer.member.calling { let _ = peer.connection.send_datagram(bytes::Bytes::copy_from_slice(&forwarded)); } }
            }
        }
    }
    read_task.abort();
    connection.close(0u32.into(), b"session closed");
    let mut room = state.lock().await;
    room.peers.remove(&id);
    room.presence();
}
struct Client {
    endpoint: quinn::Endpoint,
    connection: quinn::Connection,
    requests: mpsc::Sender<Request>,
    task: tokio::task::JoinHandle<()>,
    activation: Option<tokio::sync::oneshot::Sender<()>>,
}
impl Drop for Client {
    fn drop(&mut self) {
        self.connection.close(0u32.into(), b"left");
        self.endpoint.close(0u32.into(), b"left");
        self.task.abort();
    }
}
enum Payload {
    Response(Response),
    Closed,
}
type Update = (usize, Payload);
impl Client {
    async fn connect(
        invitation: &Invitation,
        user: String,
        updates: mpsc::Sender<Update>,
        audio: AudioHandle,
    ) -> Result<(Self, Room), String> {
        invitation.validate()?;
        if user.trim().is_empty() || user.chars().count() > 64 {
            return Err("El nombre debe contener entre 1 y 64 caracteres".into());
        }
        let mut roots = rustls::RootCertStore::empty();
        roots
            .add(rustls::pki_types::CertificateDer::from(
                invitation.certificate.clone(),
            ))
            .map_err(|_| "Certificado de invitación inválido")?;
        let bind = if invitation.address.parse::<SocketAddr>().unwrap().is_ipv4() {
            "0.0.0.0:0"
        } else {
            "[::]:0"
        };
        let mut endpoint = quinn::Endpoint::client(bind.parse().unwrap())
            .map_err(|_| "No se pudo abrir la conexión UDP")?;
        let mut config = quinn::ClientConfig::with_root_certificates(Arc::new(roots))
            .map_err(|_| "No se pudo verificar el certificado")?;
        let mut transport = quinn::TransportConfig::default();
        transport.keep_alive_interval(Some(Duration::from_secs(5)));
        transport.max_idle_timeout(Some(Duration::from_secs(30).try_into().unwrap()));
        config.transport_config(Arc::new(transport));
        endpoint.set_default_client_config(config);
        let address: SocketAddr = invitation
            .address
            .parse()
            .map_err(|_| "Dirección inválida")?;
        let connecting = endpoint
            .connect(address, "nip.local")
            .map_err(|_| "Dirección inválida")?;
        let connection = tokio::time::timeout(Duration::from_secs(8), connecting)
            .await
            .map_err(|_| "El anfitrión no responde: verificá UDP y firewall")?
            .map_err(|_| "No se pudo verificar o conectar al anfitrión")?;
        let (mut send, mut recv) = connection
            .open_bi()
            .await
            .map_err(|_| "No se pudo abrir la sesión")?;
        write_frame(
            &mut send,
            &Request::Authenticate {
                token: invitation.token.clone(),
                name: user,
            },
        )
        .await?;
        let welcome =
            tokio::time::timeout(Duration::from_secs(5), read_frame::<Response>(&mut recv))
                .await
                .map_err(|_| "El anfitrión no autenticó la sesión")??;
        let Response::Welcome(room) = welcome else {
            return Err("Respuesta de autenticación inválida".into());
        };
        let (requests, mut outgoing) = mpsc::channel(32);
        let reader = connection.clone();
        let session = connection.stable_id();
        let (read_updates, mut read_incoming) = mpsc::channel(128);
        let read_task = tokio::spawn(async move {
            loop {
                let response = read_frame::<Response>(&mut recv).await;
                let failed = response.is_err();
                if read_updates.send(response).await.is_err() || failed {
                    break;
                }
            }
        });
        let (activation, activated) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            if activated.await.is_err() {
                read_task.abort();
                return;
            }
            loop {
                tokio::select! {
                    request = outgoing.recv() => { let Some(request) = request else { break; }; if write_frame(&mut send, &request).await.is_err() { break; } }
                    response = read_incoming.recv() => { let Some(Ok(response)) = response else { break; }; if updates.send((session, Payload::Response(response))).await.is_err() { break; } }
                    packet = reader.read_datagram() => { let Ok(packet) = packet else { break; }; if packet.len() > 8 && packet.len() <= 1208 { let id = u64::from_be_bytes(packet[..8].try_into().unwrap()); audio.incoming(id, packet[8..].to_vec()); } }
                }
            }
            read_task.abort();
            let _ = updates.send((session, Payload::Closed)).await;
        });
        Ok((
            Self {
                endpoint,
                connection,
                requests,
                task,
                activation: Some(activation),
            },
            room,
        ))
    }
    fn request(&self, request: Request) -> Result<(), String> {
        self.requests
            .try_send(request)
            .map_err(|_| "La conexión está ocupada o cerrada".into())
    }
}

struct PendingConnection {
    task: tokio::task::JoinHandle<Result<(Client, Room), String>>,
    invitation: Option<Invitation>,
    host: Option<(Host, HostInfo)>,
}
impl Drop for PendingConnection {
    fn drop(&mut self) {
        self.task.abort();
    }
}
fn connect_pending(
    invitation: Invitation,
    user: String,
    updates: mpsc::Sender<(usize, Payload)>,
    audio: AudioHandle,
) -> PendingConnection {
    let saved = invitation.clone();
    PendingConnection {
        task: tokio::spawn(async move { Client::connect(&invitation, user, updates, audio).await }),
        invitation: Some(saved),
        host: None,
    }
}

pub fn spawn(
    profile: PathBuf,
    events: std::sync::mpsc::Sender<Event>,
    audio: AudioHandle,
    mut frames: mpsc::Receiver<Vec<u8>>,
) -> mpsc::UnboundedSender<Command> {
    let (commands, mut incoming) = mpsc::unbounded_channel();
    std::thread::Builder::new().name("nip-network".into()).spawn(move || {
        let runtime = match tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build() { Ok(r) => r, Err(_) => { let _ = events.send(Event::Error("No se pudo iniciar la red".into())); return; } };
        runtime.block_on(async move {
            let mut store = match Store::open(profile) { Ok(s) => s, Err(error) => { let _ = events.send(Event::Error(error)); return; } };
            let _ = audio.command(Command::AudioSettings(store.audio()));
            let mut state = AppState { saved: store.saved(), ..Default::default() };
            let _ = events.send(Event::State(state.clone()));
            let (updates, mut received) = mpsc::channel(128);
            let mut host: Option<Host> = None; let mut client: Option<Client> = None;
            let mut pending: Option<PendingConnection> = None;
            let mut status_tick = tokio::time::interval(Duration::from_millis(100)); let mut last_presence = None;
            loop {
                tokio::select! {
                    command = incoming.recv() => {
                        let Some(command) = command else { break; };
                        if matches!(command, Command::Shutdown) { break; }
                        let result: Result<(), String> = async { match command {
                            Command::Create { name, port, user } => {
                                if name.trim().is_empty() || name.chars().count() > 64 || port == 0 { return Err("Indicá un nombre de hasta 64 caracteres y un puerto válido".into()); }
                                if host.is_some() { return Err("Detené el anfitrión actual antes de crear otro".into()); }
                                let identity = store.host(&name)?; let created = Host::start(identity, ([0,0,0,0],port).into())?;
                                let invitation = created.invitation(format!("127.0.0.1:{port}"))?;
                                let mut attempt = connect_pending(invitation, user, updates.clone(), audio.clone());
                                attempt.invitation = None;
                                attempt.host = Some((created, HostInfo {name, port, addresses: local_addresses(port)}));
                                pending = Some(attempt);
                            }
                            Command::Connect { invitation, user } => {
                                let invitation = Invitation::parse(&invitation)?;
                                pending = Some(connect_pending(invitation, user, updates.clone(), audio.clone()));
                            }
                            Command::Reconnect { id, user } => {
                                let invitation = store.invitation(&id).ok_or("Servidor guardado inexistente")?;
                                pending = Some(connect_pending(invitation, user, updates.clone(), audio.clone()));
                            }
                            Command::Invite { endpoint } => {
                                let invite = host.as_ref().ok_or("Primero creá un grupo")?.invitation(endpoint)?;
                                let _ = events.send(Event::Invitation(invite.encode()?));
                            }
                            Command::Chat(text) => { client.as_ref().ok_or("Conectate a un grupo para enviar mensajes")?.request(Request::Chat(text))?; }
                            Command::Disconnect => { pending = None; disconnect(&mut client, &mut state, &audio); }
                            Command::StopHost => { pending = None; disconnect(&mut client, &mut state, &audio); host = None; state.host = None; }
                            Command::AudioSettings(settings) => { audio.command(Command::AudioSettings(settings.clone()))?; store.save_audio(settings)?; }
                            Command::Shutdown => { disconnect(&mut client, &mut state, &audio); let _ = audio.command(Command::Shutdown); return Ok(()); }
                            command => { if matches!(command, Command::Join) && client.is_none() { return Err("Conectate a un grupo antes de entrar a la llamada".into()); } audio.command(command)?; }
                        }; Ok(()) }.await;
                        if let Err(error) = result { let _ = events.send(Event::Error(error)); }
                        let _ = events.send(Event::State(state.clone()));
                    }
                    result = async { (&mut pending.as_mut().unwrap().task).await }, if pending.is_some() => {
                        let mut attempt = pending.take().unwrap();
                        let result = result.map_err(|_| "La conexión se interrumpió".to_string()).and_then(|r| r).and_then(|(mut connected, room)| {
                            if let Some(invitation) = attempt.invitation.take() { store.remember(room.name.clone(), invitation)?; }
                            disconnect(&mut client, &mut state, &audio);
                            if let Some(activation) = connected.activation.take() { let _ = activation.send(()); }
                            client = Some(connected); state.room = Some(room); last_presence = None; state.saved = store.saved();
                            if let Some((created, info)) = attempt.host.take() { host = Some(created); state.host = Some(info); }
                            Ok(())
                        });
                        if let Err(error) = result { let _ = events.send(Event::Error(error)); }
                        let _ = events.send(Event::State(state.clone()));
                    }
                    Some((session, update)) = received.recv() => { if !client.as_ref().is_some_and(|c| c.connection.stable_id() == session) { continue; } match update {
                        Payload::Response(Response::Members(members)) => { if let Some(room) = state.room.as_mut() { for previous in &room.members { if !members.iter().any(|p| p.id == previous.id) { audio.remove_peer(previous.id); } } room.members = members.clone(); let _ = events.send(Event::Members(members)); } }
                        Payload::Response(Response::Chat(message)) => { if let Some(room) = state.room.as_mut() { room.history.push(message.clone()); if room.history.len() > 100 { room.history.remove(0); } let _ = events.send(Event::Chat(message)); } }
                        Payload::Response(Response::Error(error)) => { let _ = events.send(Event::Error(error)); }
                        Payload::Closed => { if client.is_some() { disconnect(&mut client, &mut state, &audio); let _ = events.send(Event::Disconnected("Conexión cerrada por el anfitrión".into())); let _ = events.send(Event::State(state.clone())); } }
                        _ => {}
                    } }
                    Some(packet) = frames.recv() => { if audio.outgoing_allowed() && packet.len() <= 1200 { if let Some(client) = &client { let _ = client.connection.send_datagram(packet.into()); } } }
                    _ = status_tick.tick() => { let status = audio.status(); let presence = (status.calling, status.muted || status.deafened || status.testing); if last_presence != Some(presence) { if let Some(client) = &client { if client.request(Request::Presence {calling: presence.0, muted: presence.1}).is_ok() { last_presence = Some(presence); } } } }
                }
            }
            drop(pending); drop(client); drop(host); let _ = audio.command(Command::Shutdown);
        });
    }).expect("crear hilo de red");
    commands
}
fn disconnect(client: &mut Option<Client>, state: &mut AppState, audio: &AudioHandle) {
    *client = None;
    if let Some(room) = state.room.take() {
        for member in room.members {
            audio.remove_peer(member.id);
        }
    }
    let _ = audio.command(Command::Leave);
}
fn local_addresses(port: u16) -> Vec<String> {
    let mut addresses = vec![format!("127.0.0.1:{port}")];
    if let Ok(socket) = std::net::UdpSocket::bind("0.0.0.0:0") {
        if socket.connect("192.0.2.1:9").is_ok() {
            if let Ok(address) = socket.local_addr() {
                addresses.push(format!("{}:{port}", address.ip()));
            }
        }
    }
    addresses
}

#[cfg(test)]
mod tests {
    use super::*;
    fn identity() -> HostIdentity {
        let cert = rcgen::generate_simple_self_signed(vec!["nip.local".into()]).unwrap();
        HostIdentity {
            certificate: cert.cert.der().to_vec(),
            key: cert.signing_key.serialize_der(),
            token: vec![42; 32],
            name: "Grupo de prueba".into(),
        }
    }
    async fn raw_connect(
        invite: &Invitation,
    ) -> Result<
        (
            quinn::Endpoint,
            quinn::Connection,
            quinn::SendStream,
            quinn::RecvStream,
        ),
        String,
    > {
        let mut roots = rustls::RootCertStore::empty();
        roots.add(invite.certificate.clone().into()).unwrap();
        let mut endpoint = quinn::Endpoint::client("127.0.0.1:0".parse().unwrap()).unwrap();
        endpoint.set_default_client_config(
            quinn::ClientConfig::with_root_certificates(Arc::new(roots)).unwrap(),
        );
        let connection = tokio::time::timeout(
            Duration::from_secs(3),
            endpoint
                .connect(invite.address.parse().unwrap(), "nip.local")
                .unwrap(),
        )
        .await
        .map_err(|_| "timeout")?
        .map_err(|_| "certificate")?;
        let (send, recv) = connection.open_bi().await.map_err(|_| "stream")?;
        Ok((endpoint, connection, send, recv))
    }
    async fn authenticated(
        invite: &Invitation,
        name: &str,
    ) -> (
        quinn::Endpoint,
        quinn::Connection,
        quinn::SendStream,
        quinn::RecvStream,
        Room,
    ) {
        let (endpoint, connection, mut send, mut recv) = raw_connect(invite).await.unwrap();
        write_frame(
            &mut send,
            &Request::Authenticate {
                token: invite.token.clone(),
                name: name.into(),
            },
        )
        .await
        .unwrap();
        let Response::Welcome(room) = read_frame(&mut recv).await.unwrap() else {
            panic!("welcome missing")
        };
        (endpoint, connection, send, recv, room)
    }
    async fn next_chat(recv: &mut quinn::RecvStream) -> ChatMessage {
        loop {
            if let Response::Chat(message) =
                tokio::time::timeout(Duration::from_secs(3), read_frame(recv))
                    .await
                    .unwrap()
                    .unwrap()
            {
                return message;
            }
        }
    }
    fn start() -> (Host, Invitation) {
        let host = Host::start(identity(), "127.0.0.1:0".parse().unwrap()).unwrap();
        let invite = host
            .invitation(host.endpoint.local_addr().unwrap().to_string())
            .unwrap();
        (host, invite)
    }
    #[tokio::test]
    async fn two_clients_chat_and_history_use_server_identity() {
        let (_host, invite) = start();
        let (_ea, _ca, mut a, mut ar, room) = authenticated(&invite, "Ana").await;
        let (_eb, _cb, _b, mut br, _) = authenticated(&invite, "Beto").await;
        write_frame(&mut a, &Request::Chat("Hola <b>texto</b> 🦀".into()))
            .await
            .unwrap();
        let received = next_chat(&mut br).await;
        assert_eq!(received.text, "Hola <b>texto</b> 🦀");
        assert_eq!(received.sender_id, room.self_id);
        assert_eq!(received.sender_name, "Ana");
        let echo = next_chat(&mut ar).await;
        assert_eq!(echo.id, received.id);
        let (_ec, _cc, _c, _cr, joined) = authenticated(&invite, "Caro").await;
        assert_eq!(joined.history.len(), 1);
        assert_eq!(joined.history[0].text, received.text);
    }
    #[tokio::test]
    async fn wrong_pin_or_token_cannot_authenticate() {
        let (_host, invite) = start();
        let mut wrong = invite.clone();
        wrong.certificate = identity().certificate;
        assert!(raw_connect(&wrong).await.is_err());
        let (_e, connection, mut send, mut recv) = raw_connect(&invite).await.unwrap();
        write_frame(
            &mut send,
            &Request::Authenticate {
                token: vec![0; 32],
                name: "Intruso".into(),
            },
        )
        .await
        .unwrap();
        assert!(
            tokio::time::timeout(Duration::from_secs(3), read_frame::<Response>(&mut recv))
                .await
                .unwrap()
                .is_err()
        );
        assert!(connection.close_reason().is_some());
    }
    #[tokio::test]
    async fn invalid_frames_disconnect_only_offending_peer() {
        let (_host, invite) = start();
        let (_e, c, mut send, mut recv, _room) = authenticated(&invite, "Ana").await;
        send.write_u32(65537).await.unwrap();
        while let Ok(Ok(_)) =
            tokio::time::timeout(Duration::from_secs(2), read_frame::<Response>(&mut recv)).await
        {
        }
        assert!(
            tokio::time::timeout(Duration::from_secs(2), c.closed())
                .await
                .is_ok()
        );
        let (_e, _c, _s, _r, room) = authenticated(&invite, "Otro").await;
        assert_eq!(room.members.len(), 1);
    }
    #[tokio::test]
    async fn chat_limit_and_rate_are_enforced_without_audio() {
        let (_host, invite) = start();
        let (_e, _c, mut send, mut recv, _) = authenticated(&invite, "Ana").await;
        write_frame(&mut send, &Request::Chat("x".repeat(2001)))
            .await
            .unwrap();
        let mut errors = 0;
        loop {
            if let Response::Error(_) = read_frame(&mut recv).await.unwrap() {
                errors += 1;
                break;
            }
        }
        for i in 0..6 {
            write_frame(&mut send, &Request::Chat(format!("Mensaje {i}")))
                .await
                .unwrap();
        }
        let mut accepted = 0;
        while accepted + errors < 7 {
            match read_frame(&mut recv).await.unwrap() {
                Response::Chat(_) => accepted += 1,
                Response::Error(_) => errors += 1,
                _ => {}
            }
        }
        assert_eq!(accepted, 5);
        assert_eq!(errors, 2);
    }
    #[tokio::test]
    async fn relay_requires_call_and_uses_server_assigned_sender() {
        let (_host, invite) = start();
        let (_ea, ca, mut a, mut ar, ra) = authenticated(&invite, "Ana").await;
        let (_eb, cb, mut b, mut br, _) = authenticated(&invite, "Beto").await;
        ca.send_datagram(bytes::Bytes::from_static(b"blocked"))
            .unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(100), cb.read_datagram())
                .await
                .is_err()
        );
        write_frame(
            &mut a,
            &Request::Presence {
                calling: true,
                muted: false,
            },
        )
        .await
        .unwrap();
        write_frame(
            &mut b,
            &Request::Presence {
                calling: true,
                muted: false,
            },
        )
        .await
        .unwrap();
        // Wait for authoritative presence rather than relying on sleeps.
        loop {
            if let Response::Members(m) = read_frame::<Response>(&mut br).await.unwrap() {
                if m.len() == 2 && m.iter().all(|p| p.calling) {
                    break;
                }
            }
        }
        loop {
            if let Response::Members(m) = read_frame::<Response>(&mut ar).await.unwrap() {
                if m.len() == 2 && m.iter().all(|p| p.calling) {
                    break;
                }
            }
        }
        ca.send_datagram(bytes::Bytes::from_static(b"opus-test"))
            .unwrap();
        let packet = tokio::time::timeout(Duration::from_secs(2), cb.read_datagram())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(&packet[..8], ra.self_id.to_be_bytes());
        assert_eq!(&packet[8..], b"opus-test");
    }
    #[tokio::test]
    async fn host_restarts_with_same_identity_and_empty_history() {
        let identity = identity();
        let host = Host::start(identity.clone(), "127.0.0.1:0".parse().unwrap()).unwrap();
        let address = host.endpoint.local_addr().unwrap();
        let invite = host.invitation(address.to_string()).unwrap();
        let (_e, c, mut send, mut recv, _) = authenticated(&invite, "Ana").await;
        write_frame(&mut send, &Request::Chat("Temporal".into()))
            .await
            .unwrap();
        next_chat(&mut recv).await;
        drop(host);
        c.closed().await;
        let restarted = Host::start(identity, "127.0.0.1:0".parse().unwrap()).unwrap();
        let mut invite = invite;
        invite.address = restarted.endpoint.local_addr().unwrap().to_string();
        let (_e, _c, _s, _r, room) = authenticated(&invite, "Ana").await;
        assert!(room.history.is_empty());
    }
    #[tokio::test]
    async fn newcomer_receives_only_last_hundred_session_messages() {
        let (_host, invite) = start();
        let text = |person, message| format!("{person}:{message}{}", "\u{0000}".repeat(1990));
        let mut peers = Vec::new();
        for person in 0..21 {
            let (e, c, mut send, mut recv, room) =
                authenticated(&invite, &format!("Persona{person}")).await;
            for message in 0..5 {
                write_frame(&mut send, &Request::Chat(text(person, message)))
                    .await
                    .unwrap();
            }
            loop {
                let message = next_chat(&mut recv).await;
                if message.sender_id == room.self_id && message.text == text(person, 4) {
                    break;
                }
            }
            peers.push((e, c, send, recv));
        }
        let (_e, _c, _s, _r, room) = authenticated(&invite, "Nueva").await;
        assert_eq!(room.history.len(), 100);
        assert_eq!(room.history.first().unwrap().text, text(1, 0));
        assert_eq!(room.history.last().unwrap().text, text(20, 4));
    }
    #[tokio::test]
    async fn pending_client_preserves_messages_until_adopted() {
        let (_host, invite) = start();
        let (events, _) = std::sync::mpsc::channel();
        let (audio, _frames) =
            crate::audio::AudioEngine::spawn_synthetic(events, Default::default()).unwrap();
        let (updates, mut received) = mpsc::channel(128);
        let (mut client, _) = Client::connect(&invite, "Nueva".into(), updates, audio)
            .await
            .unwrap();
        let (_e, _c, mut send, mut recv, _) = authenticated(&invite, "Ana").await;
        write_frame(&mut send, &Request::Chat("Durante bienvenida".into()))
            .await
            .unwrap();
        next_chat(&mut recv).await;
        assert!(
            tokio::time::timeout(Duration::from_millis(100), received.recv())
                .await
                .is_err()
        );
        client.activation.take().unwrap().send(()).unwrap();
        loop {
            let (_, update) = tokio::time::timeout(Duration::from_secs(2), received.recv())
                .await
                .unwrap()
                .unwrap();
            if let Payload::Response(Response::Chat(message)) = update {
                assert_eq!(message.text, "Durante bienvenida");
                break;
            }
        }
    }
}
