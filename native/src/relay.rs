//! Bounded TLS tunnels carry QUIC ciphertext without terminating the host session.
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, net::SocketAddr, sync::Arc, time::Duration};
use subtle::ConstantTimeEq;
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::{TcpListener, TcpStream, UdpSocket},
    sync::{Mutex, Semaphore, mpsc, oneshot},
    task::JoinHandle,
};
use tokio_rustls::{TlsAcceptor, TlsConnector};

const AUTH_TIMEOUT: Duration = Duration::from_secs(5);
const IDLE_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_PACKET: usize = 2048;

#[derive(Clone, Serialize, Deserialize)]
pub struct RelayEndpoint {
    pub address: SocketAddr,
    pub certificate: Vec<u8>,
    pub route: String,
    pub token: Vec<u8>,
}
impl std::fmt::Debug for RelayEndpoint {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RelayEndpoint")
            .field("address", &self.address)
            .field("route", &self.route)
            .finish_non_exhaustive()
    }
}
#[derive(Clone, Serialize, Deserialize)]
pub struct RelayRoute {
    pub id: String,
    pub host_token: Vec<u8>,
    pub client_token: Vec<u8>,
    pub max_tunnels: usize,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct RelayServerConfig {
    pub listen: SocketAddr,
    pub certificate: Vec<u8>,
    pub key: Vec<u8>,
    pub routes: Vec<RelayRoute>,
}
#[derive(Serialize, Deserialize)]
struct Authentication {
    route: String,
    token: Vec<u8>,
    host: bool,
}
type ServerTls = tokio_rustls::server::TlsStream<TcpStream>;
struct WaitingHost {
    pair: oneshot::Sender<oneshot::Sender<ServerTls>>,
}
struct Route {
    config: RelayRoute,
    hosts: mpsc::Sender<WaitingHost>,
    waiting: Mutex<mpsc::Receiver<WaitingHost>>,
    members: Semaphore,
}
pub struct RelayServer {
    address: SocketAddr,
    task: JoinHandle<()>,
}
impl Drop for RelayServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl RelayServer {
    pub async fn start(config: RelayServerConfig) -> Result<Self, String> {
        if config.routes.is_empty() || config.routes.len() > 128 {
            return Err("Configurá entre 1 y 128 rutas relay".into());
        }
        let mut routes = HashMap::new();
        for route in config.routes {
            if !valid_route(&route.id)
                || route.host_token.len() != 32
                || route.client_token.len() != 32
                || bool::from(route.host_token.ct_eq(&route.client_token))
                || !(1..=32).contains(&route.max_tunnels)
            {
                return Err("Ruta relay inválida: usá credenciales distintas de 32 bytes y entre 1 y 32 túneles".into());
            }
            let (hosts, waiting) = mpsc::channel(route.max_tunnels);
            let id = route.id.clone();
            let entry = Arc::new(Route {
                members: Semaphore::new(route.max_tunnels),
                config: route,
                hosts,
                waiting: Mutex::new(waiting),
            });
            if routes.insert(id, entry).is_some() {
                return Err("Ruta relay duplicada".into());
            }
        }
        let tls = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(
                vec![rustls::pki_types::CertificateDer::from(config.certificate)],
                rustls::pki_types::PrivatePkcs8KeyDer::from(config.key).into(),
            )
            .map_err(|_| "Certificado o clave del relay inválidos")?;
        let acceptor = TlsAcceptor::from(Arc::new(tls));
        let listener = TcpListener::bind(config.listen)
            .await
            .map_err(|e| format!("No se pudo abrir el relay TCP: {e}"))?;
        let address = listener.local_addr().map_err(|e| e.to_string())?;
        let task = tokio::spawn(async move {
            let routes = Arc::new(routes);
            let slots = Arc::new(Semaphore::new(256));
            let mut sessions = tokio::task::JoinSet::new();
            loop {
                tokio::select! {
                    accepted = listener.accept() => {
                        let Ok((socket, _)) = accepted else { break; };
                        let Ok(slot) = slots.clone().try_acquire_owned() else { continue; };
                        let acceptor = acceptor.clone(); let routes = routes.clone();
                        sessions.spawn(async move { let _slot = slot; let _ = serve_tunnel(socket, acceptor, routes).await; });
                    }
                    _ = sessions.join_next(), if !sessions.is_empty() => {}
                }
            }
        });
        Ok(Self { address, task })
    }
    pub fn address(&self) -> SocketAddr {
        self.address
    }
}
impl RelayEndpoint {
    pub fn validate(&self) -> Result<(), String> {
        if self.address.port() == 0
            || self.certificate.is_empty()
            || self.certificate.len() > 8192
            || !valid_route(&self.route)
            || self.token.len() != 32
        {
            return Err("Configuración del relay inválida".into());
        }
        Ok(())
    }
}
fn valid_route(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}
async fn serve_tunnel(
    socket: TcpStream,
    acceptor: TlsAcceptor,
    routes: Arc<HashMap<String, Arc<Route>>>,
) -> Result<(), String> {
    let (mut stream, auth) = tokio::time::timeout(AUTH_TIMEOUT, async {
        socket.set_nodelay(true).map_err(|e| e.to_string())?;
        let mut stream = acceptor
            .accept(socket)
            .await
            .map_err(|_| "TLS relay rechazado")?;
        let size = stream
            .read_u16()
            .await
            .map_err(|_| "Autenticación relay incompleta")? as usize;
        if size == 0 || size > 1024 {
            return Err("Autenticación relay demasiado grande".into());
        }
        let mut data = vec![0; size];
        stream
            .read_exact(&mut data)
            .await
            .map_err(|_| "Autenticación relay incompleta")?;
        let auth: Authentication =
            serde_json::from_slice(&data).map_err(|_| "Autenticación relay inválida")?;
        Ok::<_, String>((stream, auth))
    })
    .await
    .map_err(|_| "Tiempo de autenticación relay agotado")??;
    let route = routes.get(&auth.route).ok_or("Ruta relay desconocida")?;
    let expected = if auth.host {
        &route.config.host_token
    } else {
        &route.config.client_token
    };
    if auth.token.len() != 32 || !bool::from(expected.ct_eq(&auth.token)) {
        return Err("Credencial relay inválida".into());
    }
    stream.write_u8(1).await.map_err(|_| "Relay desconectado")?;
    stream.flush().await.map_err(|_| "Relay desconectado")?;
    if auth.host {
        let (pair, paired) = oneshot::channel();
        route
            .hosts
            .try_send(WaitingHost { pair })
            .map_err(|_| "Cola de anfitriones completa")?;
        // Detect an abandoned outbound host before its queued slot is reused.
        let deliver = tokio::select! {
            result = paired => result.map_err(|_| "Cliente relay cancelado")?,
            _ = stream.read_u8() => return Err("Anfitrión relay desconectado".into()),
        };
        stream
            .write_u8(1)
            .await
            .map_err(|_| "Anfitrión relay desconectado")?;
        stream
            .flush()
            .await
            .map_err(|_| "Anfitrión relay desconectado")?;
        let _ = deliver.send(stream);
    } else {
        let _slot = route
            .members
            .try_acquire()
            .map_err(|_| "Límite de clientes relay alcanzado")?;
        let host = tokio::time::timeout(AUTH_TIMEOUT, async {
            let mut waiting = route.waiting.lock().await;
            loop {
                let queued = waiting.recv().await.ok_or("Relay cerrado")?;
                let (deliver, delivered) = oneshot::channel();
                if queued.pair.send(deliver).is_err() {
                    continue;
                }
                if let Ok(stream) = delivered.await {
                    return Ok::<_, String>(stream);
                }
            }
        })
        .await
        .map_err(|_| "El anfitrión no está disponible en relay")??;
        stream
            .write_u8(1)
            .await
            .map_err(|_| "Cliente relay desconectado")?;
        stream
            .flush()
            .await
            .map_err(|_| "Cliente relay desconectado")?;
        let (mut hr, mut hw) = tokio::io::split(host);
        let (mut cr, mut cw) = tokio::io::split(stream);
        tokio::select! { _ = forward_packets(&mut hr, &mut cw) => {}, _ = forward_packets(&mut cr, &mut hw) => {} }
    }
    Ok(())
}
async fn forward_packets(
    reader: &mut (impl AsyncRead + Unpin),
    writer: &mut (impl AsyncWrite + Unpin),
) -> Result<(), String> {
    let mut buffer = [0u8; MAX_PACKET];
    let mut bytes = 0usize;
    let mut window = tokio::time::Instant::now();
    loop {
        let size = read_packet(reader, &mut buffer).await?;
        writer
            .write_u16(size as u16)
            .await
            .map_err(|_| "Relay desconectado")?;
        writer
            .write_all(&buffer[..size])
            .await
            .map_err(|_| "Relay desconectado")?;
        writer.flush().await.map_err(|_| "Relay desconectado")?;
        bytes += size;
        if bytes >= 8 * 1024 * 1024 {
            tokio::time::sleep_until(window + Duration::from_secs(1)).await;
            bytes = 0;
            window = tokio::time::Instant::now();
        }
    }
}
async fn read_packet(
    reader: &mut (impl AsyncRead + Unpin),
    buffer: &mut [u8; MAX_PACKET],
) -> Result<usize, String> {
    tokio::time::timeout(IDLE_TIMEOUT, async {
        let size = reader.read_u16().await.map_err(|_| "Relay desconectado")? as usize;
        if size == 0 || size > MAX_PACKET {
            return Err("Paquete relay inválido".into());
        }
        reader
            .read_exact(&mut buffer[..size])
            .await
            .map_err(|_| "Paquete relay incompleto")?;
        Ok(size)
    })
    .await
    .map_err(|_| "Relay inactivo durante 60 s")?
}
async fn connect_tls(
    endpoint: &RelayEndpoint,
    host: bool,
) -> Result<tokio_rustls::client::TlsStream<TcpStream>, String> {
    endpoint.validate()?;
    let mut roots = rustls::RootCertStore::empty();
    roots
        .add(rustls::pki_types::CertificateDer::from(
            endpoint.certificate.clone(),
        ))
        .map_err(|_| "Certificado relay inválido")?;
    let config = rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    tokio::time::timeout(AUTH_TIMEOUT, async {
        let socket = TcpStream::connect(endpoint.address)
            .await
            .map_err(|e| format!("No se pudo conectar al relay: {e}"))?;
        socket.set_nodelay(true).map_err(|e| e.to_string())?;
        let mut stream = TlsConnector::from(Arc::new(config))
            .connect(
                rustls::pki_types::ServerName::try_from("nip-relay.local").unwrap(),
                socket,
            )
            .await
            .map_err(|_| "No se pudo verificar la identidad TLS del relay")?;
        let data = serde_json::to_vec(&Authentication {
            route: endpoint.route.clone(),
            token: endpoint.token.clone(),
            host,
        })
        .map_err(|e| e.to_string())?;
        stream
            .write_u16(data.len() as u16)
            .await
            .map_err(|_| "Relay desconectado")?;
        stream
            .write_all(&data)
            .await
            .map_err(|_| "Relay desconectado")?;
        stream.flush().await.map_err(|_| "Relay desconectado")?;
        if stream
            .read_u8()
            .await
            .map_err(|_| "Relay rechazó la credencial o ruta")?
            != 1
        {
            return Err("Relay rechazó la autenticación".into());
        }
        Ok(stream)
    })
    .await
    .map_err(|_| "El relay no respondió en 5 s")?
}
pub struct RelayHost {
    tasks: Vec<JoinHandle<()>>,
}
impl Drop for RelayHost {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}
impl RelayHost {
    pub async fn start(
        endpoint: RelayEndpoint,
        target: SocketAddr,
        tunnels: usize,
    ) -> Result<Self, String> {
        if !(1..=32).contains(&tunnels) || !target.ip().is_loopback() || target.port() == 0 {
            return Err("Usá entre 1 y 32 túneles y un anfitrión loopback válido".into());
        }
        let mut first = Some(connect_tls(&endpoint, true).await?);
        let mut tasks = Vec::new();
        for _ in 0..tunnels {
            let endpoint = endpoint.clone();
            let mut initial = first.take();
            tasks.push(tokio::spawn(async move {
                loop {
                    let connected = match initial.take() {
                        Some(stream) => Ok(stream),
                        None => connect_tls(&endpoint, true).await,
                    };
                    if let Ok(mut stream) = connected {
                        if matches!(stream.read_u8().await, Ok(1)) {
                            if let Ok(socket) = UdpSocket::bind(if target.is_ipv4() {
                                "127.0.0.1:0"
                            } else {
                                "[::1]:0"
                            })
                            .await
                            {
                                if socket.connect(target).await.is_ok() {
                                    let _ = bridge(stream, socket, Some(target)).await;
                                }
                            }
                        }
                    }
                    tokio::time::sleep(Duration::from_millis(500)).await;
                }
            }));
        }
        Ok(Self { tasks })
    }
}
pub struct RelayClient {
    address: SocketAddr,
    task: JoinHandle<()>,
}
impl Drop for RelayClient {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl RelayClient {
    pub async fn connect(endpoint: &RelayEndpoint) -> Result<Self, String> {
        let mut stream = connect_tls(endpoint, false).await?;
        if tokio::time::timeout(AUTH_TIMEOUT, stream.read_u8())
            .await
            .map_err(|_| "El anfitrión no está disponible en relay")?
            .map_err(|_| "Relay sin anfitrión disponible")?
            != 1
        {
            return Err("Relay rechazó el emparejamiento".into());
        }
        let socket = UdpSocket::bind("127.0.0.1:0")
            .await
            .map_err(|e| e.to_string())?;
        let address = socket.local_addr().map_err(|e| e.to_string())?;
        let task = tokio::spawn(async move {
            let _ = bridge(stream, socket, None).await;
        });
        Ok(Self { address, task })
    }
    pub fn address(&self) -> SocketAddr {
        self.address
    }
}
async fn bridge(
    stream: tokio_rustls::client::TlsStream<TcpStream>,
    socket: UdpSocket,
    peer: Option<SocketAddr>,
) -> Result<(), String> {
    let (mut reader, mut writer) = tokio::io::split(stream);
    let target = if let Some(target) = peer {
        target
    } else {
        let mut initial = [0u8; MAX_PACKET + 1];
        let (size, source) = tokio::time::timeout(AUTH_TIMEOUT, socket.recv_from(&mut initial))
            .await
            .map_err(|_| "Cliente local relay inactivo")?
            .map_err(|e| e.to_string())?;
        if size == 0 || size > MAX_PACKET {
            return Err("Paquete relay inválido".into());
        }
        writer
            .write_u16(size as u16)
            .await
            .map_err(|_| "Relay desconectado")?;
        writer
            .write_all(&initial[..size])
            .await
            .map_err(|_| "Relay desconectado")?;
        writer.flush().await.map_err(|_| "Relay desconectado")?;
        source
    };
    let receive = async {
        let mut incoming = [0u8; MAX_PACKET];
        loop {
            let size = read_packet(&mut reader, &mut incoming).await?;
            socket
                .send_to(&incoming[..size], target)
                .await
                .map_err(|e| e.to_string())?;
        }
        #[allow(unreachable_code)]
        Ok::<(), String>(())
    };
    let send = async {
        let mut outgoing = [0u8; MAX_PACKET + 1];
        loop {
            let (size, source) = socket
                .recv_from(&mut outgoing)
                .await
                .map_err(|e| e.to_string())?;
            if source != target || size == 0 || size > MAX_PACKET {
                continue;
            }
            writer
                .write_u16(size as u16)
                .await
                .map_err(|_| "Relay desconectado")?;
            writer
                .write_all(&outgoing[..size])
                .await
                .map_err(|_| "Relay desconectado")?;
            writer.flush().await.map_err(|_| "Relay desconectado")?;
        }
        #[allow(unreachable_code)]
        Ok::<(), String>(())
    };
    tokio::select! { result = receive => result, result = send => result }
}
