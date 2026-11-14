use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct AudioSettings {
    pub input: String,
    pub output: String,
    pub input_level: f32,
    pub volume: f32,
}
impl Default for AudioSettings {
    fn default() -> Self {
        Self {
            input: String::new(),
            output: String::new(),
            input_level: 1.0,
            volume: 1.0,
        }
    }
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AudioStatus {
    pub calling: bool,
    pub muted: bool,
    pub deafened: bool,
    pub testing: bool,
    pub level: f32,
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
    pub settings: AudioSettings,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Member {
    pub id: u64,
    pub name: String,
    pub calling: bool,
    pub muted: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChatMessage {
    pub id: u64,
    pub sender_id: u64,
    pub sender_name: String,
    pub text: String,
    pub sent_at: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SavedServer {
    pub id: String,
    pub name: String,
    pub endpoint: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HostInfo {
    pub name: String,
    pub port: u16,
    pub addresses: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Room {
    pub id: String,
    pub name: String,
    pub self_id: u64,
    pub members: Vec<Member>,
    pub history: Vec<ChatMessage>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AppState {
    pub saved: Vec<SavedServer>,
    pub host: Option<HostInfo>,
    pub room: Option<Room>,
}
#[derive(Clone, Debug)]
pub enum Command {
    Create {
        name: String,
        port: u16,
        user: String,
    },
    Connect {
        invitation: String,
        user: String,
    },
    Reconnect {
        id: String,
        user: String,
    },
    Invite {
        endpoint: String,
    },
    Disconnect,
    StopHost,
    Chat(String),
    Join,
    Leave,
    Mute,
    Deafen,
    StartTest,
    StopTest,
    AudioSettings(AudioSettings),
    RefreshDevices,
    Shutdown,
}
#[derive(Clone, Debug)]
pub enum Event {
    State(AppState),
    Members(Vec<Member>),
    Chat(ChatMessage),
    Invitation(String),
    Audio(AudioStatus),
    Error(String),
    Disconnected(String),
}
