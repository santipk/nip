#![cfg(windows)]
use nip_native::{audio::AudioEngine, contracts::*, network, storage::Invitation};
use std::{
    sync::{Arc, mpsc},
    time::Duration,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn previous_client_authentication_shape_receives_explicit_update_error() {
    let profile =
        std::env::temp_dir().join(format!("nip-old-protocol-{}", getrandom::u64().unwrap()));
    let (events, received) = mpsc::channel();
    let (audio, frames) = AudioEngine::spawn_synthetic(events.clone(), Default::default()).unwrap();
    let commands = network::spawn_loopback(profile, events, audio, frames);
    let port = 0;
    commands
        .send(Command::Create {
            name: "QA compatibilidad".into(),
            user: "Host".into(),
            port,
            persistent: false,
        })
        .unwrap();
    let (id, port) = loop {
        match received.recv_timeout(Duration::from_secs(10)).unwrap() {
            Event::State(state) if state.room.is_some() => {
                break (state.room.unwrap().id, state.hosts[0].port);
            }
            Event::Error(error) => panic!("{error}"),
            _ => {}
        }
    };
    commands
        .send(Command::Invite {
            id: Some(id),
            endpoint: format!("127.0.0.1:{port}"),
        })
        .unwrap();
    let invite = loop {
        if let Event::Invitation(encoded) = received.recv_timeout(Duration::from_secs(10)).unwrap()
        {
            break Invitation::parse(&encoded).unwrap();
        }
    };
    let mut roots = rustls::RootCertStore::empty();
    roots
        .add(rustls::pki_types::CertificateDer::from(invite.certificate))
        .unwrap();
    let mut endpoint = quinn::Endpoint::client("127.0.0.1:0".parse().unwrap()).unwrap();
    endpoint.set_default_client_config(
        quinn::ClientConfig::with_root_certificates(Arc::new(roots)).unwrap(),
    );
    let connection = endpoint
        .connect(invite.address.parse().unwrap(), "nip.local")
        .unwrap()
        .await
        .unwrap();
    let (mut send, mut recv) = connection.open_bi().await.unwrap();
    // Historical wire fixture deliberately has no protocol version field.
    let payload = serde_json::to_vec(
        &serde_json::json!({"Authenticate":{"token":invite.token,"name":"Old QA"}}),
    )
    .unwrap();
    send.write_u32(payload.len() as u32).await.unwrap();
    send.write_all(&payload).await.unwrap();
    let length = tokio::time::timeout(Duration::from_secs(3), recv.read_u32())
        .await
        .unwrap()
        .unwrap() as usize;
    assert!(length <= 65536);
    let mut bytes = vec![0u8; length];
    recv.read_exact(&mut bytes).await.unwrap();
    let response: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let error = response["Error"].as_str().unwrap();
    assert!(error.contains("protocolo incompatible") && error.contains("Actualizá"));
    connection.close(0u32.into(), b"QA complete");
    endpoint.close(0u32.into(), b"QA complete");
    commands.send(Command::Shutdown).unwrap();
}
