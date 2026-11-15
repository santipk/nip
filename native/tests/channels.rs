#![cfg(target_os = "windows")]
use nip_native::{
    audio::{AudioEngine, AudioHandle},
    contracts::*,
    network,
};
use std::{
    sync::mpsc,
    time::{Duration, Instant},
};

struct Client {
    commands: tokio::sync::mpsc::UnboundedSender<Command>,
    events: mpsc::Receiver<Event>,
    audio: AudioHandle,
}
impl Client {
    fn new() -> Self {
        let profile =
            std::env::temp_dir().join(format!("nip-channels-{}", getrandom::u64().unwrap()));
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
        let deadline = Instant::now() + Duration::from_secs(15);
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
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match self
                .events
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .unwrap()
            {
                Event::State(state) if predicate(&state) => return state,
                Event::Error(error) => panic!("{error}"),
                _ => {}
            }
        }
    }
}

#[test]
fn attachment_is_idempotent_downloads_after_restart_and_does_not_stop_synthetic_voice() {
    let host = Client::new();
    let guest = Client::new();
    let port = 0;
    host.send(Command::Create {
        name: "Archivos QA".into(),
        user: "Host".into(),
        port,
        persistent: true,
    });
    let connected = host.state(|s| s.room.is_some());
    let port = connected.hosts[0].port;
    let id = connected.room.unwrap().id;
    host.send(Command::Invite {
        id: Some(id.clone()),
        endpoint: format!("127.0.0.1:{port}"),
    });
    let Event::Invitation(invitation) = host.event(|e| matches!(e, Event::Invitation(_))) else {
        unreachable!()
    };
    guest.send(Command::Connect {
        invitation,
        user: "Miembro".into(),
    });
    guest.state(|s| s.room.is_some());
    for client in [&host, &guest] {
        client.send(Command::JoinVoice {
            channel_id: GENERAL_VOICE_CHANNEL.into(),
        });
    }
    wait(|| guest.audio.synthetic_metrics().unwrap().remote_rms > 0.01);
    let baseline = guest.audio.synthetic_metrics().unwrap().remote_frames;
    let baseline_start = Instant::now();
    std::thread::sleep(Duration::from_millis(400));
    let baseline_frames = guest.audio.synthetic_metrics().unwrap().remote_frames - baseline;
    let baseline_elapsed = baseline_start.elapsed();
    assert!(baseline_frames >= 10);
    let root = std::env::temp_dir().join(format!("nip-transfer-{}", getrandom::u64().unwrap()));
    std::fs::create_dir(&root).unwrap();
    let bytes: Vec<u8> = (0..4 * 1024 * 1024).map(|n| (n % 251) as u8).collect();
    let source = root.join("original.bin");
    std::fs::write(&source, &bytes).unwrap();
    let operation_id = format!("{:032x}", getrandom::u64().unwrap());
    let upload = || Command::UploadFile {
        server_id: id.clone(),
        channel_id: GENERAL_TEXT_CHANNEL.into(),
        path: source.clone(),
        operation_id: operation_id.clone(),
        text: "Archivo original".into(),
    };
    let voice_before = guest.audio.synthetic_metrics().unwrap().remote_frames;
    let started = Instant::now();
    guest.send(upload());
    let mut last_frame = voice_before;
    let mut last_advance = Instant::now();
    let mut largest_gap = Duration::ZERO;
    loop {
        match guest.events.recv_timeout(Duration::from_millis(20)) {
            Ok(Event::Transfer(status))
                if status.operation_id == operation_id && status.finished =>
            {
                assert!(status.error.is_none(), "{:?}", status.error);
                break;
            }
            Ok(Event::Error(error)) => panic!("{error}"),
            _ => {}
        }
        let frame = guest.audio.synthetic_metrics().unwrap().remote_frames;
        if frame != last_frame {
            largest_gap = largest_gap.max(last_advance.elapsed());
            last_advance = Instant::now();
            last_frame = frame;
        }
        assert!(
            started.elapsed() < Duration::from_secs(15),
            "transfer timeout"
        );
    }
    let during = guest.audio.synthetic_metrics().unwrap().remote_frames - voice_before;
    eprintln!(
        "voice baseline {:?}: {baseline_frames} frames; transfer {:?}: {during} frames, max observed gap {largest_gap:?}",
        baseline_elapsed,
        started.elapsed()
    );
    assert!(during > 0);
    assert!(largest_gap < Duration::from_millis(500));
    let message = host
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
    guest.send(upload());
    guest.event(|e| matches!(e,Event::Transfer(t) if t.operation_id == operation_id && t.finished && t.error.is_none()));
    host.send(Command::DiagnoseNetwork);
    assert_eq!(
        host.state(|s| s.diagnostics.rtt_ms.is_some())
            .room
            .unwrap()
            .history
            .iter()
            .filter(|m| !m.attachments.is_empty())
            .count(),
        1
    );
    host.send(Command::StopServer { id: id.clone() });
    host.state(|s| s.hosts.is_empty());
    guest.state(|s| s.room.is_none());
    host.send(Command::StartServer {
        id: id.clone(),
        user: "Host".into(),
    });
    host.state(|s| s.room.is_some());
    guest.send(Command::Reconnect {
        id: id.clone(),
        user: "Miembro".into(),
    });
    guest.state(|s| s.room.is_some());
    assert!(!guest.audio.status().calling);
    let download_id = format!("{:032x}", getrandom::u64().unwrap());
    let destination = root.join("download.bin");
    guest.send(Command::DownloadFile {
        server_id: id,
        channel_id: GENERAL_TEXT_CHANNEL.into(),
        attachment_id: message.attachments[0].id.clone(),
        destination: destination.clone(),
        operation_id: download_id.clone(),
    });
    guest.event(|e| matches!(e,Event::Transfer(t) if t.operation_id == download_id && t.finished && t.error.is_none()));
    assert_eq!(std::fs::read(destination).unwrap(), bytes);
    std::fs::remove_dir_all(root).unwrap();
}

