use nip_native::relay::*;
use std::{sync::Arc, time::Duration};

#[tokio::test]
async fn host_pool_recovers_after_relay_restart_without_reusing_a_cancelled_client() {
    let identity = rcgen::generate_simple_self_signed(vec!["nip-relay.local".into()]).unwrap();
    let mut config = RelayServerConfig {
        listen: "127.0.0.1:0".parse().unwrap(),
        certificate: identity.cert.der().to_vec(),
        key: identity.signing_key.serialize_der(),
        routes: vec![RelayRoute {
            id: "qa".into(),
            host_token: vec![1; 32],
            client_token: vec![2; 32],
            max_tunnels: 2,
        }],
    };
    let relay = RelayServer::start(config.clone()).await.unwrap();
    config.listen = relay.address();
    let target = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let target_address = target.local_addr().unwrap();
    let echo = tokio::spawn(async move {
        let mut data = [0u8; 2048];
        loop {
            let (len, peer) = target.recv_from(&mut data).await.unwrap();
            target.send_to(&data[..len], peer).await.unwrap();
        }
    });
    let endpoint = RelayEndpoint {
        address: relay.address(),
        certificate: identity.cert.der().to_vec(),
        route: "qa".into(),
        token: vec![1; 32],
    };
    let _host = RelayHost::start(endpoint.clone(), target_address, 2)
        .await
        .unwrap();
    let member = RelayEndpoint {
        token: vec![2; 32],
        ..endpoint
    };
    let first = RelayClient::connect(&member).await.unwrap();
    let local = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    local.send_to(b"before", first.address()).await.unwrap();
    let mut data = [0u8; 64];
    let (len, _) = tokio::time::timeout(Duration::from_secs(2), local.recv_from(&mut data))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&data[..len], b"before");
    drop(first);
    drop(relay);
    tokio::time::sleep(Duration::from_millis(50)).await;
    let _restarted = RelayServer::start(config).await.unwrap();
    let second = RelayClient::connect(&member).await.unwrap();
    local.send_to(b"after", second.address()).await.unwrap();
    let (len, _) = tokio::time::timeout(Duration::from_secs(2), local.recv_from(&mut data))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&data[..len], b"after");
    echo.abort();
}

#[tokio::test]
async fn relay_does_not_replace_host_certificate_validation() {
    let identity = rcgen::generate_simple_self_signed(vec!["nip-relay.local".into()]).unwrap();
    let relay = RelayServer::start(RelayServerConfig {
        listen: "127.0.0.1:0".parse().unwrap(),
        certificate: identity.cert.der().to_vec(),
        key: identity.signing_key.serialize_der(),
        routes: vec![RelayRoute {
            id: "qa".into(),
            host_token: vec![1; 32],
            client_token: vec![2; 32],
            max_tunnels: 1,
        }],
    })
    .await
    .unwrap();
    let host_identity = rcgen::generate_simple_self_signed(vec!["nip.local".into()]).unwrap();
    let config = quinn::ServerConfig::with_single_cert(
        vec![host_identity.cert.der().clone()],
        rustls::pki_types::PrivatePkcs8KeyDer::from(host_identity.signing_key.serialize_der())
            .into(),
    )
    .unwrap();
    let server = quinn::Endpoint::server(config, "127.0.0.1:0".parse().unwrap()).unwrap();
    let endpoint = RelayEndpoint {
        address: relay.address(),
        certificate: identity.cert.der().to_vec(),
        route: "qa".into(),
        token: vec![1; 32],
    };
    let _host = RelayHost::start(endpoint.clone(), server.local_addr().unwrap(), 1)
        .await
        .unwrap();
    let member = RelayEndpoint {
        token: vec![2; 32],
        ..endpoint
    };
    let tunnel = RelayClient::connect(&member).await.unwrap();
    let accept = tokio::spawn(async move {
        if let Some(incoming) = server.accept().await {
            let _ = incoming.await;
        }
    });
    let other_identity = rcgen::generate_simple_self_signed(vec!["nip.local".into()]).unwrap();
    let mut roots = rustls::RootCertStore::empty();
    roots.add(other_identity.cert.der().clone()).unwrap();
    let mut client = quinn::Endpoint::client("127.0.0.1:0".parse().unwrap()).unwrap();
    client.set_default_client_config(
        quinn::ClientConfig::with_root_certificates(Arc::new(roots)).unwrap(),
    );
    let result = tokio::time::timeout(
        Duration::from_secs(5),
        client.connect(tunnel.address(), "nip.local").unwrap(),
    )
    .await
    .unwrap();
    assert!(
        result.is_err(),
        "valid relay authentication must not authorize the wrong host certificate"
    );
    accept.abort();
}

