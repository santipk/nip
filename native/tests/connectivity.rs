#![cfg(target_os = "windows")]
use nip_native::{audio::AudioEngine, contracts::*, network, storage::Invitation};
use std::{
    sync::mpsc,
    time::{Duration, Instant},
};

#[test]
fn pending_attempt_can_be_cancelled_and_retried_without_losing_active_session() {
    let profile =
        std::env::temp_dir().join(format!("nip-connectivity-{}", getrandom::u64().unwrap()));
    let (events, received) = mpsc::channel();
    let (audio, frames) = AudioEngine::spawn_synthetic(events.clone(), Default::default()).unwrap();
    let commands = network::spawn_loopback(profile, events, audio, frames);
    let state = |predicate: &dyn Fn(&AppState) -> bool| {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match received
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .unwrap()
            {
                Event::State(state) if predicate(&state) => return state,
                Event::Error(error) => panic!("{error}"),
                _ => {}
            }
        }
    };
    let port = 0;
    commands
        .send(Command::Create {
            name: "QA".into(),
            user: "QA".into(),
            port,
            persistent: false,
        })
        .unwrap();
    let connected = state(&|s| s.room.is_some());
    let port = connected.hosts[0].port;
    let server_id = connected.room.as_ref().unwrap().id.clone();
    assert_eq!(
        connected.hosts[0].addresses,
        vec![format!("127.0.0.1:{port}")]
    );
    assert_eq!(
        connected.diagnostics.transport.as_deref(),
        Some("QUIC/TLS 1.3 · directo")
    );
    assert_eq!(
        connected.diagnostics.remote_endpoint.as_deref(),
        Some(format!("127.0.0.1:{port}").as_str())
    );
    commands
        .send(Command::Invite {
            id: Some(server_id.clone()),
            endpoint: format!("127.0.0.1:{port}"),
        })
        .unwrap();
    let invite = loop {
        if let Event::Invitation(value) = received.recv_timeout(Duration::from_secs(5)).unwrap() {
            break Invitation::parse(&value).unwrap();
        }
    };
    // A bound UDP socket never completes QUIC, unlike a closed port that can fail immediately.
    let blackhole = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let target = blackhole.local_addr().unwrap().to_string();
    let mut pending = invite;
    pending.address = target.clone();
    for _ in 0..3 {
        commands
            .send(Command::Connect {
                invitation: pending.encode().unwrap(),
                user: "QA".into(),
            })
            .unwrap();
        state(&|s| s.connecting && s.diagnostics.pending_endpoint.as_deref() == Some(&target));
        let started = Instant::now();
        commands.send(Command::CancelConnect).unwrap();
        let cancelled = state(&|s| !s.connecting && s.diagnostics.pending_endpoint.is_none());
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "cancel took {:?}",
            started.elapsed()
        );
        assert_eq!(cancelled.room.unwrap().id, server_id);
    }
    commands.send(Command::DiagnoseNetwork).unwrap();
    let diagnosed = state(&|s| s.diagnostics.rtt_ms.is_some());
    assert!(diagnosed.diagnostics.pending_endpoint.is_none());
    commands.send(Command::Disconnect).unwrap();
    let disconnected = state(&|s| s.room.is_none() && !s.connecting);
    assert!(disconnected.diagnostics.remote_endpoint.is_none());
    assert!(disconnected.diagnostics.transport.is_none());
    assert_eq!(disconnected.diagnostics.sent_packets, 0);
    let mut external = pending.clone();
    external.address = "192.0.2.1:45454".into();
    let rejected_at = Instant::now();
    commands
        .send(Command::Connect {
            invitation: external.encode().unwrap(),
            user: "QA".into(),
        })
        .unwrap();
    loop {
        if let Event::Error(error) = received.recv_timeout(Duration::from_secs(2)).unwrap() {
            assert!(error.contains("Modo QA local"));
            break;
        }
    }
    assert!(rejected_at.elapsed() < Duration::from_secs(1));
    let timeout_started = Instant::now();
    commands
        .send(Command::Connect {
            invitation: pending.encode().unwrap(),
            user: "QA".into(),
        })
        .unwrap();
    let timeout_deadline = timeout_started + Duration::from_secs(11);
    loop {
        if let Event::Error(error) = received
            .recv_timeout(timeout_deadline.saturating_duration_since(Instant::now()))
            .unwrap()
        {
            assert!(error.contains("8 s"), "{error}");
            break;
        }
    }
    assert!(
        timeout_started.elapsed() >= Duration::from_secs(7)
            && timeout_started.elapsed() < Duration::from_secs(11)
    );
    commands.send(Command::Shutdown).unwrap();
}
