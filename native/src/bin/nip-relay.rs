use nip_native::relay::{RelayEndpoint, RelayRoute, RelayServer, RelayServerConfig};
use std::{fs::OpenOptions, io::Write, net::SocketAddr, path::Path};

fn write_new(path: &Path, value: &impl serde::Serialize) -> Result<(), String> {
    let data = serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| format!("No se pudo crear {}: {e}", path.display()))?;
    file.write_all(&data).map_err(|e| e.to_string())?;
    file.sync_all().map_err(|e| e.to_string())
}
fn initialize(directory: &Path, address: SocketAddr) -> Result<(), String> {
    if address.port() == 0 {
        return Err("Elegí un puerto TCP distinto de cero".into());
    }
    std::fs::create_dir(directory).map_err(|e| format!("Usá una carpeta nueva: {e}"))?;
    let identity = rcgen::generate_simple_self_signed(vec!["nip-relay.local".into()])
        .map_err(|e| e.to_string())?;
    let mut host_token = vec![0u8; 32];
    let mut client_token = vec![0u8; 32];
    getrandom::fill(&mut host_token).map_err(|e| e.to_string())?;
    getrandom::fill(&mut client_token).map_err(|e| e.to_string())?;
    let config = RelayServerConfig {
        listen: address,
        certificate: identity.cert.der().to_vec(),
        key: identity.signing_key.serialize_der(),
        routes: vec![RelayRoute {
            id: "community".into(),
            host_token: host_token.clone(),
            client_token: client_token.clone(),
            max_tunnels: 32,
        }],
    };
    write_new(&directory.join("relay-server.json"), &config)?;
    let endpoint = RelayEndpoint {
        address,
        certificate: identity.cert.der().to_vec(),
        route: "community".into(),
        token: host_token,
    };
    write_new(&directory.join("relay-host.json"), &endpoint)?;
    write_new(
        &directory.join("relay-member.json"),
        &RelayEndpoint {
            token: client_token,
            ..endpoint
        },
    )?;
    println!(
        "Configuración creada. Protegé relay-server.json y relay-host.json; no los compartas con miembros."
    );
    Ok(())
}
#[tokio::main]
async fn main() {
    let args: Vec<_> = std::env::args().collect();
    let result = match args.get(1).map(String::as_str) {
        Some("init") if (3..=4).contains(&args.len()) => {
            let address = args
                .get(3)
                .map(String::as_str)
                .unwrap_or("127.0.0.1:45455")
                .parse::<SocketAddr>();
            match address {
                Ok(address) => initialize(Path::new(&args[2]), address),
                Err(_) => Err("Indicá IP numérica y puerto TCP válidos".into()),
            }
        }
        Some("serve") if args.len() == 3 => {
            async {
                let bytes = std::fs::read(&args[2]).map_err(|e| e.to_string())?;
                if bytes.len() > 2 * 1024 * 1024 {
                    return Err("Configuración demasiado grande".into());
                }
                let config: RelayServerConfig =
                    serde_json::from_slice(&bytes).map_err(|_| "Configuración relay inválida")?;
                let server = RelayServer::start(config).await?;
                println!(
                    "Relay TCP/TLS escuchando en {}. WAN sin verificar.",
                    server.address()
                );
                std::future::pending::<()>().await;
                Ok(())
            }
            .await
        }
        _ => Err(
            "Uso: nip-relay init <carpeta-nueva> [IP:puerto] | nip-relay serve <relay-server.json>"
                .into(),
        ),
    };
    if let Err(error) = result {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