#[tokio::test]
async fn relay_rejects_wrong_role_credentials_identity_and_excess_clients() {
    let identity = rcgen::generate_simple_self_signed(vec!["nip-relay.local".into()]).unwrap();
    let relay = RelayServer::start(RelayServerConfig {
        listen: "127.0.0.1:0".parse().unwrap(),
        certificate: identity.cert.der().to_vec(),
        key: identity.signing_key.serialize_der(),
        routes: vec![RelayRoute {
            id: "qa".into(),
            host_token: vec![1; 32],
            client_token: vec![2; 32],
            max_tunnels: 1,
        }],
    })
    .await
    .unwrap();
    let endpoint = RelayEndpoint {
        address: relay.address(),
        certificate: identity.cert.der().to_vec(),
        route: "qa".into(),
        token: vec![1; 32],
    };
    assert!(
        RelayClient::connect(&endpoint).await.is_err(),
        "host secret must not authenticate as member"
    );
    let socket = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let _host = RelayHost::start(endpoint.clone(), socket.local_addr().unwrap(), 1)
        .await
        .unwrap();
    let mut member = endpoint.clone();
    member.token = vec![2; 32];
    let _client = RelayClient::connect(&member).await.unwrap();
    assert!(
        RelayClient::connect(&member).await.is_err(),
        "route max_tunnels must bound active member tunnels"
    );
    let other = rcgen::generate_simple_self_signed(vec!["nip-relay.local".into()]).unwrap();
    member.certificate = other.cert.der().to_vec();
    assert!(
        RelayClient::connect(&member).await.is_err(),
        "wrong relay identity must fail"
    );
    member.certificate = identity.cert.der().to_vec();
    assert!(
        RelayHost::start(member, socket.local_addr().unwrap(), 1)
            .await
            .is_err(),
        "member secret must not register a host"
    );
}

#[tokio::test]
async fn two_clients_use_outbound_tls_tunnels_while_quic_still_checks_host_identity() {
    let relay_identity =
        rcgen::generate_simple_self_signed(vec!["nip-relay.local".into()]).unwrap();
    let relay = RelayServer::start(RelayServerConfig {
        listen: "127.0.0.1:0".parse().unwrap(),
        certificate: relay_identity.cert.der().to_vec(),
        key: relay_identity.signing_key.serialize_der(),
        routes: vec![RelayRoute {
            id: "qa".into(),
            host_token: vec![1; 32],
            client_token: vec![2; 32],
            max_tunnels: 2,
        }],
    })
    .await
    .unwrap();
    let host_identity = rcgen::generate_simple_self_signed(vec!["nip.local".into()]).unwrap();
    let config = quinn::ServerConfig::with_single_cert(
        vec![host_identity.cert.der().clone()],
        rustls::pki_types::PrivatePkcs8KeyDer::from(host_identity.signing_key.serialize_der())
            .into(),
    )
    .unwrap();
    let server = quinn::Endpoint::server(config, "127.0.0.1:0".parse().unwrap()).unwrap();
    let endpoint = RelayEndpoint {
        address: relay.address(),
        certificate: relay_identity.cert.der().to_vec(),
        route: "qa".into(),
        token: vec![1; 32],
    };
    let _host = RelayHost::start(endpoint.clone(), server.local_addr().unwrap(), 2)
        .await
        .unwrap();
    let mut member = endpoint;
    member.token = vec![2; 32];
    let host_task = tokio::spawn(async move {
        let mut sessions = tokio::task::JoinSet::new();
        for _ in 0..2 {
            let incoming = server.accept().await.unwrap();
            sessions.spawn(async move {
                let connection = incoming.await.unwrap();
                let (mut send, mut recv) = connection.accept_bi().await.unwrap();
                let bytes = recv.read_to_end(64).await.unwrap();
                send.write_all(&bytes).await.unwrap();
                send.finish().unwrap();
                let _ = send.stopped().await;
            });
        }
        while let Some(result) = sessions.join_next().await {
            result.unwrap();
        }
        server.close(0u32.into(), b"done");
    });
    let mut clients = tokio::task::JoinSet::new();
    for message in [b"alpha".as_slice(), b"beta".as_slice()] {
        let member = member.clone();
        let certificate = host_identity.cert.der().clone();
        clients.spawn(async move {
            let tunnel = RelayClient::connect(&member).await.unwrap();
            let mut roots = rustls::RootCertStore::empty();
            roots.add(certificate).unwrap();
            let mut client = quinn::Endpoint::client("127.0.0.1:0".parse().unwrap()).unwrap();
            client.set_default_client_config(
                quinn::ClientConfig::with_root_certificates(Arc::new(roots)).unwrap(),
            );
            let connection = tokio::time::timeout(
                Duration::from_secs(5),
                client.connect(tunnel.address(), "nip.local").unwrap(),
            )
            .await
            .unwrap()
            .unwrap();
            let (mut send, mut recv) = connection.open_bi().await.unwrap();
            send.write_all(message).await.unwrap();
            send.finish().unwrap();
            let echoed = tokio::time::timeout(Duration::from_secs(5), recv.read_to_end(64))
                .await
                .unwrap()
                .unwrap();
            assert_eq!(echoed, message);
            connection.close(0u32.into(), b"done");
            client.wait_idle().await;
        });
    }
    while let Some(result) = clients.join_next().await {
        result.unwrap();
    }
    tokio::time::timeout(Duration::from_secs(5), host_task)
        .await
        .unwrap()
        .unwrap();
}