fn wait(predicate: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !predicate() {
        assert!(Instant::now() < deadline, "audio timeout");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn members_cannot_administer_and_voice_changes_preserve_mute_without_cross_room_audio() {
    let host = Client::new();
    let guest = Client::new();
    let other = Client::new();
    let port = 0;
    host.send(Command::Create {
        name: "QA voz".into(),
        user: "Admin".into(),
        port,
        persistent: false,
    });
    let connected = host.state(|s| s.room.is_some());
    let port = connected.hosts[0].port;
    let id = connected.room.unwrap().id;
    host.send(Command::CreateChannel {
        server_id: id.clone(),
        name: "Privada".into(),
        kind: ChannelKind::Voice,
    });
    let room = host
        .state(|s| s.room.as_ref().is_some_and(|r| r.channels.len() == 3))
        .room
        .unwrap();
    let private = room
        .channels
        .iter()
        .find(|c| c.name == "Privada")
        .unwrap()
        .id
        .clone();
    host.send(Command::Invite {
        id: Some(id.clone()),
        endpoint: format!("127.0.0.1:{port}"),
    });
    let invitation = loop {
        if let Event::Invitation(invitation) =
            host.events.recv_timeout(Duration::from_secs(5)).unwrap()
        {
            break invitation;
        }
    };
    for client in [&guest, &other] {
        client.send(Command::Connect {
            invitation: invitation.clone(),
            user: "Admin".into(),
        });
        client.state(|s| s.room.is_some());
    }
    guest.send(Command::CreateChannel {
        server_id: id,
        name: "Prohibido".into(),
        kind: ChannelKind::Text,
    });
    loop {
        if let Event::Error(error) = guest.events.recv_timeout(Duration::from_secs(5)).unwrap() {
            assert!(error.contains("administrador local"));
            break;
        }
    }
    for client in [&host, &guest] {
        client.send(Command::JoinVoice {
            channel_id: GENERAL_VOICE_CHANNEL.into(),
        });
    }
    other.send(Command::JoinVoice {
        channel_id: private.clone(),
    });
    wait(|| {
        host.audio.status().calling && guest.audio.status().calling && other.audio.status().calling
    });
    wait(|| guest.audio.synthetic_metrics().unwrap().remote_rms > 0.01);
    std::thread::sleep(Duration::from_millis(150));
    assert_eq!(other.audio.synthetic_metrics().unwrap().remote_rms, 0.0);
    guest.send(Command::Mute);
    wait(|| guest.audio.status().muted);
    guest.send(Command::JoinVoice {
        channel_id: private.clone(),
    });
    guest.state(|s| {
        s.room.as_ref().is_some_and(|r| {
            r.members
                .iter()
                .any(|m| m.id == r.self_id && m.voice_channel.as_ref() == Some(&private))
        })
    });
    assert!(guest.audio.status().muted);
    wait(|| guest.audio.synthetic_metrics().unwrap().remote_rms > 0.01);
    assert!(!guest.audio.outgoing_allowed());
    guest.send(Command::Mute);
    wait(|| other.audio.synthetic_metrics().unwrap().remote_rms > 0.01);
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(host.audio.synthetic_metrics().unwrap().remote_rms, 0.0);
}
impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Shutdown);
    }
}

