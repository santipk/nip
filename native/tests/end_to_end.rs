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
    profile: PathBuf,
    commands: tokio::sync::mpsc::UnboundedSender<Command>,
    events: mpsc::Receiver<Event>,
    audio: AudioHandle,
}
impl Client {
    fn new(label: &str) -> Self {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let profile: PathBuf = std::env::temp_dir().join(format!(
            "nip-native-e2e-{label}-{}-{suffix}",
            std::process::id()
        ));
        Self::open(profile)
    }
    fn open(profile: PathBuf) -> Self {
        let (events, receive) = mpsc::channel();
        let (audio, frames) =
            AudioEngine::spawn_synthetic(events.clone(), Default::default()).unwrap();
        let commands = network::spawn_loopback(profile.clone(), events, audio.clone(), frames);
        Self {
            profile,
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
fn persistent_server_conversion_restart_and_multiple_owned_servers() {
    let host = Client::new("persistent-host");
    let visitor = Client::new("persistent-visitor");
    let port_a = 0;
    let port_b = 0;
    host.send(Command::Create {
        name: "Permanente A".into(),
        port: port_a,
        user: "Admin".into(),
        persistent: false,
    });
    let Event::State(state) = host.event(|e| matches!(e, Event::State(s) if s.room.is_some()))
    else {
        unreachable!()
    };
    let port_a = state.hosts[0].port;
    let id_a = state.room.unwrap().id;
    host.send(Command::Chat("Conservar al convertir".into()));
    host.event(|e| matches!(e, Event::Chat(_)));
    host.send(Command::ConfigureServer {
        id: id_a.clone(),
        settings: LocalServerSettings {
            file_limit_bytes: 100_000_000,
            storage_limit_bytes: 10_000_000_000,
            persistent: true,
            auto_start: true,
            port: port_a,
            max_participants: 4,
        },
    });
    host.event(|e| matches!(e, Event::State(s) if s.room.as_ref().is_some_and(|r| r.persistent)));
    host.send(Command::Invite {
        id: None,
        endpoint: format!("127.0.0.1:{port_a}"),
    });
    let Event::Invitation(invitation) = host.event(|e| matches!(e, Event::Invitation(_))) else {
        unreachable!()
    };
    host.send(Command::Create {
        name: "Temporal B".into(),
        port: port_b,
        user: "Admin".into(),
        persistent: false,
    });
    let Event::State(state) = host.event(|e| matches!(e, Event::State(s) if s.hosts.len() == 2 && s.room.as_ref().is_some_and(|r| r.id != id_a))) else { unreachable!() };
    let id_b = state.room.unwrap().id;
    assert_ne!(id_a, id_b);
    host.send(Command::Chat("Este mensaje es temporal".into()));
    host.event(|e| matches!(e, Event::Chat(_)));
    let profile = host.profile.clone();
    host.send(Command::Shutdown);
    host.event(|e| matches!(e, Event::State(s) if s.hosts.is_empty()));
    drop(host);
    let host = Client::open(profile.clone());
    let Event::State(state) = host.event(|e| matches!(e, Event::State(s) if s.hosts.len() == 1))
    else {
        unreachable!()
    };
    assert_eq!(state.hosts[0].id, id_a);
    assert_eq!(state.saved.len(), 2);
    assert!(state.room.is_none());
    assert!(
        !host.audio.outgoing_allowed() && !host.audio.synthetic_metrics().unwrap().active_capture
    );
    visitor.send(Command::Connect {
        invitation,
        user: "Amigo".into(),
    });
    let Event::State(state) = visitor.event(|e| matches!(e, Event::State(s) if s.room.is_some()))
    else {
        unreachable!()
    };
    assert_eq!(
        state.room.unwrap().history[0].text,
        "Conservar al convertir"
    );
    host.send(Command::StartServer {
        id: id_b.clone(),
        user: "Admin".into(),
    });
    let Event::State(state) = host.event(|e| matches!(e, Event::State(s) if s.hosts.len() == 2 && s.room.as_ref().is_some_and(|r| r.id == id_b))) else { unreachable!() };
    assert!(state.room.unwrap().history.is_empty());
    visitor.send(Command::Chat("Disponible sin admin en la sala".into()));
    visitor.event(|e| matches!(e, Event::Chat(m) if m.text == "Disponible sin admin en la sala"));
    visitor.send(Command::Preferences(Preferences {
        auto_reconnect: false,
        ..Default::default()
    }));
    visitor.event(|e| matches!(e, Event::State(s) if !s.preferences.auto_reconnect));
    host.send(Command::ConfigureServer {
        id: id_a.clone(),
        settings: LocalServerSettings {
            file_limit_bytes: 100_000_000,
            storage_limit_bytes: 10_000_000_000,
            persistent: false,
            auto_start: false,
            port: port_a,
            max_participants: 4,
        },
    });
    visitor
        .event(|e| matches!(e, Event::State(s) if s.room.as_ref().is_some_and(|r| !r.persistent)));
    host.send(Command::StopServer { id: id_a.clone() });
    host.event(|e| matches!(e, Event::State(s) if s.hosts.len() == 1));
    visitor.event(|e| matches!(e, Event::Disconnected(_)));
    host.send(Command::StartServer {
        id: id_a.clone(),
        user: "Admin".into(),
    });
    let Event::State(state) = host
        .event(|e| matches!(e, Event::State(s) if s.room.as_ref().is_some_and(|r| r.id == id_a)))
    else {
        unreachable!()
    };
    assert!(state.room.unwrap().history.is_empty());
    host.send(Command::Chat("Mensaje de sesión antes de restaurar".into()));
    host.event(|e| matches!(e, Event::Chat(_)));
    host.send(Command::ConfigureServer {
        id: id_a.clone(),
        settings: LocalServerSettings {
            file_limit_bytes: 100_000_000,
            storage_limit_bytes: 10_000_000_000,
            persistent: true,
            auto_start: true,
            port: port_a,
            max_participants: 4,
        },
    });
    let Event::State(state) = host
        .event(|e| matches!(e, Event::State(s) if s.room.as_ref().is_some_and(|r| r.persistent)))
    else {
        unreachable!()
    };
    assert_eq!(
        state.room.unwrap().history.len(),
        3,
        "restoring persistence merges archived and current messages immediately"
    );
    visitor.send(Command::Reconnect {
        id: id_a.clone(),
        user: "Amigo".into(),
    });
    let Event::State(state) = visitor.event(|e| matches!(e, Event::State(s) if s.room.is_some()))
    else {
        unreachable!()
    };
    assert_eq!(state.room.unwrap().history.len(), 3);
    host.send(Command::DeleteServer { id: id_b });
    host.event(|e| matches!(e, Event::State(s) if s.saved.len() == 1));
    let history_path = profile.join("servers").join(&id_a).join("history.sqlite3");
    assert!(history_path.exists());
    host.send(Command::DeleteServer { id: id_a });
    host.event(|e| matches!(e, Event::State(s) if s.saved.is_empty()));
    visitor.event(|e| matches!(e, Event::Disconnected(_)));
    assert!(!history_path.exists());
    host.send(Command::Shutdown);
    host.event(|e| matches!(e, Event::State(s) if s.hosts.is_empty()));
    drop(host);
    std::fs::remove_dir_all(profile).unwrap();
}

#[test]
fn rename_stop_and_delete_host_preserve_connection_to_another_server() {
    let host_a = Client::new("actions-a");
    let host_b = Client::new("actions-b");
    let visitor = Client::new("actions-visitor");
    let mut invitations = Vec::new();
    let mut identities = Vec::new();
    for (host, name) in [(&host_a, "Own A"), (&host_b, "Own B")] {
        let port = 0;
        host.send(Command::Create {
            persistent: false,
            name: name.into(),
            port,
            user: "Host".into(),
        });
        let Event::State(state) = host.event(|e| matches!(e, Event::State(s) if s.host.is_some()))
        else {
            unreachable!()
        };
        let port = state.hosts[0].port;
        identities.push(state.host.unwrap().id);
        host.send(Command::Invite {
            id: None,
            endpoint: format!("127.0.0.1:{port}"),
        });
        let Event::Invitation(invite) = host.event(|e| matches!(e, Event::Invitation(_))) else {
            unreachable!()
        };
        invitations.push(invite);
    }
    visitor.send(Command::Connect {
        invitation: invitations[0].clone(),
        user: "Visitor".into(),
    });
    visitor.event(|e| matches!(e, Event::State(s) if s.room.is_some()));
    host_a.send(Command::Connect {
        invitation: invitations[1].clone(),
        user: "Host A visiting B".into(),
    });
    host_a.event(
        |e| matches!(e, Event::State(s) if s.room.as_ref().is_some_and(|r| r.id == identities[1])),
    );
    host_a.send(Command::RenameServer {
        id: identities[0].clone(),
        name: "New A".into(),
    });
    visitor.event(
        |e| matches!(e, Event::State(s) if s.room.as_ref().is_some_and(|r| r.name == "New A")),
    );
    host_a.send(Command::StopServer {
        id: identities[0].clone(),
    });
    host_a.event(|e| matches!(e, Event::State(s) if s.host.is_none() && s.room.as_ref().is_some_and(|r|r.id==identities[1])));
    visitor.event(|e| matches!(e, Event::Disconnected(_)));
    host_a.send(Command::DeleteServer {
        id: identities[0].clone(),
    });
    host_a.event(|e| matches!(e, Event::State(s) if !s.saved.iter().any(|server| server.id==identities[0]) && s.room.as_ref().is_some_and(|r|r.id==identities[1])));
    host_a.send(Command::Chat("Still connected to B".into()));
    host_b.event(|e| matches!(e, Event::Chat(m) if m.text=="Still connected to B"));
}

#[test]
fn automatic_reconnect_restores_chat_without_reopening_microphone() {
    let host = Client::new("retry-host");
    let visitor = Client::new("retry-visitor");
    let port = 0;
    host.send(Command::Create {
        persistent: false,
        name: "Reintento".into(),
        port,
        user: "Host".into(),
    });
    let Event::State(created) = host.event(|e| matches!(e, Event::State(s) if s.host.is_some()))
    else {
        unreachable!()
    };
    let port = created.hosts[0].port;
    let server_id = created.host.unwrap().id;
    host.send(Command::Invite {
        id: None,
        endpoint: format!("127.0.0.1:{port}"),
    });
    let Event::Invitation(invitation) = host.event(|e| matches!(e, Event::Invitation(_))) else {
        unreachable!()
    };
    visitor.send(Command::Connect {
        invitation,
        user: "Visitor".into(),
    });
    visitor.event(|e| matches!(e, Event::State(s) if s.room.is_some()));
    visitor.send(Command::Join);
    wait(|| visitor.audio.status().calling);
    host.send(Command::StopHost);
    host.event(|e| matches!(e, Event::State(s) if s.host.is_none()));
    visitor.event(|e| matches!(e, Event::Disconnected(_)));
    host.send(Command::StartServer {
        id: server_id,
        user: "Host".into(),
    });
    host.event(|e| matches!(e, Event::State(s) if s.host.is_some()));
    visitor.event(|e| matches!(e, Event::State(s) if !s.connecting && s.room.is_some()));
    assert!(!visitor.audio.status().calling && !visitor.audio.outgoing_allowed());
    wait(|| !visitor.audio.synthetic_metrics().unwrap().active_capture);
    visitor.send(Command::Chat("Reconectado sin micrófono".into()));
    host.event(|e| matches!(e, Event::Chat(m) if m.text == "Reconectado sin micrófono"));
}

#[test]
fn saved_servers_survive_switching_and_reconnect_without_starting_audio() {
    let host_a = Client::new("server-a");
    let host_b = Client::new("server-b");
    let visitor = Client::new("visitor");
    let mut invitations = Vec::new();
    for (host, name) in [(&host_a, "Amigos"), (&host_b, "Juegos")] {
        let port = 0;
        host.send(Command::Create {
            persistent: false,
            name: name.into(),
            port,
            user: "Host".into(),
        });
        let Event::State(created) =
            host.event(|e| matches!(e, Event::State(s) if s.room.is_some()))
        else {
            unreachable!()
        };
        let port = created.hosts[0].port;
        host.send(Command::Invite {
            id: None,
            endpoint: format!("127.0.0.1:{port}"),
        });
        let Event::Invitation(invite) = host.event(|e| matches!(e, Event::Invitation(_))) else {
            unreachable!()
        };
        invitations.push(invite);
    }
    for (index, invitation) in invitations.into_iter().enumerate() {
        visitor.send(Command::Connect {
            invitation,
            user: "QA".into(),
        });
        visitor
            .event(|e| matches!(e, Event::State(s) if !s.connecting && s.saved.len() == index + 1));
    }
    visitor.send(Command::RefreshDevices);
    let Event::State(state) = visitor.event(|e| matches!(e, Event::State(s) if s.saved.len() == 2))
    else {
        unreachable!()
    };
    assert_eq!(state.room.unwrap().name, "Juegos");
    let friends = state.saved.iter().find(|s| s.name == "Amigos").unwrap();
    visitor.send(Command::Reconnect {
        id: friends.id.clone(),
        user: "QA".into(),
    });
    visitor.event(|e| matches!(e, Event::State(s) if !s.connecting && s.room.as_ref().is_some_and(|r| r.name == "Amigos")));
    assert!(!visitor.audio.status().calling);
    assert!(!visitor.audio.synthetic_metrics().unwrap().active_capture);
}

#[test]
fn two_native_clients_chat_audio_private_monitor_and_restoration() {
    let host = Client::new("host");
    let friend = Client::new("friend");
    let port = 0;
    host.send(Command::Create {
        persistent: false,
        name: "Prueba nativa".into(),
        port,
        user: "Ana".into(),
    });
    let Event::State(created) = host.event(|e| matches!(e, Event::State(s) if s.room.is_some()))
    else {
        unreachable!()
    };
    let port = created.hosts[0].port;
    assert_eq!(
        created.saved.len(),
        1,
        "el servidor propio debe aparecer en la navegación"
    );
    assert_eq!(created.saved[0].id, created.room.as_ref().unwrap().id);
    assert!(!created.connecting);
    host.send(Command::Invite {
        id: None,
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
    friend.event(|e| matches!(e, Event::State(s) if s.connecting && s.room.is_some()));
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
    friend.send(Command::CancelConnect);
    friend.event(|e| matches!(e, Event::State(s) if !s.connecting && s.room.is_some()));
    assert!(
        friend.audio.status().calling,
        "cancelar otra conexión conserva la llamada actual"
    );
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
