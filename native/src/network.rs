use crate::{
    audio::AudioHandle,
    contracts::*,
    history::History,
    storage::{HostIdentity, Invitation, LocalServer, Store},
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
mod transfers;

const MAX_FRAME: usize = 65536;
const PROTOCOL_VERSION: u32 = 2;
trait WireMessage: Serialize + serde::de::DeserializeOwned {
    const MAX_BYTES: usize;
}
#[derive(Serialize, Deserialize)]
enum Request {
    History {
        before: u64,
    },
    HistoryIn {
        channel_id: String,
        before: u64,
    },
    SelectText {
        channel_id: String,
    },
    JoinVoice {
        channel_id: String,
        request_id: u64,
    },
    LeaveVoice,
    Authenticate {
        #[serde(default)]
        version: u32,
        token: Vec<u8>,
        name: String,
    },
    Presence {
        calling: bool,
        muted: bool,
    },
    Chat(String),
    ChatIn {
        channel_id: String,
        text: String,
    },
}
#[derive(Clone, Serialize, Deserialize)]
enum Response {
    StorageUsage(crate::files::Usage),
    History {
        channel_id: String,
        messages: Vec<ChatMessage>,
        has_more: bool,
    },
    SelectedText {
        channel_id: String,
        messages: Vec<ChatMessage>,
        has_more: bool,
    },
    Channels(Vec<Channel>),
    VoiceJoined {
        channel_id: String,
        epoch: u64,
        request_id: u64,
    },
    VoiceRejected {
        request_id: u64,
        error: String,
    },
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
#[derive(Serialize, Deserialize)]
enum TransferRequest {
    Upload {
        operation_id: String,
        channel_id: String,
        name: String,
        size: u64,
        sha256: String,
        text: String,
    },
    Download {
        channel_id: String,
        attachment_id: String,
    },
}
#[derive(Serialize, Deserialize)]
enum TransferResponse {
    Ready,
    Complete(ChatMessage),
    File(Attachment),
    Error(String),
}
impl WireMessage for TransferRequest {
    const MAX_BYTES: usize = MAX_FRAME;
}
impl WireMessage for TransferResponse {
    const MAX_BYTES: usize = MAX_FRAME;
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
    sent: VecDeque<Instant>,
    voice_epoch: u64,
    member: Member,
    connection: quinn::Connection,
    responses: mpsc::Sender<Response>,
}
struct RoomState {
    files: Option<crate::files::AttachmentStore>,
    mutations: Arc<Mutex<()>>,
    channels: Vec<Channel>,
    history_store: Option<History>,
    persistent: bool,
    has_older_messages: bool,
    token: Vec<u8>,
    max_participants: usize,
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
    fn prepare_chat(
        &mut self,
        id: u64,
        text: String,
        channel_id: String,
        _sent: &mut VecDeque<Instant>,
    ) -> Result<ChatMessage, String> {
        if !self
            .channels
            .iter()
            .any(|c| c.id == channel_id && c.kind == ChannelKind::Text)
        {
            return Err("El canal de texto no existe".into());
        }
        if text.chars().count() > 2000 {
            return Err("El mensaje debe contener entre 1 y 2000 caracteres".into());
        }
        let now = Instant::now();
        let peer = self.peers.get_mut(&id).ok_or("Participante desconectado")?;
        let sent = &mut peer.sent;
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
        let message = ChatMessage {
            attachments: vec![],
            channel_id,
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
        Ok(message)
    }
    fn commit_chat(&mut self, message: ChatMessage) {
        self.history.push_back(message.clone());
        self.history
            .make_contiguous()
            .sort_by_key(|message| message.id);
        if self
            .history
            .iter()
            .filter(|m| m.channel_id == message.channel_id)
            .count()
            > 100
        {
            if let Some(index) = self
                .history
                .iter()
                .position(|m| m.channel_id == message.channel_id)
            {
                self.history.remove(index);
            }
            self.has_older_messages = self.persistent;
        }
        self.broadcast(Response::Chat(message));
    }
}
// Quinn starts its endpoint driver in the first Runtime::spawn during construction.
// Connection-idle alone does not guarantee that this driver has released UDP.
#[derive(Debug)]
struct HostRuntime(std::sync::Mutex<Option<tokio::sync::oneshot::Sender<()>>>);
impl quinn::Runtime for HostRuntime {
    fn new_timer(&self, at: Instant) -> std::pin::Pin<Box<dyn quinn::AsyncTimer>> {
        quinn::Runtime::new_timer(&quinn::TokioRuntime, at)
    }
    fn wrap_udp_socket(
        &self,
        socket: std::net::UdpSocket,
    ) -> std::io::Result<Arc<dyn quinn::AsyncUdpSocket>> {
        quinn::Runtime::wrap_udp_socket(&quinn::TokioRuntime, socket)
    }
    fn spawn(&self, future: std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>) {
        let finished = self.0.lock().unwrap().take();
        tokio::spawn(async move {
            future.await;
            if let Some(finished) = finished {
                let _ = finished.send(());
            }
        });
    }
}
struct Host {
    driver_finished: Option<tokio::sync::oneshot::Receiver<()>>,
    relay: Option<crate::relay::RelayHost>,
    relay_client: Option<crate::relay::RelayEndpoint>,
    state: Arc<Mutex<RoomState>>,
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
    async fn shutdown(mut self) {
        let finished = self.driver_finished.take();
        self.endpoint.close(0u32.into(), b"host stopped");
        let _ = (&mut self.task).await;
        self.endpoint.wait_idle().await;
        drop(self);
        if let Some(finished) = finished {
            let _ = finished.await;
        }
    }
    #[cfg(test)]
    fn start(identity: HostIdentity, address: SocketAddr) -> Result<Self, String> {
        Self::build(
            identity,
            address,
            &LocalServerSettings::default(),
            None,
            vec![],
            (1, 1),
            false,
            default_channels(),
            None,
        )
    }
    async fn start_local(
        server: LocalServer,
        path: PathBuf,
        local_only: bool,
        reserved_socket: Option<std::net::UdpSocket>,
    ) -> Result<Self, String> {
        let server_directory = path.parent().ok_or("Servidor sin carpeta")?.to_owned();
        let limits = crate::files::Limits {
            max_file_bytes: server.settings.file_limit_bytes,
            max_server_bytes: server.settings.storage_limit_bytes,
        };
        let files = tokio::task::spawn_blocking(move || {
            crate::files::AttachmentStore::open(server_directory, limits)
        })
        .await
        .map_err(|_| "Se interrumpió el almacenamiento")??;
        let relay = server.relay.clone();
        let history = Some(History::open(path).await?);
        let channels = history.as_ref().unwrap().channels().await?;
        files
            .remove_missing_channels(&channels.iter().map(|c| c.id.clone()).collect::<Vec<_>>())?;
        for message in files.pending_publications()? {
            if !channels
                .iter()
                .any(|c| c.id == message.channel_id && c.kind == ChannelKind::Text)
            {
                return Err("Hay un adjunto pendiente para un canal inexistente".into());
            }
            history
                .as_ref()
                .unwrap()
                .ensure_message(message.clone())
                .await?;
            for attachment in &message.attachments {
                files.mark_published(&attachment.id)?;
            }
        }
        let counters = if let Some(history) = &history {
            history.counters().await?
        } else {
            (1, 1)
        };
        let (messages, more) = if server.settings.persistent {
            let mut messages = Vec::new();
            let mut more = false;
            for channel in channels.iter().filter(|c| c.kind == ChannelKind::Text) {
                let (page, has_more) = history
                    .as_ref()
                    .unwrap()
                    .page_in(channel.id.clone(), u64::MAX)
                    .await?;
                messages.extend(page);
                more |= has_more;
            }
            (messages, more)
        } else {
            (vec![], false)
        };
        let mut host = Self::build(
            server.identity,
            (
                if local_only {
                    [127, 0, 0, 1]
                } else {
                    [0, 0, 0, 0]
                },
                server.settings.port,
            )
                .into(),
            &server.settings,
            history,
            messages,
            counters,
            more,
            channels,
            reserved_socket,
        )?;
        host.state.lock().await.files = Some(files);
        if let Some(configuration) = relay {
            if local_only
                && (!configuration.host.address.ip().is_loopback()
                    || !configuration.client.address.ip().is_loopback())
            {
                return Err("Modo QA local: el relay debe estar en loopback".into());
            }
            let target = SocketAddr::from((
                [127, 0, 0, 1],
                host.endpoint
                    .local_addr()
                    .map_err(|e| e.to_string())?
                    .port(),
            ));
            host.relay = Some(
                crate::relay::RelayHost::start(configuration.host, target, configuration.tunnels)
                    .await?,
            );
            host.relay_client = Some(configuration.client);
        }
        Ok(host)
    }
    fn build(
        identity: HostIdentity,
        address: SocketAddr,
        settings: &LocalServerSettings,
        history_store: Option<History>,
        messages: Vec<ChatMessage>,
        counters: (u64, u64),
        more: bool,
        channels: Vec<Channel>,
        reserved_socket: Option<std::net::UdpSocket>,
    ) -> Result<Self, String> {
        let certificate = rustls::pki_types::CertificateDer::from(identity.certificate.clone());
        let key = rustls::pki_types::PrivatePkcs8KeyDer::from(identity.key.clone());
        let mut config = quinn::ServerConfig::with_single_cert(vec![certificate], key.into())
            .map_err(|_| "Certificado del anfitrión inválido")?;
        let transport = Arc::get_mut(&mut config.transport).unwrap();
        transport.max_concurrent_bidi_streams(5u32.into());
        transport.max_concurrent_uni_streams(0u32.into());
        transport.max_idle_timeout(Some(Duration::from_secs(30).try_into().unwrap()));
        transport.keep_alive_interval(Some(Duration::from_secs(5)));
        let socket = match reserved_socket {
            Some(socket) => socket,
            None => std::net::UdpSocket::bind(address)
                .map_err(|error| udp_error(address.port(), &error))?,
        };
        let (driver_done, driver_finished) = tokio::sync::oneshot::channel();
        let endpoint = quinn::Endpoint::new(
            Default::default(),
            Some(config),
            socket,
            Arc::new(HostRuntime(std::sync::Mutex::new(Some(driver_done)))),
        )
        .map_err(|error| udp_error(address.port(), &error))?;
        let accept = endpoint.clone();
        let token = identity.token.clone();
        let state = Arc::new(Mutex::new(RoomState {
            files: None,
            mutations: Arc::new(Mutex::new(())),
            channels,
            history_store,
            persistent: settings.persistent,
            has_older_messages: more,
            token: token.clone(),
            max_participants: settings.max_participants as usize,
            name: identity.name.clone(),
            id: Invitation {
                relay: None,
                address: address.to_string(),
                certificate: identity.certificate.clone(),
                token: token.clone(),
            }
            .id(),
            next_peer: counters.1,
            next_chat: counters.0,
            peers: HashMap::new(),
            history: messages.into(),
        }));
        let host_state = state.clone();
        let task = tokio::spawn(async move {
            let slots = Arc::new(tokio::sync::Semaphore::new(64));
            let mut serving = tokio::task::JoinSet::new();
            while let Some(incoming) = accept.accept().await {
                let Ok(slot) = slots.clone().try_acquire_owned() else {
                    incoming.refuse();
                    continue;
                };
                let state = state.clone();
                serving.spawn(async move {
                    let _slot = slot;
                    if let Ok(Ok(connection)) =
                        tokio::time::timeout(Duration::from_secs(5), incoming).await
                    {
                        serve_peer(connection, state).await;
                    }
                });
                while serving.try_join_next().is_some() {}
            }
            while serving.join_next().await.is_some() {}
        });
        Ok(Self {
            driver_finished: Some(driver_finished),
            relay: None,
            relay_client: None,
            state: host_state,
            endpoint,
            identity,
            task,
        })
    }
    fn invitation(&self, address: String) -> Result<Invitation, String> {
        let invite = Invitation {
            relay: self.relay_client.clone(),
            address,
            certificate: self.identity.certificate.clone(),
            token: self.identity.token.clone(),
        };
        invite.validate()?;
        Ok(invite)
    }
    async fn rename(&mut self, name: String) {
        self.identity.name = name.clone();
        let mut room = self.state.lock().await;
        room.name = name.clone();
        let members = room.members();
        let history: Vec<_> = room
            .history
            .iter()
            .filter(|m| m.channel_id == GENERAL_TEXT_CHANNEL)
            .cloned()
            .collect();
        for (&id, peer) in &room.peers {
            let updated = Room {
                storage_usage: Default::default(),
                protocol_version: PROTOCOL_VERSION,
                channels: room.channels.clone(),
                selected_text: GENERAL_TEXT_CHANNEL.into(),
                persistent: room.persistent,
                has_older_messages: room.has_older_messages,
                id: room.id.clone(),
                name: name.clone(),
                self_id: id,
                members: members.clone(),
                history: history.clone(),
            };
            // Reuse the existing wire variant: older clients stay connected and see the name on reconnect.
            if peer.responses.try_send(Response::Welcome(updated)).is_err() {
                peer.connection.close(2u32.into(), b"slow client");
            }
        }
    }
}
async fn serve_peer(connection: quinn::Connection, state: Arc<Mutex<RoomState>>) {
    let accepted = tokio::time::timeout(Duration::from_secs(5), connection.accept_bi()).await;
    let Ok(Ok((mut send, mut recv))) = accepted else {
        connection.close(1u32.into(), b"authentication required");
        return;
    };
    let auth = tokio::time::timeout(Duration::from_secs(5), read_frame::<Request>(&mut recv)).await;
    let Ok(Ok(Request::Authenticate {
        version,
        token: supplied,
        name,
    })) = auth
    else {
        connection.close(1u32.into(), b"authentication required");
        return;
    };
    if version != PROTOCOL_VERSION {
        let _ = write_frame(
            &mut send,
            &Response::Error(
                "Versión de protocolo incompatible. Actualizá NIP en ambos equipos.".into(),
            ),
        )
        .await;
        let _ = send.finish();
        let _ = tokio::time::timeout(Duration::from_secs(1), send.stopped()).await;
        return;
    }
    if supplied.len() != 32 || name.trim().is_empty() || name.chars().count() > 64 {
        connection.close(1u32.into(), b"authentication failed");
        return;
    }
    let (responses, mut outgoing) = mpsc::channel(128);
    let id;
    {
        let mut room = state.lock().await;
        if !bool::from(room.token.ct_eq(&supplied)) {
            connection.close(1u32.into(), b"authentication failed");
            return;
        }
        if room.peers.len() >= room.max_participants {
            connection.close(1u32.into(), b"server full");
            return;
        }
        id = room.next_peer;
        room.next_peer += 1;
        let welcome = Room {
            storage_usage: room
                .files
                .as_ref()
                .and_then(|f| f.usage().ok())
                .unwrap_or_default(),
            protocol_version: PROTOCOL_VERSION,
            channels: room.channels.clone(),
            selected_text: room
                .channels
                .iter()
                .find(|c| c.kind == ChannelKind::Text)
                .unwrap()
                .id
                .clone(),
            persistent: room.persistent,
            has_older_messages: room.has_older_messages,
            id: room.id.clone(),
            name: room.name.clone(),
            self_id: id,
            members: vec![],
            history: room
                .history
                .iter()
                .filter(|m| {
                    m.channel_id
                        == room
                            .channels
                            .iter()
                            .find(|c| c.kind == ChannelKind::Text)
                            .unwrap()
                            .id
                })
                .cloned()
                .collect(),
        };
        room.peers.insert(
            id,
            Peer {
                sent: VecDeque::new(),
                voice_epoch: 0,
                member: Member {
                    voice_channel: None,
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
    let (incoming_requests, requests) = mpsc::channel(32);
    let read_task = tokio::spawn(async move {
        loop {
            let request = read_frame::<Request>(&mut recv).await;
            let failed = request.is_err();
            if incoming_requests.send(request).await.is_err() || failed {
                break;
            }
        }
    });
    let controls_state = state.clone();
    let controls_responses = responses.clone();
    let mut control_task = tokio::spawn(async move {
        serve_controls(id, controls_state, controls_responses, requests).await;
    });
    let mut controls_joined = false;
    let transfer_slots = Arc::new(tokio::sync::Semaphore::new(2));
    let mut tasks = tokio::task::JoinSet::new();
    loop {
        tokio::select! {
            stream = connection.accept_bi() => {
                let Ok((send,recv)) = stream else { break; };
                if let Ok(slot) = transfer_slots.clone().try_acquire_owned() {
                    let state = state.clone(); tasks.spawn(async move { let _slot = slot; transfers::serve(id,state,send,recv).await; });
                } else { let mut send = send; let mut recv = recv; let _ = send.reset(3u32.into()); let _ = recv.stop(3u32.into()); }
            }
            _ = tasks.join_next(), if !tasks.is_empty() => {}
            _ = &mut control_task => { controls_joined = true; break; }
            Some(response) = outgoing.recv() => { if !matches!(tokio::time::timeout(Duration::from_secs(5), write_frame(&mut send, &response)).await, Ok(Ok(()))) { break; } }
            packet = connection.read_datagram() => {
                let Ok(packet) = packet else { break; };
                if packet.len() <= 8 || packet.len() > 1208 { continue; }
                let room = state.lock().await;
                let Some(sender) = room.peers.get(&id).filter(|p| p.member.calling && !p.member.muted && p.voice_epoch == u64::from_be_bytes(packet[..8].try_into().unwrap())) else { continue; };
                for (&target, peer) in &room.peers { if target != id && peer.member.calling && peer.member.voice_channel == sender.member.voice_channel {
                    let mut forwarded = Vec::with_capacity(packet.len()+8); forwarded.extend_from_slice(&peer.voice_epoch.to_be_bytes()); forwarded.extend_from_slice(&id.to_be_bytes()); forwarded.extend_from_slice(&packet[8..]);
                    let _ = peer.connection.send_datagram(bytes::Bytes::from(forwarded));
                } }
            }
        }
    }
    read_task.abort();
    control_task.abort();
    connection.close(0u32.into(), b"session closed");
    let _ = read_task.await;
    if !controls_joined {
        let _ = control_task.await;
    }
    tasks.shutdown().await;
    let mut room = state.lock().await;
    room.peers.remove(&id);
    room.presence();
}
async fn serve_controls(
    id: u64,
    state: Arc<Mutex<RoomState>>,
    responses: mpsc::Sender<Response>,
    mut requests: mpsc::Receiver<Result<Request, String>>,
) {
    let mut sent = VecDeque::new();
    let mut history_requested: Option<Instant> = None;
    loop {
        let result = requests.recv().await;
        match result {
            Some(Ok(Request::Presence { calling: _, muted })) => {
                let mut room = state.lock().await;
                if let Some(peer) = room.peers.get_mut(&id) {
                    peer.member.muted = muted;
                }
                room.presence();
            }
            Some(Ok(Request::JoinVoice {
                channel_id,
                request_id,
            })) => {
                let mut room = state.lock().await;
                if !room
                    .channels
                    .iter()
                    .any(|c| c.id == channel_id && c.kind == ChannelKind::Voice)
                {
                    let _ = responses.try_send(Response::VoiceRejected {
                        request_id,
                        error:
                            "La sala de voz no existe. Volvé a seleccionar una sala para continuar."
                                .into(),
                    });
                    continue;
                }
                if let Some(peer) = room.peers.get_mut(&id) {
                    peer.voice_epoch = peer.voice_epoch.wrapping_add(1).max(1);
                    peer.member.voice_channel = Some(channel_id.clone());
                    peer.member.calling = true;
                    let _ = responses.try_send(Response::VoiceJoined {
                        channel_id,
                        epoch: peer.voice_epoch,
                        request_id,
                    });
                }
                room.presence();
            }
            Some(Ok(Request::LeaveVoice)) => {
                let mut room = state.lock().await;
                if let Some(peer) = room.peers.get_mut(&id) {
                    peer.member.voice_channel = None;
                    peer.member.calling = false;
                    peer.voice_epoch = peer.voice_epoch.wrapping_add(1);
                }
                room.presence();
            }
            Some(Ok(Request::Chat(text))) => {
                if let Err(error) = post_chat(&state, id, text, &mut sent).await {
                    if responses.try_send(Response::Error(error)).is_err() {
                        break;
                    }
                }
            }
            Some(Ok(Request::ChatIn { channel_id, text })) => {
                if let Err(error) = post_chat_in(&state, id, channel_id, text, &mut sent).await {
                    if responses.try_send(Response::Error(error)).is_err() {
                        break;
                    }
                }
            }
            Some(Ok(Request::SelectText { channel_id })) => {
                let response = channel_page(&state, channel_id, u64::MAX, true).await;
                if responses.try_send(response).is_err() {
                    break;
                }
            }
            Some(Ok(Request::HistoryIn { channel_id, before })) => {
                if history_requested.is_some_and(|last| last.elapsed() < Duration::from_secs(1)) {
                    let _ = responses.try_send(Response::Error(
                        "Esperá un segundo antes de cargar más historial.".into(),
                    ));
                    continue;
                }
                history_requested = Some(Instant::now());
                if responses
                    .try_send(channel_page(&state, channel_id, before, false).await)
                    .is_err()
                {
                    break;
                }
            }
            Some(Ok(Request::History { before })) => {
                if history_requested.is_some_and(|last| last.elapsed() < Duration::from_secs(1)) {
                    let _ = responses.try_send(Response::Error(
                        "Esperá un segundo antes de cargar más historial.".into(),
                    ));
                    continue;
                }
                history_requested = Some(Instant::now());
                let history = {
                    let room = state.lock().await;
                    if room.persistent {
                        room.history_store.clone()
                    } else {
                        None
                    }
                };
                let response = match history {
                    Some(history) => match history.page(before).await {
                        Ok((messages, has_more)) => Response::History {
                            channel_id: GENERAL_TEXT_CHANNEL.into(),
                            messages,
                            has_more,
                        },
                        Err(error) => Response::Error(error),
                    },
                    None => {
                        Response::Error("Este servidor no conserva historial permanente.".into())
                    }
                };
                if responses.try_send(response).is_err() {
                    break;
                }
            }
            _ => break,
        }
    }
}
async fn post_chat(
    state: &Arc<Mutex<RoomState>>,
    id: u64,
    text: String,
    sent: &mut VecDeque<Instant>,
) -> Result<(), String> {
    post_chat_in(state, id, GENERAL_TEXT_CHANNEL.into(), text, sent).await
}
async fn channel_page(
    state: &Arc<Mutex<RoomState>>,
    channel_id: String,
    before: u64,
    selected: bool,
) -> Response {
    let room = state.lock().await;
    if !room
        .channels
        .iter()
        .any(|c| c.id == channel_id && c.kind == ChannelKind::Text)
    {
        return Response::Error("El canal de texto no existe".into());
    }
    let persistent = room.persistent;
    let history = room.history_store.clone();
    let cached: Vec<_> = room
        .history
        .iter()
        .filter(|m| m.channel_id == channel_id && m.id < before)
        .cloned()
        .collect();
    drop(room);
    let result = if persistent {
        match history {
            Some(history) => history.page_in(channel_id.clone(), before).await,
            None => Err("Historial no disponible".into()),
        }
    } else {
        Ok((cached, false))
    };
    match result {
        Ok((messages, has_more)) if selected => Response::SelectedText {
            channel_id,
            messages,
            has_more,
        },
        Ok((messages, has_more)) => Response::History {
            channel_id,
            messages,
            has_more,
        },
        Err(error) => Response::Error(error),
    }
}
async fn post_chat_in(
    state: &Arc<Mutex<RoomState>>,
    id: u64,
    channel_id: String,
    text: String,
    sent: &mut VecDeque<Instant>,
) -> Result<(), String> {
    if text.trim().is_empty() {
        return Err("El mensaje debe tener texto o un adjunto".into());
    }
    let gate = state.lock().await.mutations.clone();
    let _mutation = gate.lock().await;
    let mut room = state.lock().await;
    let message = room.prepare_chat(id, text, channel_id.clone(), sent)?;
    let persistent = room.persistent;
    let history = room.history_store.clone();
    drop(room);
    if persistent {
        history
            .ok_or("El historial no está disponible; no se envió el mensaje.")?
            .append_to(channel_id, message.clone())
            .await?;
    }
    state.lock().await.commit_chat(message);
    Ok(())
}
struct Client {
    voice_request: u64,
    relay: Option<crate::relay::RelayClient>,
    remote_endpoint: String,
    voice_epoch: Arc<std::sync::atomic::AtomicU64>,
    desired_voice: Option<String>,
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
        local_only: bool,
    ) -> Result<(Self, Room), String> {
        invitation.validate()?;
        let destination = invitation
            .relay
            .as_ref()
            .map(|relay| relay.address)
            .unwrap_or(
                invitation
                    .address
                    .parse::<SocketAddr>()
                    .map_err(|_| "Dirección inválida")?,
            );
        if local_only && !destination.ip().is_loopback() {
            return Err("Modo QA local: sólo se permiten destinos loopback".into());
        }
        if user.trim().is_empty() || user.chars().count() > 64 {
            return Err("El nombre debe contener entre 1 y 64 caracteres".into());
        }
        let mut roots = rustls::RootCertStore::empty();
        roots
            .add(rustls::pki_types::CertificateDer::from(
                invitation.certificate.clone(),
            ))
            .map_err(|_| "Certificado de invitación inválido")?;
        let bind = if invitation.relay.is_some()
            || invitation.address.parse::<SocketAddr>().unwrap().is_ipv4()
        {
            if local_only {
                "127.0.0.1:0"
            } else {
                "0.0.0.0:0"
            }
        } else {
            if local_only { "[::1]:0" } else { "[::]:0" }
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
        let relay = if let Some(endpoint) = &invitation.relay {
            Some(crate::relay::RelayClient::connect(endpoint).await?)
        } else {
            None
        };
        let address: SocketAddr = if let Some(bridge) = &relay {
            bridge.address()
        } else {
            invitation
                .address
                .parse()
                .map_err(|_| "Dirección inválida")?
        };
        let connecting = endpoint
            .connect(address, "nip.local")
            .map_err(|_| "Dirección inválida")?;
        let connection = tokio::time::timeout(Duration::from_secs(8), connecting)
            .await
            .map_err(|_| format!("El anfitrión en {address} no respondió en 8 s por QUIC directo. Revisá que esté abierto y el acceso UDP por router/firewall. Con CGNAT puede no haber conexión directa; esta prueba no valida WAN."))?
            .map_err(|error| format!("No se pudo conectar o verificar al anfitrión en {address} por QUIC directo: {error}. Revisá la invitación y el acceso UDP por router/firewall."))?;
        let (mut send, mut recv) = connection
            .open_bi()
            .await
            .map_err(|_| "No se pudo abrir la sesión")?;
        write_frame(
            &mut send,
            &Request::Authenticate {
                version: PROTOCOL_VERSION,
                token: invitation.token.clone(),
                name: user,
            },
        )
        .await?;
        let welcome =
            tokio::time::timeout(Duration::from_secs(5), read_frame::<Response>(&mut recv))
                .await
                .map_err(|_| "El anfitrión no autenticó la sesión")??;
        let room = match welcome {
            Response::Welcome(room) => room,
            Response::Error(error) => return Err(error),
            _ => {
                return Err(
                    "Respuesta de autenticación inválida; actualizá NIP en ambos equipos".into(),
                );
            }
        };
        if room.protocol_version != PROTOCOL_VERSION {
            return Err(
                "Versión de protocolo incompatible. Actualizá NIP en ambos equipos.".into(),
            );
        }
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
        let voice_epoch = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let receive_epoch = voice_epoch.clone();
        let task = tokio::spawn(async move {
            if activated.await.is_err() {
                read_task.abort();
                return;
            }
            loop {
                tokio::select! {
                    request = outgoing.recv() => { let Some(request) = request else { break; }; if write_frame(&mut send, &request).await.is_err() { break; } }
                    response = read_incoming.recv() => { let Some(Ok(response)) = response else { break; }; if updates.send((session, Payload::Response(response))).await.is_err() { break; } }
                    packet = reader.read_datagram() => { let Ok(packet) = packet else { break; }; if packet.len() > 16 && packet.len() <= 1216 { let epoch = u64::from_be_bytes(packet[..8].try_into().unwrap()); if epoch != 0 && receive_epoch.load(std::sync::atomic::Ordering::SeqCst) == epoch { let id = u64::from_be_bytes(packet[8..16].try_into().unwrap()); audio.incoming(id, packet[16..].to_vec()); } } }
                }
            }
            read_task.abort();
            let _ = updates.send((session, Payload::Closed)).await;
        });
        Ok((
            Self {
                voice_request: 0,
                relay,
                remote_endpoint: invitation.relay.as_ref().map_or_else(
                    || invitation.address.clone(),
                    |relay| relay.address.to_string(),
                ),
                voice_epoch,
                desired_voice: None,
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
    user: String,
    task: tokio::task::JoinHandle<Result<(Client, Room), String>>,
    invitation: Option<Invitation>,
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
    local_only: bool,
) -> PendingConnection {
    let saved = invitation.clone();
    PendingConnection {
        user: user.clone(),
        task: tokio::spawn(async move {
            Client::connect(&invitation, user, updates, audio, local_only).await
        }),
        invitation: Some(saved),
    }
}

pub fn spawn(
    profile: PathBuf,
    events: std::sync::mpsc::Sender<Event>,
    audio: AudioHandle,
    frames: mpsc::Receiver<Vec<u8>>,
) -> mpsc::UnboundedSender<Command> {
    spawn_scoped(profile, events, audio, frames, false)
}
/// Isolated QA: every listener/client socket and invitation destination stays on loopback.
pub fn spawn_loopback(
    profile: PathBuf,
    events: std::sync::mpsc::Sender<Event>,
    audio: AudioHandle,
    frames: mpsc::Receiver<Vec<u8>>,
) -> mpsc::UnboundedSender<Command> {
    spawn_scoped(profile, events, audio, frames, true)
}
fn spawn_scoped(
    profile: PathBuf,
    events: std::sync::mpsc::Sender<Event>,
    audio: AudioHandle,
    mut frames: mpsc::Receiver<Vec<u8>>,
    local_only: bool,
) -> mpsc::UnboundedSender<Command> {
    let (commands, mut incoming) = mpsc::unbounded_channel();
    std::thread::Builder::new().name("nip-network".into()).spawn(move || {
        let runtime = match tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build() { Ok(r) => r, Err(_) => { let _ = events.send(Event::Error("No se pudo iniciar la red".into())); return; } };
        runtime.block_on(async move {
            let mut store = match Store::open(profile) { Ok(s) => s, Err(error) => { let _ = events.send(Event::Error(error)); return; } };
            let _ = audio.command(Command::AudioSettings(store.audio()));
            let mut state = AppState { preferences: store.preferences(), saved: store.saved(), display_name: store.display_name(), ..Default::default() };
            audio.configure_shortcuts([state.preferences.mute_shortcut, state.preferences.deafen_shortcut]);
            let _ = events.send(Event::State(state.clone()));
            let (updates, mut received) = mpsc::channel(128);
            let mut hosts = HashMap::<String, Host>::new(); let mut client: Option<Client> = None;
            let publication = Arc::new(std::sync::Mutex::new(None::<(quinn::Connection,Arc<std::sync::atomic::AtomicU64>)>));
            let publisher = publication.clone(); let publish_audio = audio.clone();
            let publishing_task = tokio::spawn(async move {
                while let Some(packet) = frames.recv().await {
                    if !publish_audio.outgoing_allowed() || packet.len() > 1200 { continue; }
                    let active = publisher.lock().unwrap().clone();
                    if let Some((connection,generation)) = active {
                        let epoch = generation.load(std::sync::atomic::Ordering::SeqCst);
                        if epoch != 0 {
                            let mut framed = Vec::with_capacity(packet.len()+8); framed.extend_from_slice(&epoch.to_be_bytes()); framed.extend_from_slice(&packet);
                            let _ = connection.send_datagram(framed.into());
                        }
                    }
                }
            });
            for server in store.local_servers().into_iter().filter(|s| s.settings.persistent && s.settings.auto_start) {
                if let Err(error) = start_server(&store, &mut hosts, &server.identity.id(), local_only, None).await { let _ = events.send(Event::Error(format!("No se pudo iniciar «{}»: {error}", server.identity.name))); }
            }
            refresh_hosts(&mut state, &hosts); let _ = events.send(Event::State(state.clone()));
            let mut pending: Option<PendingConnection> = None;
            let mut reconnect_target: Option<(Invitation, String)> = None;
            let mut retry_at: Option<Instant> = None;
            let mut retries = 0u32;
            let mut backup_task: Option<tokio::task::JoinHandle<()>> = None;
            let mut transfer_jobs = HashMap::<String,TransferTask>::new();
            let mut status_tick = tokio::time::interval(Duration::from_millis(100)); let mut last_presence = None;
            loop {
                tokio::select! {
                    command = incoming.recv() => {
                        let Some(command) = command else { break; };
                        if matches!(command, Command::Shutdown) { break; }
                        let cancels_reconnect = match &command {
                            Command::Connect{..}|Command::Reconnect{..}|Command::StartServer{..}|Command::Create{..}|Command::Disconnect|Command::CancelConnect => true,
                            Command::StopServer { id } | Command::DeleteServer { id } => reconnect_target.as_ref().is_some_and(|(invite, _)| invite.id() == *id),
                            Command::StopHost => state.host.as_ref().is_some_and(|h| reconnect_target.as_ref().is_some_and(|(invite, _)| invite.id() == h.id)),
                            _ => false,
                        };
                        if cancels_reconnect { retry_at = None; reconnect_target = None; retries = 0; state.diagnostics.last_error = None; }
                        let connection_command = matches!(&command, Command::Connect {..}|Command::Reconnect {..}|Command::Create {..}|Command::StartServer {..});
                        let result: Result<(), String> = async { match command {
                            Command::UploadFile { server_id, channel_id, path, operation_id, text } => {
                                if !state.room.as_ref().is_some_and(|r| r.id == server_id && r.persistent && r.channels.iter().any(|c| c.id == channel_id && c.kind == ChannelKind::Text)) { return Err("Elegí un canal de texto de un servidor persistente conectado".into()); }
                                if transfer_jobs.get(&operation_id).is_some_and(|job| !job.is_finished()) { return Err("Este envío ya está en curso".into()); }
                                let connection = client.as_ref().ok_or("Servidor desconectado")?.connection.clone();
                                let events = events.clone(); let operation = operation_id.clone(); let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
                                let completed = Arc::new(std::sync::atomic::AtomicBool::new(false)); let completion = completed.clone();
                                let task = tokio::spawn(async move {
                                    if let Err(error) = transfers::upload(connection,path,channel_id,operation.clone(),text,events.clone(),completion.clone()).await { completion.store(true,std::sync::atomic::Ordering::SeqCst); let _ = events.send(Event::Transfer(TransferStatus { operation_id: operation,name,completed:0,total:0,finished:true,error:Some(error) })); }
                                }); transfer_jobs.insert(operation_id,TransferTask { task,completed });
                            }
                            Command::DownloadFile { server_id, channel_id, attachment_id, destination, operation_id } => {
                                if !state.room.as_ref().is_some_and(|r| r.id == server_id && r.channels.iter().any(|c| c.id == channel_id && c.kind == ChannelKind::Text)) { return Err("El canal o servidor ya no están conectados".into()); }
                                if transfer_jobs.get(&operation_id).is_some_and(|job| !job.is_finished()) { return Err("Esta descarga ya está en curso".into()); }
                                let connection = client.as_ref().ok_or("Servidor desconectado")?.connection.clone();
                                let events = events.clone(); let operation = operation_id.clone(); let name = destination.file_name().unwrap_or_default().to_string_lossy().into_owned();
                                let completed = Arc::new(std::sync::atomic::AtomicBool::new(false)); let completion = completed.clone();
                                let task = tokio::spawn(async move {
                                    if let Err(error) = transfers::download(connection,channel_id,attachment_id,destination,operation.clone(),events.clone(),completion.clone()).await { completion.store(true,std::sync::atomic::Ordering::SeqCst); let _ = events.send(Event::Transfer(TransferStatus { operation_id: operation,name,completed:0,total:0,finished:true,error:Some(error) })); }
                                }); transfer_jobs.insert(operation_id,TransferTask { task,completed });
                            }
                            Command::CancelTransfer { operation_id } => {
                                if let Some(job) = transfer_jobs.remove(&operation_id) { job.abort(); }
                                let _ = events.send(Event::Transfer(TransferStatus { operation_id,name:"Archivo".into(),completed:0,total:0,finished:true,error:Some("Cancelación solicitada. Si el anfitrión ya confirmó el archivo, el mensaje permanece; reintentar este envío no lo duplica".into()) }));
                            }
                            Command::DeleteServer { .. } | Command::ConfigureServer { .. } if backup_task.as_ref().is_some_and(|task| !task.is_finished()) => { return Err("Esperá a que termine el respaldo antes de borrar o reconfigurar servidores".into()); }
                            Command::ConfigureRelay { server_id, configuration } => {
                                if hosts.contains_key(&server_id) { return Err("Apagá este servidor antes de cambiar su relay; los datos se conservan".into()); }
                                store.configure_relay(&server_id, configuration)?;
                                let _ = events.send(Event::Notice("Configuración del relay guardada. Encendé el servidor para abrir sus túneles salientes; esto no valida WAN.".into()));
                            }
                            Command::ExportBackup { destination, password } => {
                                if backup_task.as_ref().is_some_and(|task| !task.is_finished()) { return Err("Ya hay un respaldo o restauración en curso".into()); }
                                let mut ids: Vec<_> = hosts.keys().cloned().collect(); ids.sort();
                                let mut gates = Vec::new();
                                for id in ids { gates.push(hosts[&id].state.lock().await.mutations.clone()); }
                                let snapshot = store.clone(); let task_events = events.clone();
                                backup_task = Some(tokio::spawn(async move {
                                    match crate::backup::export_gated(&snapshot, destination, password, gates).await {
                                        Ok(i) => { let _ = task_events.send(Event::Notice(format!("Respaldo cifrado completado: {} servidores y {} mensajes. Verificá restaurándolo en un perfil nuevo.", i.servers, i.messages))); }
                                        Err(error) => { let _ = task_events.send(Event::Error(error)); }
                                    }
                                }));
                                let _ = events.send(Event::Notice("Preparando respaldo cifrado… La llamada sigue activa.".into()));
                            }
                            Command::RestoreBackup { archive, profile, password } => {
                                if backup_task.as_ref().is_some_and(|task| !task.is_finished()) { return Err("Ya hay un respaldo o restauración en curso".into()); }
                                let task_events = events.clone();
                                backup_task = Some(tokio::spawn(async move {
                                    match crate::backup::restore(archive, profile.clone(), password).await {
                                        Ok(i) => { let _ = task_events.send(Event::Notice(format!("Restaurados {} servidores y {} mensajes en {}. Perfil offline; abrilo con --data-dir y revisalo antes de encender servidores.", i.servers, i.messages, profile.display()))); }
                                        Err(error) => { let _ = task_events.send(Event::Error(error)); }
                                    }
                                }));
                            }
                            Command::CreateChannel { server_id, name, kind } => {
                                if !store.owns(&server_id) { return Err("Sólo el administrador local puede gestionar canales".into()); }
                                change_channels(hosts.get(&server_id).ok_or("Encendé el servidor para gestionar canales")?, ChannelChange::Create(name,kind)).await?;
                            }
                            Command::RenameChannel { server_id, channel_id, name } => {
                                if !store.owns(&server_id) { return Err("Sólo el administrador local puede gestionar canales".into()); }
                                change_channels(hosts.get(&server_id).ok_or("Encendé el servidor para gestionar canales")?, ChannelChange::Rename(channel_id,name)).await?;
                            }
                            Command::DeleteChannel { server_id, channel_id } => {
                                if !store.owns(&server_id) { return Err("Sólo el administrador local puede gestionar canales".into()); }
                                change_channels(hosts.get(&server_id).ok_or("Encendé el servidor para gestionar canales")?, ChannelChange::Delete(channel_id)).await?;
                            }
                            Command::SelectText { channel_id } => {
                                let room = state.room.as_mut().ok_or("Conectate a un servidor")?;
                                if !room.channels.iter().any(|c| c.id == channel_id && c.kind == ChannelKind::Text) { return Err("El canal de texto no existe".into()); }
                                client.as_ref().ok_or("Conectate a un servidor")?.request(Request::SelectText { channel_id: channel_id.clone() })?;
                                room.selected_text = channel_id; room.history.clear(); room.has_older_messages = false;
                            }
                            Command::JoinVoice { channel_id } => { begin_voice(client.as_mut(), &state, &audio, channel_id)?; }
                            Command::Join => {
                                let room = state.room.as_ref().ok_or("Conectate a un servidor")?;
                                let channel_id = room.channels.iter().find(|c| c.kind == ChannelKind::Voice).ok_or("No hay salas de voz")?.id.clone();
                                begin_voice(client.as_mut(), &state, &audio, channel_id)?;
                            }
                            Command::Leave => {
                                if let Some(client) = &mut client { client.desired_voice = None; client.voice_epoch.store(0, std::sync::atomic::Ordering::SeqCst); client.request(Request::LeaveVoice)?; }
                                audio.command(Command::Leave)?;
                            }
                            Command::Preferences(preferences) => {
                                store.save_preferences(preferences)?; state.preferences = store.preferences();
                                audio.configure_shortcuts([state.preferences.mute_shortcut, state.preferences.deafen_shortcut]);
                            }
                            Command::RegenerateInvitation { id } => {
                                let identity = store.regenerate_invitation(&id)?;
                                if let Some(host) = hosts.get_mut(&id) { host.state.lock().await.token = identity.token.clone(); host.identity = identity; }
                                let _ = events.send(Event::Notice("Invitaciones anteriores revocadas. Generá una nueva desde Invitar. Las personas conectadas pueden permanecer.".into()));
                            }
                            Command::ConfigureServer { id, settings } => {
                                if let Some(host) = hosts.get_mut(&id) {
                                    if host.endpoint.local_addr().map_err(|e| e.to_string())?.port() != settings.port { return Err("Apagá este servidor antes de cambiar su puerto. Sus datos se conservarán.".into()); }
                                    let gate = host.state.lock().await.mutations.clone(); let _mutation = gate.lock().await;
                                    let (history,was_persistent,memory,files,channels) = { let room = host.state.lock().await; (room.history_store.clone().ok_or("Historial no disponible")?,room.persistent,room.history.iter().cloned().collect::<Vec<_>>(),room.files.clone(),room.channels.clone()) };
                                    let mut restored = None;
                                    if settings.persistent && !was_persistent {
                                        history.seed(memory).await?;
                                        let mut messages = Vec::new(); let mut more = false;
                                        for channel in channels.iter().filter(|c| c.kind == ChannelKind::Text) { let (page,has_more) = history.page_in(channel.id.clone(),u64::MAX).await?; messages.extend(page); more |= has_more; }
                                        restored = Some((messages,more,history.counters().await?));
                                    }
                                    let prior_limits = files.as_ref().map(|files| files.limits()).transpose()?;
                                    if let Some(files) = &files { files.set_limits(crate::files::Limits { max_file_bytes: settings.file_limit_bytes,max_server_bytes:settings.storage_limit_bytes })?; }
                                    if let Err(error) = store.configure_server(&id,settings.clone()) { if let (Some(files),Some(limits)) = (&files,prior_limits) { let _ = files.set_limits(limits); } return Err(error); }
                                    let mut room = host.state.lock().await;
                                    if let Some((messages,more,counters)) = restored { room.history = messages.into(); room.has_older_messages = more; room.next_chat = room.next_chat.max(counters.0); room.next_peer = room.next_peer.max(counters.1); }
                                    room.persistent = settings.persistent; room.max_participants = settings.max_participants as usize;
                                    if !settings.persistent { room.has_older_messages = false; }
                                    drop(room); drop(_mutation); host.rename(host.identity.name.clone()).await;
                                } else { store.configure_server(&id,settings)?; }
                                let _ = events.send(Event::Notice("Configuración del servidor guardada.".into()));
                            }
                            Command::SetDisplayName(name) => { store.save_display_name(name)?; state.display_name = store.display_name(); }
                            Command::DiagnoseNetwork => {
                                let port = state.host.as_ref().map_or(state.preferences.port, |h| h.port);
                                state.diagnostics.udp_status = if state.hosts.iter().any(|h| h.port == port) { format!("NIP está escuchando en UDP {port}.") }
                                    else { match std::net::UdpSocket::bind((if local_only { std::net::Ipv4Addr::LOCALHOST } else { std::net::Ipv4Addr::UNSPECIFIED }, port)) { Ok(_) => format!("Windows permite enlazar UDP {port}; el puerto está disponible."), Err(error) => udp_error(port, &error) } };
                                state.diagnostics.local_addresses = if local_only { vec![format!("127.0.0.1:{port}")] } else { local_addresses(port) };
                                refresh_connection_diagnostics(&mut state, client.as_ref(), pending.as_ref());
                            }
                            Command::OpenMicrophonePermissions => { open_system_settings("ms-settings:privacy-microphone")?; }
                            Command::OpenSoundSettings => { open_system_settings("ms-settings:sound")?; }
                            Command::OpenSecuritySettings => { open_system_settings("ms-settings:windowsdefender")?; }
                            Command::Create { name, port, user, persistent } => {
                                if user.trim().is_empty() || user.chars().count() > 64 { return Err("Indicá tu nombre (hasta 64 caracteres).".into()); }
                                // Retain the OS-assigned QA socket through profile initialization.
                                let reserved_socket = if local_only && port == 0 {
                                    Some(std::net::UdpSocket::bind((std::net::Ipv4Addr::LOCALHOST, 0)).map_err(|e| udp_error(0, &e))?)
                                } else { None };
                                let port = match &reserved_socket { Some(socket) => socket.local_addr().map_err(|e| e.to_string())?.port(), None => port };
                                let server = store.create_server(&name, LocalServerSettings { persistent, auto_start: persistent, port, max_participants: state.preferences.max_participants, ..Default::default() })?;
                                let invitation = start_server(&store, &mut hosts, &server.identity.id(), local_only, reserved_socket).await?;
                                pending = Some(connect_pending(invitation, user, updates.clone(), audio.clone(), local_only));
                            }
                            Command::StartServer { id, user } => {
                                let invitation = start_server(&store, &mut hosts, &id, local_only, None).await?;
                                pending = Some(connect_pending(invitation, user, updates.clone(), audio.clone(), local_only));
                            }
                            Command::History { before } => { let channel_id = state.room.as_ref().ok_or("Conectate a un servidor")?.selected_text.clone(); client.as_ref().ok_or("Conectate al servidor para consultar el historial.")?.request(Request::HistoryIn { channel_id, before })?; }
                            Command::Connect { invitation, user } => {
                                let invitation = Invitation::parse(&invitation)?;
                                pending = Some(connect_pending(invitation, user, updates.clone(), audio.clone(), local_only));
                            }
                            Command::Reconnect { id, user } => {
                                let invitation = if store.local_server(&id).is_some() { start_server(&store, &mut hosts, &id, local_only, None).await? } else { store.invitation(&id).ok_or("Servidor guardado inexistente")? };
                                pending = Some(connect_pending(invitation, user, updates.clone(), audio.clone(), local_only));
                            }
                            Command::Invite { id, endpoint } => {
                                let id = id.or_else(|| state.host.as_ref().map(|h| h.id.clone())).ok_or("Primero encendé un servidor propio")?;
                                let invite = hosts.get(&id).ok_or("El servidor está apagado")?.invitation(endpoint)?;
                                let _ = events.send(Event::Invitation(invite.encode()?));
                            }
                            Command::Chat(text) => { let channel_id = state.room.as_ref().ok_or("Conectate a un servidor")?.selected_text.clone(); client.as_ref().ok_or("Conectate a un grupo para enviar mensajes")?.request(Request::ChatIn { channel_id, text })?; }
                            Command::SendChat { server_id, channel_id, text } => { if !state.room.as_ref().is_some_and(|r| r.id == server_id) { return Err("El servidor cambió; revisá el destino antes de enviar".into()); } client.as_ref().ok_or("Conectate a un servidor")?.request(Request::ChatIn { channel_id, text })?; }
                            Command::Disconnect => { pending = None; disconnect(&mut client, &mut state, &audio); }
                            Command::CancelConnect => { pending = None; }
                            Command::StopHost => { if let Some(info) = &state.host { let id = info.id.clone(); stop_server(&id, &mut hosts, &mut client, &mut state, &audio).await; } }
                            Command::StopServer { id } => { stop_server(&id, &mut hosts, &mut client, &mut state, &audio).await; }
                            Command::DeleteServer { id } => {
                                if pending.as_ref().is_some_and(|p| p.invitation.as_ref().is_some_and(|i| i.id() == id)) { pending = None; }
                                stop_server(&id, &mut hosts, &mut client, &mut state, &audio).await;
                                if state.room.as_ref().is_some_and(|r| r.id == id) { disconnect(&mut client, &mut state, &audio); }
                                store.delete_server(&id)?;
                            }
                            Command::RenameServer { id, name } => {
                                store.rename_server(&id, &name)?;
                                if let Some(host) = hosts.get_mut(&id) { host.rename(name.trim().into()).await;
                                    if let Some(room) = state.room.as_mut().filter(|r| r.id == id) { room.name = name.trim().into(); }
                                }
                            }
                            Command::AudioSettings(settings) => { let previous = audio.status().settings; audio.command(Command::AudioSettings(settings.clone()))?; if let Err(error) = store.save_audio(settings) { let _ = audio.command(Command::AudioSettings(previous)); return Err(error); } }
                            Command::Shutdown => { disconnect(&mut client, &mut state, &audio); let _ = audio.command(Command::Shutdown); return Ok(()); }
                            command => { if matches!(command, Command::Join) && client.is_none() { return Err("Conectate a un grupo antes de entrar a la llamada".into()); } audio.command(command)?; }
                        }; Ok(()) }.await;
                        if let Err(error) = result { if connection_command { state.diagnostics.last_error = Some(error.clone()); } let _ = events.send(Event::Error(error)); }
                        if client.is_none() { *publication.lock().unwrap() = None; abort_transfers(&mut transfer_jobs,&events); }
                        state.saved = store.saved(); refresh_hosts(&mut state, &hosts);
                        state.connecting = pending.is_some();
                        refresh_connection_diagnostics(&mut state, client.as_ref(), pending.as_ref());
                        let _ = events.send(Event::State(state.clone()));
                    }
                    result = async { (&mut pending.as_mut().unwrap().task).await }, if pending.is_some() => {
                        let mut attempt = pending.take().unwrap();
                        state.connecting = false;
                        let result = result.map_err(|_| "La conexión se interrumpió".to_string()).and_then(|r| r).and_then(|(mut connected, room)| {
                            if let Some(invitation) = attempt.invitation.take() { store.remember(room.name.clone(), invitation.clone())?; reconnect_target = Some((invitation, attempt.user.clone())); }
                            disconnect(&mut client, &mut state, &audio);
                            abort_transfers(&mut transfer_jobs,&events);
                            if let Some(activation) = connected.activation.take() { let _ = activation.send(()); }
                            *publication.lock().unwrap() = Some((connected.connection.clone(),connected.voice_epoch.clone()));
                            client = Some(connected); state.room = Some(room); last_presence = None; state.saved = store.saved();
                            refresh_hosts(&mut state, &hosts);
                            retries = 0;
                            Ok(())
                        });
                        if let Err(error) = result {
                            state.diagnostics.last_error = Some(error.clone());
                            if retries > 0 && retries < 5 && state.preferences.auto_reconnect { retry_at = Some(Instant::now() + Duration::from_secs(1u64 << retries)); }
                            else { let _ = events.send(Event::Error(error)); }
                        }
                        refresh_connection_diagnostics(&mut state, client.as_ref(), pending.as_ref());
                        let _ = events.send(Event::State(state.clone()));
                    }
                    Some((session, update)) = received.recv() => { if !client.as_ref().is_some_and(|c| c.connection.stable_id() == session) { continue; } match update {
                        Payload::Response(Response::StorageUsage(usage)) => { if let Some(room) = &mut state.room { room.storage_usage = usage; } let _ = events.send(Event::State(state.clone())); }
                        Payload::Response(Response::Welcome(updated)) => {
                            if state.room.as_ref().is_some_and(|r| r.id == updated.id && r.self_id == updated.self_id) {
                                let room = state.room.as_mut().unwrap(); room.name = updated.name.clone(); room.persistent = updated.persistent; room.channels = updated.channels.clone(); if room.selected_text == updated.selected_text { room.has_older_messages = updated.has_older_messages; room.history.extend(updated.history.clone()); room.history.sort_by_key(|m| m.id); room.history.dedup_by_key(|m| m.id); }
                                if let Some(invite) = store.invitation(&updated.id) {
                                    if let Err(error) = store.remember(updated.name, invite) { let _ = events.send(Event::Error(error)); }
                                    state.saved = store.saved();
                                }
                                let _ = events.send(Event::State(state.clone()));
                            }
                        }
                        Payload::Response(Response::VoiceJoined { channel_id, epoch, request_id }) => {
                            if let Some(client) = &mut client { if client.voice_request == request_id && client.desired_voice.as_ref() == Some(&channel_id) {
                                if !audio.status().calling { if let Err(error) = audio.command(Command::Join) { let _ = events.send(Event::Error(error)); client.desired_voice = None; let _ = client.request(Request::LeaveVoice); continue; } }

                                client.voice_epoch.store(epoch, std::sync::atomic::Ordering::SeqCst); audio.set_transport_ready(true);
                            } }
                        }
                        Payload::Response(Response::VoiceRejected { request_id, error }) => {
                            if let Some(client) = &mut client { if client.voice_request == request_id && client.desired_voice.is_some() {
                                client.desired_voice = None; client.voice_epoch.store(0,std::sync::atomic::Ordering::SeqCst); audio.set_transport_ready(false); let _ = audio.command(Command::Leave); let _ = client.request(Request::LeaveVoice); let _ = events.send(Event::Error(error));
                            } }
                        }
                        Payload::Response(Response::Channels(channels)) => {
                            if let Some(room) = &mut state.room {
                                room.channels = channels;
                                if !room.channels.iter().any(|c| c.id == room.selected_text && c.kind == ChannelKind::Text) {
                                    room.selected_text = room.channels.iter().find(|c| c.kind == ChannelKind::Text).unwrap().id.clone(); room.history.clear(); room.has_older_messages = false;
                                    if let Some(client) = &client { let _ = client.request(Request::SelectText { channel_id: room.selected_text.clone() }); }
                                }
                                if let Some(client) = &mut client { if client.desired_voice.as_ref().is_some_and(|id| !room.channels.iter().any(|c| &c.id == id && c.kind == ChannelKind::Voice)) { client.desired_voice = None; client.voice_epoch.store(0, std::sync::atomic::Ordering::SeqCst); let _ = audio.command(Command::Leave); } }
                            }
                            let _ = events.send(Event::State(state.clone()));
                        }
                        Payload::Response(Response::SelectedText { channel_id, messages, has_more }) => {
                            if let Some(room) = &mut state.room { if room.selected_text == channel_id { room.history = messages; room.has_older_messages = has_more; let _ = events.send(Event::State(state.clone())); } }
                        }
                        Payload::Response(Response::History { channel_id, messages, has_more }) => {
                            if !state.room.as_ref().is_some_and(|r| r.selected_text == channel_id) { continue; }
                            if let Some(room) = &mut state.room { room.history.extend(messages.clone()); room.history.sort_by_key(|m| m.id); room.history.dedup_by_key(|m| m.id); room.has_older_messages = has_more; }
                            let _ = events.send(Event::History { messages, has_more });
                        }
                        Payload::Response(Response::Members(members)) => { if let Some(room) = state.room.as_mut() { for previous in &room.members { if !members.iter().any(|p| p.id == previous.id) { audio.remove_peer(previous.id); } else if members.iter().any(|p| p.id == previous.id && p.voice_channel != previous.voice_channel) { audio.flush_receive(); } } room.members = members.clone(); let _ = events.send(Event::Members(members)); let _ = events.send(Event::State(state.clone())); } }
                        Payload::Response(Response::Chat(message)) => { if let Some(room) = state.room.as_mut() { if message.channel_id == room.selected_text { room.history.push(message.clone()); room.history.sort_by_key(|m| m.id); if !room.persistent && room.history.len() > 100 { room.history.remove(0); } let _ = events.send(Event::Chat(message)); let _ = events.send(Event::State(state.clone())); } } }
                        Payload::Response(Response::Error(error)) => { let _ = events.send(Event::Error(error)); }
                        Payload::Closed => { *publication.lock().unwrap() = None; abort_transfers(&mut transfer_jobs,&events); if client.is_some() { disconnect(&mut client, &mut state, &audio); if state.preferences.auto_reconnect && reconnect_target.is_some() { retry_at = Some(Instant::now() + Duration::from_secs(1)); } let _ = events.send(Event::Disconnected("Conexión perdida. La reconexión, si está activada, no reactiva el micrófono.".into())); let _ = events.send(Event::State(state.clone())); } }
                    } }
                    _ = status_tick.tick() => {
                        for action in audio.take_shortcut_actions() { let _ = audio.command(action); }
                        if !state.preferences.auto_reconnect { retry_at = None; }
                        if retry_at.is_some_and(|at| Instant::now() >= at) && pending.is_none() {
                            retry_at = None;
                            if let Some((invite, user)) = &reconnect_target { retries += 1; pending = Some(connect_pending(invite.clone(), user.clone(), updates.clone(), audio.clone(), local_only)); state.connecting = true; refresh_connection_diagnostics(&mut state, client.as_ref(), pending.as_ref()); let _ = events.send(Event::State(state.clone())); }
                        }
                        let status = audio.status(); let presence = (status.calling, status.muted || status.deafened || status.testing || (status.settings.advanced.push_to_talk && !status.push_to_talk_active)); if last_presence != Some(presence) { if let Some(client) = &client { if client.request(Request::Presence {calling: presence.0, muted: presence.1}).is_ok() { last_presence = Some(presence); } } }
                    }
                }
            }
            publishing_task.abort(); drop(pending); drop(client); let _ = audio.command(Command::Shutdown);
            abort_transfers(&mut transfer_jobs,&events);
            for (_, host) in hosts.drain() { host.shutdown().await; }
            state.hosts.clear(); state.host = None; state.room = None; state.connecting = false;
            refresh_connection_diagnostics(&mut state, None, None);
            let _ = events.send(Event::State(state));
        });
    }).expect("crear hilo de red");
    commands
}
struct TransferTask {
    task: tokio::task::JoinHandle<()>,
    completed: Arc<std::sync::atomic::AtomicBool>,
}
impl TransferTask {
    fn is_finished(&self) -> bool {
        self.completed.load(std::sync::atomic::Ordering::SeqCst) || self.task.is_finished()
    }
    fn abort(&self) {
        self.task.abort();
    }
}
fn abort_transfers(
    jobs: &mut HashMap<String, TransferTask>,
    events: &std::sync::mpsc::Sender<Event>,
) {
    for (operation_id, job) in jobs.drain() {
        if !job.is_finished() {
            job.abort();
            let _ = events.send(Event::Transfer(TransferStatus {
                operation_id,
                name: "Archivo".into(),
                completed: 0,
                total: 0,
                finished: true,
                error: Some("Conexión cerrada; podés reintentar al reconectar".into()),
            }));
        }
    }
}
enum ChannelChange {
    Create(String, ChannelKind),
    Rename(String, String),
    Delete(String),
}
async fn change_channels(host: &Host, change: ChannelChange) -> Result<(), String> {
    let (gate, history) = {
        let room = host.state.lock().await;
        (
            room.mutations.clone(),
            room.history_store.clone().ok_or("Catálogo no disponible")?,
        )
    };
    let _mutation = gate.lock().await;
    let deleted = match change {
        ChannelChange::Create(name, kind) => {
            history.create_channel(name, kind).await?;
            None
        }
        ChannelChange::Rename(id, name) => {
            history.rename_channel(id, name).await?;
            None
        }
        ChannelChange::Delete(id) => {
            history.delete_channel(id.clone()).await?;
            Some(id)
        }
    };
    let channels = history.channels().await?;
    let mut room = host.state.lock().await;
    let mut cleanup_error = None;
    if let Some(id) = deleted {
        room.history.retain(|m| m.channel_id != id);
        for peer in room
            .peers
            .values_mut()
            .filter(|p| p.member.voice_channel.as_ref() == Some(&id))
        {
            peer.member.voice_channel = None;
            peer.member.calling = false;
            peer.voice_epoch = peer.voice_epoch.wrapping_add(1);
        }
        let files = room.files.clone();
        drop(room);
        if let Some(files) = files {
            cleanup_error = tokio::task::spawn_blocking(move || files.delete_channel(&id))
                .await
                .map_err(|_| "Se interrumpió la limpieza".to_string())
                .and_then(|result| result)
                .err();
        }
        room = host.state.lock().await;
    }
    room.channels = channels.clone();
    room.broadcast(Response::Channels(channels));
    room.presence();
    if let Some(error) = cleanup_error {
        return Err(format!(
            "El canal se eliminó; hay limpieza de archivos pendiente que se reintentará al iniciar: {error}"
        ));
    }
    Ok(())
}
fn begin_voice(
    client: Option<&mut Client>,
    state: &AppState,
    audio: &AudioHandle,
    channel_id: String,
) -> Result<(), String> {
    let room = state.room.as_ref().ok_or("Conectate a un servidor")?;
    if !room
        .channels
        .iter()
        .any(|c| c.id == channel_id && c.kind == ChannelKind::Voice)
    {
        return Err("La sala de voz no existe".into());
    }
    let client = client.ok_or("Conectate a un servidor")?;
    client.voice_request = client.voice_request.wrapping_add(1).max(1);
    audio.set_transport_ready(false);
    client
        .voice_epoch
        .store(0, std::sync::atomic::Ordering::SeqCst);
    client.request(Request::JoinVoice {
        channel_id: channel_id.clone(),
        request_id: client.voice_request,
    })?;
    client.desired_voice = Some(channel_id);
    Ok(())
}
fn refresh_connection_diagnostics(
    state: &mut AppState,
    client: Option<&Client>,
    pending: Option<&PendingConnection>,
) {
    let diagnostics = &mut state.diagnostics;
    diagnostics.pending_endpoint = pending
        .and_then(|attempt| attempt.invitation.as_ref())
        .map(|invite| invite.address.clone());
    diagnostics.transport = client.map(|c| {
        if c.relay.is_some() {
            "QUIC/TLS 1.3 · relay TLS/TCP".into()
        } else {
            "QUIC/TLS 1.3 · directo".into()
        }
    });
    diagnostics.remote_endpoint = client.map(|client| client.remote_endpoint.clone());
    diagnostics.rtt_ms = client.map(|client| client.connection.rtt().as_secs_f64() * 1000.0);
    let stats = client.map(|client| client.connection.stats());
    diagnostics.sent_packets = stats.as_ref().map_or(0, |stats| stats.path.sent_packets);
    diagnostics.lost_packets = stats.as_ref().map_or(0, |stats| stats.path.lost_packets);
}
fn open_system_settings(uri: &str) -> Result<(), String> {
    std::process::Command::new("explorer.exe")
        .arg(uri)
        .spawn()
        .map(|_| ())
        .map_err(|_| {
            "No se pudo abrir Configuración de Windows. Abrila desde el menú Inicio.".into()
        })
}
fn disconnect(client: &mut Option<Client>, state: &mut AppState, audio: &AudioHandle) {
    audio.set_transport_ready(false);
    *client = None;
    refresh_connection_diagnostics(state, None, None);
    if let Some(room) = state.room.take() {
        for member in room.members {
            audio.remove_peer(member.id);
        }
    }
    let _ = audio.command(Command::Disconnect);
}
async fn start_server(
    store: &Store,
    hosts: &mut HashMap<String, Host>,
    id: &str,
    local_only: bool,
    reserved_socket: Option<std::net::UdpSocket>,
) -> Result<Invitation, String> {
    if let Some(host) = hosts.get(id) {
        let mut invitation = host.invitation(format!(
            "127.0.0.1:{}",
            host.endpoint
                .local_addr()
                .map_err(|e| e.to_string())?
                .port()
        ))?;
        invitation.relay = None;
        return Ok(invitation);
    }
    let server = store
        .local_server(id)
        .ok_or("El servidor no pertenece a este perfil.")?;
    let host =
        Host::start_local(server, store.history_path(id)?, local_only, reserved_socket).await?;
    let mut invitation = host.invitation(format!(
        "127.0.0.1:{}",
        host.endpoint
            .local_addr()
            .map_err(|e| e.to_string())?
            .port()
    ))?;
    invitation.relay = None;
    hosts.insert(id.into(), host);
    Ok(invitation)
}
fn refresh_hosts(state: &mut AppState, hosts: &HashMap<String, Host>) {
    state.hosts = hosts
        .iter()
        .filter_map(|(id, host)| {
            host.endpoint.local_addr().ok().map(|address| HostInfo {
                id: id.clone(),
                name: host.identity.name.clone(),
                port: address.port(),
                addresses: if address.ip().is_loopback() {
                    vec![address.to_string()]
                } else {
                    local_addresses(address.port())
                },
            })
        })
        .collect();
    state.hosts.sort_by(|a, b| a.id.cmp(&b.id));
    state.host = state
        .room
        .as_ref()
        .and_then(|r| state.hosts.iter().find(|h| h.id == r.id))
        .or_else(|| state.hosts.first())
        .cloned();
}
async fn stop_server(
    id: &str,
    hosts: &mut HashMap<String, Host>,
    client: &mut Option<Client>,
    state: &mut AppState,
    audio: &AudioHandle,
) {
    if !hosts.contains_key(id) {
        return;
    }
    if state.room.as_ref().is_some_and(|r| r.id == id) {
        disconnect(client, state, audio);
    }
    if let Some(host) = hosts.remove(id) {
        host.shutdown().await;
    }
    refresh_hosts(state, hosts);
}
fn udp_error(port: u16, error: &std::io::Error) -> String {
    match error.kind() {
        std::io::ErrorKind::AddrInUse => format!(
            "El puerto UDP {port} está ocupado. Apagá la otra copia de NIP o elegí otro puerto en Ajustes → Servidores."
        ),
        std::io::ErrorKind::PermissionDenied => format!(
            "Windows no permitió abrir UDP {port}. Probá otro puerto en Ajustes → Servidores. Detalle: {error}"
        ),
        _ => format!("No se pudo abrir UDP {port}: {error}"),
    }
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
                version: PROTOCOL_VERSION,
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
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn delayed_voice_confirmation_cannot_open_capture_for_a_newer_request_to_same_room() {
        let host = identity();
        let config = quinn::ServerConfig::with_single_cert(
            vec![rustls::pki_types::CertificateDer::from(
                host.certificate.clone(),
            )],
            rustls::pki_types::PrivatePkcs8KeyDer::from(host.key.clone()).into(),
        )
        .unwrap();
        let endpoint = quinn::Endpoint::server(config, "127.0.0.1:0".parse().unwrap()).unwrap();
        let invite = Invitation {
            address: endpoint.local_addr().unwrap().to_string(),
            certificate: host.certificate,
            token: host.token,
            relay: None,
        };
        let server_id = invite.id();
        let server = endpoint.clone();
        let (stale_sent, stale_received) = tokio::sync::oneshot::channel();
        let (resume, allowed) = tokio::sync::oneshot::channel();
        let (finish, finished) = tokio::sync::oneshot::channel();
        let peer = tokio::spawn(async move {
            let connection = server.accept().await.unwrap().await.unwrap();
            let (mut send, mut recv) = connection.accept_bi().await.unwrap();
            assert!(matches!(
                read_frame::<Request>(&mut recv).await.unwrap(),
                Request::Authenticate {
                    version: PROTOCOL_VERSION,
                    ..
                }
            ));
            let mut channels = default_channels();
            channels.push(Channel {
                id: "other-voice".into(),
                name: "Otra".into(),
                kind: ChannelKind::Voice,
            });
            write_frame(
                &mut send,
                &Response::Welcome(Room {
                    storage_usage: Default::default(),
                    protocol_version: PROTOCOL_VERSION,
                    channels,
                    selected_text: GENERAL_TEXT_CHANNEL.into(),
                    persistent: false,
                    has_older_messages: false,
                    id: server_id,
                    name: "QA protocolo".into(),
                    self_id: 1,
                    members: vec![],
                    history: vec![],
                }),
            )
            .await
            .unwrap();
            let mut joins = Vec::new();
            while joins.len() < 3 {
                if let Request::JoinVoice {
                    channel_id,
                    request_id,
                } = read_frame::<Request>(&mut recv).await.unwrap()
                {
                    joins.push((channel_id, request_id));
                }
            }
            write_frame(
                &mut send,
                &Response::VoiceJoined {
                    channel_id: joins[0].0.clone(),
                    request_id: joins[0].1,
                    epoch: 1,
                },
            )
            .await
            .unwrap();
            stale_sent.send(()).unwrap();
            allowed.await.unwrap();
            write_frame(
                &mut send,
                &Response::VoiceJoined {
                    channel_id: joins[2].0.clone(),
                    request_id: joins[2].1,
                    epoch: 3,
                },
            )
            .await
            .unwrap();
            let _ = finished.await;
            connection.close(0u32.into(), b"test complete");
        });
        let profile =
            std::env::temp_dir().join(format!("nip-late-voice-{}", getrandom::u64().unwrap()));
        let (events, received) = std::sync::mpsc::channel();
        let (audio, frames) =
            crate::audio::AudioEngine::spawn_synthetic(events.clone(), Default::default()).unwrap();
        let commands = spawn_loopback(profile, events, audio.clone(), frames);
        commands
            .send(Command::Connect {
                invitation: invite.encode().unwrap(),
                user: "QA".into(),
            })
            .unwrap();
        loop {
            match received.recv_timeout(Duration::from_secs(5)).unwrap() {
                Event::State(state) if state.room.is_some() => break,
                Event::Error(error) => panic!("{error}"),
                _ => {}
            }
        }
        for channel_id in [GENERAL_VOICE_CHANNEL, "other-voice", GENERAL_VOICE_CHANNEL] {
            commands
                .send(Command::JoinVoice {
                    channel_id: channel_id.into(),
                })
                .unwrap();
        }
        stale_received.await.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(
            !audio.status().calling && !audio.outgoing_allowed(),
            "stale same-room acknowledgement opened capture"
        );
        resume.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        while !audio.status().calling {
            assert!(Instant::now() < deadline);
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        commands.send(Command::Shutdown).unwrap();
        finish.send(()).unwrap();
        peer.await.unwrap();
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
    async fn participant_limit_and_rotated_token_apply_to_new_connections() {
        let (mut host, invite) = start();
        let (_e, _c, mut existing, mut received, _) = authenticated(&invite, "Presente").await;
        host.state.lock().await.max_participants = 1;
        let (_e2, _c2, mut send, mut recv) = raw_connect(&invite).await.unwrap();
        write_frame(
            &mut send,
            &Request::Authenticate {
                version: PROTOCOL_VERSION,
                token: invite.token.clone(),
                name: "Sin lugar".into(),
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
        {
            let mut state = host.state.lock().await;
            state.max_participants = 3;
            state.token = vec![93; 32];
        }
        host.identity.token = vec![93; 32];
        let (_e3, _c3, mut send, mut recv) = raw_connect(&invite).await.unwrap();
        write_frame(
            &mut send,
            &Request::Authenticate {
                version: PROTOCOL_VERSION,
                token: invite.token.clone(),
                name: "Vieja invitación".into(),
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
        let rotated = host.invitation(invite.address).unwrap();
        let (_e4, _c4, _s4, _r4, room) = authenticated(&rotated, "Invitación nueva").await;
        assert_eq!(room.members.len(), 2);
        write_frame(&mut existing, &Request::Chat("Sigo conectado".into()))
            .await
            .unwrap();
        assert_eq!(next_chat(&mut received).await.text, "Sigo conectado");
    }
    #[tokio::test]
    async fn persistent_history_is_available_over_the_authenticated_connection() {
        let directory =
            std::env::temp_dir().join(format!("nip-history-wire-{}", getrandom::u64().unwrap()));
        let path = directory.join("history.sqlite3");
        let history = History::open(path.clone()).await.unwrap();
        history
            .seed(
                (1..=205)
                    .map(|id| ChatMessage {
                        attachments: vec![],
                        channel_id: GENERAL_TEXT_CHANNEL.into(),
                        id,
                        sender_id: 7,
                        sender_name: "Anterior".into(),
                        text: format!("Guardado {id}"),
                        sent_at: id,
                    })
                    .collect(),
            )
            .await
            .unwrap();
        drop(history);
        let host = Host::start_local(
            LocalServer {
                relay: None,
                identity: identity(),
                settings: LocalServerSettings {
                    port: 0,
                    persistent: true,
                    ..Default::default()
                },
            },
            path,
            true,
            None,
        )
        .await
        .unwrap();
        assert!(host.endpoint.local_addr().unwrap().ip().is_loopback());
        let invite = host
            .invitation(format!(
                "127.0.0.1:{}",
                host.endpoint.local_addr().unwrap().port()
            ))
            .unwrap();
        let (endpoint, connection, mut send, mut recv, room) =
            authenticated(&invite, "Lector").await;
        assert!(room.persistent && room.has_older_messages);
        assert_eq!(room.history.len(), 100);
        assert_eq!(room.history[0].id, 106);
        assert_eq!(room.self_id, 8);
        write_frame(&mut send, &Request::History { before: 106 })
            .await
            .unwrap();
        loop {
            if let Response::History {
                messages, has_more, ..
            } = read_frame(&mut recv).await.unwrap()
            {
                assert!(has_more);
                assert_eq!(messages.len(), 100);
                assert_eq!(messages[0].id, 6);
                break;
            }
        }
        drop(send);
        drop(recv);
        connection.close(0u32.into(), b"done");
        drop(connection);
        drop(endpoint);
        host.endpoint.close(0u32.into(), b"done");
        host.endpoint.wait_idle().await;
        drop(host);
        // Serving tasks may still be returning after QUIC has drained.
        tokio::task::yield_now().await;
        std::fs::remove_dir_all(directory).unwrap();
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
                version: PROTOCOL_VERSION,
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
            &Request::JoinVoice {
                request_id: 1,
                channel_id: GENERAL_VOICE_CHANNEL.into(),
            },
        )
        .await
        .unwrap();
        write_frame(
            &mut b,
            &Request::JoinVoice {
                request_id: 1,
                channel_id: GENERAL_VOICE_CHANNEL.into(),
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
        let mut frame = 1u64.to_be_bytes().to_vec();
        frame.extend_from_slice(b"opus-test");
        ca.send_datagram(bytes::Bytes::from(frame)).unwrap();
        let packet = tokio::time::timeout(Duration::from_secs(2), cb.read_datagram())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(&packet[..8], 1u64.to_be_bytes());
        assert_eq!(&packet[8..16], ra.self_id.to_be_bytes());
        assert_eq!(&packet[16..], b"opus-test");
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
        let (mut client, _) = Client::connect(&invite, "Nueva".into(), updates, audio, true)
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
