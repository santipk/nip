#![cfg(target_os = "windows")]
use nip_native::{
    audio::{AudioEngine, AudioHandle},
    contracts::*,
    network,
};
use std::{
    path::PathBuf,
    sync::mpsc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

struct Client {
    commands: tokio::sync::mpsc::UnboundedSender<Command>,
    events: mpsc::Receiver<Event>,
    audio: AudioHandle,
}
impl Client {
    fn new(label: &str) -> Self {
        let (events, receive) = mpsc::channel();
        let (audio, frames) =
            AudioEngine::spawn_synthetic(events.clone(), Default::default()).unwrap();
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let profile: PathBuf = std::env::temp_dir().join(format!(
            "nip-native-e2e-{label}-{}-{suffix}",
            std::process::id()
        ));
        let commands = network::spawn(profile, events, audio.clone(), frames);
        Self {
            commands,
            events: receive,
            audio,
        }
    }
    fn send(&self, command: Command) {
        self.commands.send(command).unwrap();
    }
    fn event(&self, mut predicate: impl FnMut(&Event) -> bool) -> Event {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let event = self
                .events
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .expect("native event timeout");
            if let Event::Error(error) = &event {
                panic!("native runtime error: {error}");
            }
            if predicate(&event) {
                return event;
            }
        }
    }
}
impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Shutdown);
    }
}
fn wait(mut predicate: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(8);
    while !predicate() {
        assert!(
            Instant::now() < deadline,
            "native audio condition timed out"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}
fn signal(client: &Client) -> f32 {
    client.audio.synthetic_metrics().unwrap().remote_rms
}

#[test]
fn two_native_clients_chat_audio_private_monitor_and_restoration() {
    let host = Client::new("host");
    let friend = Client::new("friend");
    let reserve = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let port = reserve.local_addr().unwrap().port();
    drop(reserve);
    host.send(Command::Create {
        name: "Prueba nativa".into(),
        port,
        user: "Ana".into(),
    });
    host.event(|e| matches!(e, Event::State(s) if s.room.is_some()));
    host.send(Command::Invite {
        endpoint: format!("127.0.0.1:{port}"),
    });
    let Event::Invitation(invitation) = host.event(|e| matches!(e, Event::Invitation(_))) else {
        unreachable!()
    };
    friend.send(Command::Connect {
        invitation: invitation.clone(),
        user: "Bruno".into(),
    });
    friend.event(
        |e| matches!(e, Event::State(s) if s.room.as_ref().is_some_and(|r| r.members.len() == 2)),
    );
    host.send(Command::Chat(
        "Texto literal <img src=x onerror=alert(1)> ñ".into(),
    ));
    let Event::Chat(message) = friend.event(|e| matches!(e, Event::Chat(_))) else {
        unreachable!()
    };
    assert_eq!(message.sender_name, "Ana");
    assert_eq!(message.text, "Texto literal <img src=x onerror=alert(1)> ñ");
    assert!(!host.audio.status().calling && !friend.audio.status().calling);
    assert!(!host.audio.synthetic_metrics().unwrap().active_capture);

    host.send(Command::Join);
    friend.send(Command::Join);
    wait(|| signal(&host) > 0.03 && signal(&friend) > 0.03);
    host.send(Command::StartTest);
    wait(|| {
        host.audio.status().testing && host.audio.synthetic_metrics().unwrap().monitor_rms > 0.1
    });
    assert!(!host.audio.outgoing_allowed());
    wait(|| signal(&friend) == 0.0);
    let before = friend.audio.synthetic_metrics().unwrap().remote_frames;
    std::thread::sleep(Duration::from_millis(160));
    assert_eq!(
        friend.audio.synthetic_metrics().unwrap().remote_frames,
        before,
        "private monitor must not reach remote decoder"
    );
    host.send(Command::Mute);
    std::thread::sleep(Duration::from_millis(30));
    assert!(
        !host.audio.outgoing_allowed(),
        "mute toggle cannot reopen publication during private test"
    );
    // A peer leaving/rejoining must not bypass the monitor gate.
    friend.send(Command::Leave);
    wait(|| !friend.audio.status().calling);
    friend.send(Command::Join);
    wait(|| friend.audio.status().calling);
    std::thread::sleep(Duration::from_millis(250));
    assert_eq!(signal(&friend), 0.0);
    host.send(Command::StopTest);
    wait(|| !host.audio.status().testing);
    assert!(!host.audio.status().muted, "restore previous unmuted state");
    wait(|| signal(&friend) > 0.03);
    host.send(Command::Mute);
    wait(|| host.audio.status().muted);
    host.send(Command::StartTest);
    wait(|| host.audio.status().testing);
    host.send(Command::StopTest);
    wait(|| !host.audio.status().testing);
    assert!(host.audio.status().muted, "previously muted stays muted");
    wait(|| signal(&friend) == 0.0);
    host.send(Command::Mute);
    wait(|| !host.audio.status().muted);
    wait(|| signal(&friend) > 0.03);

    friend.send(Command::Deafen);
    wait(|| friend.audio.status().deafened);
    assert!(!friend.audio.outgoing_allowed());
    wait(|| signal(&friend) == 0.0 && signal(&host) == 0.0);
    friend.send(Command::Deafen);
    wait(|| !friend.audio.status().deafened);
    wait(|| signal(&friend) > 0.03 && signal(&host) > 0.03);
    friend.send(Command::Mute);
    wait(|| friend.audio.status().muted);
    friend.send(Command::Deafen);
    wait(|| friend.audio.status().deafened);
    friend.send(Command::Deafen);
    wait(|| !friend.audio.status().deafened);
    assert!(friend.audio.status().muted, "deafen preserves earlier mute");
    friend.send(Command::Mute);
    wait(|| !friend.audio.status().muted);

    let mut settings = host.audio.status().settings;
    settings.volume = 0.0;
    host.send(Command::AudioSettings(settings.clone()));
    wait(|| signal(&host) == 0.0);
    wait(|| signal(&friend) > 0.03); // Listening volume must not change outgoing mic level.
    settings.volume = 1.0;
    settings.input_level = 0.0;
    host.send(Command::AudioSettings(settings.clone()));
    wait(|| signal(&friend) < 0.001);
    settings.input_level = 1.0;
    host.send(Command::AudioSettings(settings));
    wait(|| signal(&friend) > 0.03);

    // A real QUIC handshake to a non-answering socket must not block privacy commands.
    let blackhole = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let mut unreachable = nip_native::storage::Invitation::parse(&invitation).unwrap();
    unreachable.address = blackhole.local_addr().unwrap().to_string();
    friend.send(Command::Connect {
        invitation: unreachable.encode().unwrap(),
        user: "Bruno".into(),
    });
    friend.send(Command::Mute);
    let deadline = Instant::now() + Duration::from_millis(500);
    while !friend.audio.status().muted {
        assert!(
            Instant::now() < deadline,
            "mute blocked by pending connection"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(!friend.audio.outgoing_allowed());
    friend.send(Command::Deafen);
    let deadline = Instant::now() + Duration::from_millis(500);
    while !friend.audio.status().deafened {
        assert!(
            Instant::now() < deadline,
            "deafen blocked by pending connection"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    friend.send(Command::Deafen);
    wait(|| !friend.audio.status().deafened);
    friend.send(Command::Disconnect);
    friend.event(|e| matches!(e, Event::State(s) if s.room.is_none()));
    friend.send(Command::StartTest);
    wait(|| friend.audio.synthetic_metrics().unwrap().monitor_rms > 0.1);
    assert!(!friend.audio.outgoing_allowed());
    friend.send(Command::StopTest);
    wait(|| !friend.audio.synthetic_metrics().unwrap().active_capture);
    host.send(Command::StopHost);
    host.event(|e| matches!(e, Event::State(s) if s.host.is_none() && s.room.is_none()));
    wait(|| !host.audio.synthetic_metrics().unwrap().active_capture);
}
