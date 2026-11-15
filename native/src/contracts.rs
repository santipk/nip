use serde::{Deserialize, Serialize};

pub const GENERAL_TEXT_CHANNEL: &str = "general";
pub const GENERAL_VOICE_CHANNEL: &str = "voice-general";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RelayConfiguration {
    pub host: crate::relay::RelayEndpoint,
    pub client: crate::relay::RelayEndpoint,
    pub tunnels: usize,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum ChannelKind {
    Text,
    Voice,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Channel {
    pub id: String,
    pub name: String,
    pub kind: ChannelKind,
}

pub fn default_channels() -> Vec<Channel> {
    vec![
        Channel {
            id: GENERAL_TEXT_CHANNEL.into(),
            name: "general".into(),
            kind: ChannelKind::Text,
        },
        Channel {
            id: GENERAL_VOICE_CHANNEL.into(),
            name: "General".into(),
            kind: ChannelKind::Voice,
        },
    ]
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct AudioSettings {
    pub input: String,
    pub output: String,
    pub input_level: f32,
    pub volume: f32,
    #[serde(default)]
    pub advanced: AdvancedAudioSettings,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct AdvancedAudioSettings {
    pub voice_activation: bool,
    pub threshold_db: f32,
    pub release_ms: u32,
    pub high_pass: bool,
    pub monitor_volume: f32,
    pub mix_limiter: bool,
    pub bitrate: u32,
    pub noise_suppression: u8,
    pub echo_cancellation: bool,
    pub echo_delay_ms: u32,
    pub automatic_gain: bool,
    pub gain_headroom_db: f32,
    pub gain_max_db: f32,
    pub processing_48khz: bool,
    pub microphone_eq: EqualizerSettings,
    pub playback_eq: EqualizerSettings,
    pub opus_complexity: u8,
    pub opus_vbr: bool,
    pub opus_dtx: bool,
    pub opus_fec: bool,
    pub packet_loss_percent: u8,
    pub join_muted: bool,
    pub push_to_talk: bool,
    pub push_to_talk_key: u16,
    pub auto_device: bool,
}
impl Default for AdvancedAudioSettings {
    fn default() -> Self {
        Self {
            voice_activation: false,
            threshold_db: -45.0,
            release_ms: 300,
            high_pass: false,
            monitor_volume: 1.0,
            mix_limiter: false,
            bitrate: 0,
            noise_suppression: 0,
            echo_cancellation: false,
            echo_delay_ms: 40,
            automatic_gain: false,
            gain_headroom_db: 5.0,
            gain_max_db: 30.0,
            processing_48khz: false,
            microphone_eq: EqualizerSettings::default(),
            playback_eq: EqualizerSettings::default(),
            opus_complexity: 5,
            opus_vbr: true,
            opus_dtx: false,
            opus_fec: false,
            packet_loss_percent: 0,
            join_muted: false,
            push_to_talk: false,
            push_to_talk_key: 0x75,
            auto_device: true,
        }
    }
}
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct EqualizerSettings {
    pub enabled: bool,
    pub preamp_db: f32,
    pub bands_db: [f32; 10],
}
impl Default for AudioSettings {
    fn default() -> Self {
        Self {
            input: String::new(),
            output: String::new(),
            input_level: 1.0,
            volume: 1.0,
            advanced: AdvancedAudioSettings::default(),
        }
    }
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AudioStatus {
    pub raw_level: f32,
    pub input_clipping: bool,
    pub calling: bool,
    pub muted: bool,
    pub deafened: bool,
    pub testing: bool,
    pub level: f32,
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
    pub settings: AudioSettings,
    pub ready: bool,
    pub captured_frames: u64,
    pub error: Option<String>,
    pub gate_open: bool,
    pub buffer_interruptions: u64,
    pub processing_us: u64,
    pub push_to_talk_active: bool,
    pub participants: Vec<ParticipantAudio>,
    pub sound_testing: bool,
    pub active_input: String,
    pub active_output: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ParticipantAudio {
    pub id: u64,
    pub volume: f32,
    pub muted: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Member {
    #[serde(default)]
    pub voice_channel: Option<String>,
    pub id: u64,
    pub name: String,
    pub calling: bool,
    pub muted: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChatMessage {
    #[serde(default)]
    pub attachments: Vec<Attachment>,
    #[serde(default = "general_text_channel")]
    pub channel_id: String,
    pub id: u64,
    pub sender_id: u64,
    pub sender_name: String,
    pub text: String,
    pub sent_at: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Attachment {
    pub id: String,
    pub channel_id: String,
    pub name: String,
    pub size: u64,
    pub sha256: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SavedServer {
    #[serde(default)]
    pub local: Option<LocalServerSettings>,
    pub id: String,
    pub name: String,
    pub endpoint: String,
    #[serde(default)]
    pub alias: Option<String>,
    #[serde(default)]
    pub owned: bool,
}
impl SavedServer {
    pub fn display_name(&self) -> &str {
        self.alias.as_deref().unwrap_or(&self.name)
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HostInfo {
    pub id: String,
    pub name: String,
    pub port: u16,
    pub addresses: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Room {
    #[serde(default)]
    pub storage_usage: crate::files::Usage,
    #[serde(default)]
    pub protocol_version: u32,
    #[serde(default = "default_channels")]
    pub channels: Vec<Channel>,
    #[serde(default = "general_text_channel")]
    pub selected_text: String,
    #[serde(default)]
    pub persistent: bool,
    #[serde(default)]
    pub has_older_messages: bool,
    pub id: String,
    pub name: String,
    pub self_id: u64,
    pub members: Vec<Member>,
    pub history: Vec<ChatMessage>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AppState {
    #[serde(default)]
    pub hosts: Vec<HostInfo>,
    #[serde(default)]
    pub diagnostics: NetworkDiagnostics,
    #[serde(default)]
    pub preferences: Preferences,
    #[serde(default)]
    pub display_name: String,
    pub saved: Vec<SavedServer>,
    pub host: Option<HostInfo>,
    pub room: Option<Room>,
    #[serde(default)]
    pub connecting: bool,
}
#[derive(Clone, Debug)]
pub enum Command {
    UploadFile {
        server_id: String,
        channel_id: String,
        path: std::path::PathBuf,
        operation_id: String,
        text: String,
    },
    DownloadFile {
        server_id: String,
        channel_id: String,
        attachment_id: String,
        destination: std::path::PathBuf,
        operation_id: String,
    },
    CancelTransfer {
        operation_id: String,
    },
    ConfigureRelay {
        server_id: String,
        configuration: Option<RelayConfiguration>,
    },
    ExportBackup {
        destination: std::path::PathBuf,
        password: age::secrecy::SecretString,
    },
    RestoreBackup {
        archive: std::path::PathBuf,
        profile: std::path::PathBuf,
        password: age::secrecy::SecretString,
    },
    SendChat {
        server_id: String,
        channel_id: String,
        text: String,
    },
    SelectText {
        channel_id: String,
    },
    CreateChannel {
        server_id: String,
        name: String,
        kind: ChannelKind,
    },
    RenameChannel {
        server_id: String,
        channel_id: String,
        name: String,
    },
    DeleteChannel {
        server_id: String,
        channel_id: String,
    },
    JoinVoice {
        channel_id: String,
    },
    StartServer {
        id: String,
        user: String,
    },
    ConfigureServer {
        id: String,
        settings: LocalServerSettings,
    },
    History {
        before: u64,
    },
    Preferences(Preferences),
    RegenerateInvitation {
        id: String,
    },
    RenameServer {
        id: String,
        name: String,
    },
    DeleteServer {
        id: String,
    },
    StopServer {
        id: String,
    },
    SetDisplayName(String),
    OpenMicrophonePermissions,
    OpenSoundSettings,
    OpenSecuritySettings,
    Create {
        persistent: bool,
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
        id: Option<String>,
        endpoint: String,
    },
    Disconnect,
    CancelConnect,
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
    ParticipantAudio(ParticipantAudio),
    TestSound,
    PlayCue {
        joined: bool,
    },
    DiagnoseNetwork,
    Shutdown,
}

fn general_text_channel() -> String {
    GENERAL_TEXT_CHANNEL.into()
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct NetworkDiagnostics {
    #[serde(default)]
    pub transport: Option<String>,
    #[serde(default)]
    pub remote_endpoint: Option<String>,
    #[serde(default)]
    pub pending_endpoint: Option<String>,
    #[serde(default)]
    pub last_error: Option<String>,
    pub local_addresses: Vec<String>,
    pub udp_status: String,
    pub rtt_ms: Option<f64>,
    pub sent_packets: u64,
    pub lost_packets: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Preferences {
    pub port: u16,
    pub max_participants: u16,
    pub auto_reconnect: bool,
    pub start_with_windows: bool,
    pub minimize_to_tray: bool,
    pub notifications: bool,
    pub join_leave_sounds: bool,
    pub mute_shortcut: u16,
    pub deafen_shortcut: u16,
}
impl Default for Preferences {
    fn default() -> Self {
        Self {
            port: 45454,
            max_participants: 32,
            auto_reconnect: true,
            start_with_windows: false,
            minimize_to_tray: false,
            notifications: true,
            join_leave_sounds: false,
            mute_shortcut: 0,
            deafen_shortcut: 0,
        }
    }
}
#[derive(Clone, Debug)]
pub enum Event {
    Transfer(TransferStatus),
    History {
        messages: Vec<ChatMessage>,
        has_more: bool,
    },
    Notice(String),
    State(AppState),
    Members(Vec<Member>),
    Chat(ChatMessage),
    Invitation(String),
    Audio(AudioStatus),
    Error(String),
    Disconnected(String),
}

#[derive(Clone, Debug)]
pub struct TransferStatus {
    pub operation_id: String,
    pub name: String,
    pub completed: u64,
    pub total: u64,
    pub finished: bool,
    pub error: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct LocalServerSettings {
    pub file_limit_bytes: u64,
    pub storage_limit_bytes: u64,
    pub persistent: bool,
    pub auto_start: bool,
    pub port: u16,
    pub max_participants: u16,
}
impl Default for LocalServerSettings {
    fn default() -> Self {
        Self {
            file_limit_bytes: 100_000_000,
            storage_limit_bytes: 10_000_000_000,
            persistent: false,
            auto_start: false,
            port: 45454,
            max_participants: 32,
        }
    }
}