#[test]
fn cancelling_upload_releases_server_reservation_and_retry_uses_one_message() {
    let host = Client::new();
    let guest = Client::new();
    let port = 0;
    host.send(Command::Create {
        name: "Cancelar QA".into(),
        user: "Host".into(),
        port,
        persistent: true,
    });
    let connected = host.state(|s| s.room.is_some());
    let port = connected.hosts[0].port;
    let id = connected.room.unwrap().id;
    host.send(Command::Invite {
        id: Some(id.clone()),
        endpoint: format!("127.0.0.1:{port}"),
    });
    let Event::Invitation(invitation) = host.event(|e| matches!(e, Event::Invitation(_))) else {
        unreachable!()
    };
    guest.send(Command::Connect {
        invitation,
        user: "Miembro".into(),
    });
    guest.state(|s| s.room.is_some());
    let path = std::env::temp_dir().join(format!("nip-cancel-{}.bin", getrandom::u64().unwrap()));
    std::fs::write(&path, vec![37u8; 4 * 1024 * 1024]).unwrap();
    let operation_id = format!("{:032x}", getrandom::u64().unwrap());
    let upload = || Command::UploadFile {
        server_id: id.clone(),
        channel_id: GENERAL_TEXT_CHANNEL.into(),
        path: path.clone(),
        operation_id: operation_id.clone(),
        text: String::new(),
    };
    guest.send(upload());
    guest.event(|e| matches!(e,Event::Transfer(t) if t.operation_id == operation_id && t.completed > 0 && t.completed < t.total));
    guest.send(Command::CancelTransfer {
        operation_id: operation_id.clone(),
    });
    guest.event(|e| matches!(e,Event::Transfer(t) if t.operation_id == operation_id && t.finished && t.error.is_some()));
    host.state(|s| {
        s.room
            .as_ref()
            .is_some_and(|r| r.storage_usage.reserved_bytes > 0)
    });
    host.state(|s| {
        s.room
            .as_ref()
            .is_some_and(|r| r.storage_usage.reserved_bytes == 0)
    });
    guest.send(upload());
    guest.event(|e| matches!(e,Event::Transfer(t) if t.operation_id == operation_id && t.finished && t.error.is_none()));
    let room = host
        .state(|s| s.room.as_ref().is_some_and(|r| !r.history.is_empty()))
        .room
        .unwrap();
    assert_eq!(room.history.len(), 1);
    assert_eq!(room.history[0].attachments.len(), 1);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn local_admin_creates_channels_and_text_selection_is_isolated() {
    let host = Client::new();
    let port = 0;
    host.send(Command::Create {
        name: "Canales QA".into(),
        user: "Admin".into(),
        port,
        persistent: true,
    });
    let connected = host.state(|s| s.room.is_some());
    let id = connected.room.unwrap().id;
    host.send(Command::Chat("Sólo general".into()));
    host.state(|s| {
        s.room
            .as_ref()
            .is_some_and(|r| r.history.iter().any(|m| m.text == "Sólo general"))
    });
    host.send(Command::CreateChannel {
        server_id: id,
        name: "Segundo".into(),
        kind: ChannelKind::Text,
    });
    let created = host.state(|s| s.room.as_ref().is_some_and(|r| r.channels.len() == 3));
    let channel = created
        .room
        .unwrap()
        .channels
        .into_iter()
        .find(|c| c.name == "Segundo")
        .unwrap();
    host.send(Command::SelectText {
        channel_id: channel.id.clone(),
    });
    let selected = host.state(|s| {
        s.room
            .as_ref()
            .is_some_and(|r| r.selected_text == channel.id)
    });
    assert!(selected.room.unwrap().history.is_empty());
    host.send(Command::Chat("Sólo segundo".into()));
    host.state(|s| {
        s.room
            .as_ref()
            .is_some_and(|r| r.history.iter().any(|m| m.text == "Sólo segundo"))
    });
    host.send(Command::SelectText {
        channel_id: GENERAL_TEXT_CHANNEL.into(),
    });
    let general = host.state(|s| {
        s.room
            .as_ref()
            .is_some_and(|r| r.selected_text == GENERAL_TEXT_CHANNEL && r.history.len() == 1)
    });
    assert_eq!(general.room.unwrap().history[0].text, "Sólo general");
}
