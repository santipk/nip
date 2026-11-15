#![cfg(target_os = "windows")]
use nip_native::{
    audio::{AudioEngine, AudioHandle},
    contracts::*,
    network,
    relay::*,
    storage::Invitation,
};
use std::{
    sync::mpsc,
    time::{Duration, Instant},
};

struct App {
    commands: tokio::sync::mpsc::UnboundedSender<Command>,
    events: mpsc::Receiver<Event>,
    audio: AudioHandle,
}
impl App {
    fn new() -> Self {
        let profile =
            std::env::temp_dir().join(format!("nip-relay-app-{}", getrandom::u64().unwrap()));
        let (events, received) = mpsc::channel();
        let (audio, frames) =
            AudioEngine::spawn_synthetic(events.clone(), Default::default()).unwrap();
        Self {
            commands: network::spawn_loopback(profile, events, audio.clone(), frames),
            events: received,
            audio,
        }
    }
    fn send(&self, command: Command) {
        self.commands.send(command).unwrap();
    }
    fn event(&self, predicate: impl Fn(&Event) -> bool) -> Event {
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            let event = self
                .events
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .unwrap();
            if let Event::Error(error) = &event {
                panic!("{error}");
            }
            if predicate(&event) {
                return event;
            }
        }
    }
    fn state(&self, predicate: impl Fn(&AppState) -> bool) -> AppState {
        let Event::State(state) =
            self.event(|event| matches!(event, Event::State(state) if predicate(state)))
        else {
            unreachable!()
        };
        state
    }
}
impl Drop for App {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Shutdown);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn app_relay_invitation_connects_two_members_and_cancel_preserves_active_voice() {
    let identity = rcgen::generate_simple_self_signed(vec!["nip-relay.local".into()]).unwrap();
    let relay = RelayServer::start(RelayServerConfig {
        listen: "127.0.0.1:0".parse().unwrap(),
        certificate: identity.cert.der().to_vec(),
        key: identity.signing_key.serialize_der(),
        routes: vec![RelayRoute {
            id: "qa".into(),
            host_token: vec![1; 32],
            client_token: vec![2; 32],
            max_tunnels: 4,
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
    let member_endpoint = RelayEndpoint {
        token: vec![2; 32],
        ..endpoint.clone()
    };
    let host = App::new();
    let a = App::new();
    let b = App::new();
    let port = 0;
    host.send(Command::Create {
        persistent: true,
        name: "Relay QA".into(),
        user: "Host".into(),
        port,
    });
    let connected = host.state(|s| s.room.is_some());
    let port = connected.hosts[0].port;
    let id = connected.room.unwrap().id;
    host.send(Command::StopServer { id: id.clone() });
    host.state(|s| s.hosts.is_empty());
    host.send(Command::ConfigureRelay {
        server_id: id.clone(),
        configuration: Some(RelayConfiguration {
            host: endpoint,
            client: member_endpoint.clone(),
            tunnels: 4,
        }),
    });
    host.event(|e| matches!(e, Event::Notice(text) if text.contains("relay guardada")));
    host.send(Command::StartServer {
        id: id.clone(),
        user: "Host".into(),
    });
    host.state(|s| s.room.is_some());
    host.send(Command::Invite {
        id: Some(id.clone()),
        endpoint: format!("127.0.0.1:{port}"),
    });
    let Event::Invitation(encoded) = host.event(|e| matches!(e, Event::Invitation(_))) else {
        unreachable!()
    };
    let invite = Invitation::parse(&encoded).unwrap();
    assert_eq!(
        invite.relay.as_ref().unwrap().token,
        vec![2; 32],
        "invitation must only contain the member credential"
    );
    for app in [&a, &b] {
        app.send(Command::Connect {
            invitation: encoded.clone(),
            user: "Member".into(),
        });
        let connected = app.state(|s| s.room.is_some());
        assert_eq!(
            connected.diagnostics.transport.as_deref(),
            Some("QUIC/TLS 1.3 · relay TLS/TCP")
        );
        assert_eq!(
            connected.diagnostics.remote_endpoint.as_deref(),
            Some(relay.address().to_string().as_str())
        );
        assert!(
            !app.audio.status().calling,
            "connecting chat must not start capture"
        );
    }
    a.send(Command::Chat("Mensaje mediante relay".into()));
    b.state(|s| {
        s.room
            .as_ref()
            .is_some_and(|r| r.history.iter().any(|m| m.text == "Mensaje mediante relay"))
    });
    for app in [&a, &b] {
        app.send(Command::JoinVoice {
            channel_id: GENERAL_VOICE_CHANNEL.into(),
        });
    }
    a.state(|s| {
        s.room
            .as_ref()
            .is_some_and(|r| r.members.iter().filter(|m| m.calling).count() == 2)
    });
    let deadline = Instant::now() + Duration::from_secs(3);
    while b.audio.synthetic_metrics().unwrap().remote_rms < 0.01 {
        assert!(Instant::now() < deadline, "relay synthetic audio timeout");
        std::thread::sleep(Duration::from_millis(10));
    }
    a.send(Command::Mute);
    let deadline = Instant::now() + Duration::from_secs(2);
    while !a.audio.status().muted {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    let blackhole = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let mut pending = invite;
    pending.relay.as_mut().unwrap().address = blackhole.local_addr().unwrap();
    a.send(Command::Connect {
        invitation: pending.encode().unwrap(),
        user: "Member".into(),
    });
    a.state(|s| s.connecting);
    let cancellation = Instant::now();
    a.send(Command::CancelConnect);
    let retained = a.state(|s| !s.connecting);
    assert!(cancellation.elapsed() < Duration::from_secs(1));
    assert_eq!(retained.room.unwrap().id, id);
    assert!(
        a.audio.status().calling && a.audio.status().muted,
        "cancel must preserve current voice privacy state"
    );
    let directory =
        std::env::temp_dir().join(format!("nip-relay-file-{}", getrandom::u64().unwrap()));
    std::fs::create_dir(&directory).unwrap();
    let bytes: Vec<u8> = (0..4 * 1024 * 1024).map(|n| (n % 251) as u8).collect();
    let source = directory.join("relay.bin");
    std::fs::write(&source, &bytes).unwrap();
    let operation_id = format!("{:032x}", getrandom::u64().unwrap());
    let upload = || Command::UploadFile {
        server_id: id.clone(),
        channel_id: GENERAL_TEXT_CHANNEL.into(),
        path: source.clone(),
        operation_id: operation_id.clone(),
        text: "Adjunto por relay".into(),
    };
    let baseline_start = Instant::now();
    let baseline_before = a.audio.synthetic_metrics().unwrap().remote_frames;
    std::thread::sleep(Duration::from_millis(400));
    let baseline_frames = a.audio.synthetic_metrics().unwrap().remote_frames - baseline_before;
    let baseline_elapsed = baseline_start.elapsed();
    let loaded_start = Instant::now();
    let loaded_before = a.audio.synthetic_metrics().unwrap().remote_frames;
    a.send(upload());
    a.event(|e| matches!(e,Event::Transfer(t) if t.operation_id == operation_id && t.finished && t.error.is_none()));
    let loaded_frames = a.audio.synthetic_metrics().unwrap().remote_frames - loaded_before;
    let loaded_elapsed = loaded_start.elapsed();
    eprintln!(
        "relay voice baseline {baseline_elapsed:?}: {baseline_frames} frames; 4MiB transfer {loaded_elapsed:?}: {loaded_frames} frames"
    );
    assert!(baseline_frames >= 10 && loaded_frames > 0);
    assert!(loaded_frames as f64 / loaded_elapsed.as_secs_f64() >= 20.0);
    let message = b
        .state(|s| {
            s.room
                .as_ref()
                .is_some_and(|r| r.history.iter().any(|m| !m.attachments.is_empty()))
        })
        .room
        .unwrap()
        .history
        .into_iter()
        .find(|m| !m.attachments.is_empty())
        .unwrap();
    a.send(upload());
    a.event(|e| matches!(e,Event::Transfer(t) if t.operation_id == operation_id && t.finished && t.error.is_none()));
    for app in [&a, &b] {
        app.send(Command::Preferences(Preferences {
            auto_reconnect: false,
            ..Default::default()
        }));
        app.state(|s| !s.preferences.auto_reconnect);
    }
    host.send(Command::StopServer { id: id.clone() });
    host.state(|s| s.hosts.is_empty());
    a.state(|s| s.room.is_none());
    host.send(Command::StartServer {
        id: id.clone(),
        user: "Host".into(),
    });
    host.state(|s| s.room.is_some());
    a.send(Command::Reconnect {
        id: id.clone(),
        user: "Member".into(),
    });
    let connected = a.state(|s| s.room.is_some());
    assert_eq!(
        connected
            .room
            .unwrap()
            .history
            .iter()
            .filter(|m| !m.attachments.is_empty())
            .count(),
        1
    );
    let download_id = format!("{:032x}", getrandom::u64().unwrap());
    let destination = directory.join("download.bin");
    a.send(Command::DownloadFile {
        server_id: id,
        channel_id: GENERAL_TEXT_CHANNEL.into(),
        attachment_id: message.attachments[0].id.clone(),
        destination: destination.clone(),
        operation_id: download_id.clone(),
    });
    a.event(|e| matches!(e,Event::Transfer(t) if t.operation_id == download_id && t.finished && t.error.is_none()));
    assert_eq!(std::fs::read(destination).unwrap(), bytes);
    std::fs::remove_dir_all(directory).unwrap();
}
