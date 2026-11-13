# Integration contracts (frozen for tickets 1–3)

Read src/contracts.rs for shared UI/data types. Do not change it without coordinating. Errors are Spanish actionable strings, no invitation logging. New protocol uses QUIC/TLS, UDP45454, invites nip-native://join#base64url(JSON) contain address, certificate DER and randomtoken; pinned root verifies nip.local, no custom insecure verifier.

Audio API to implement in audio.rs:
- `pub struct AudioEngine;`
- `pub fn AudioEngine::spawn(events: std::sync::mpsc::Sender<Event>, settings: AudioSettings) -> Result<(AudioHandle, tokio::sync::mpsc::Receiver<Vec<u8>>), String>`
- `AudioHandle` Clone+Send+Sync, methods `command(&self, command: Command) -> Result<(), String>`, `status(&self) -> AudioStatus`, `outgoing_allowed(&self) -> bool`, `incoming(&self, peer: u64, packet: Vec<u8>)`, `remove_peer(&self, peer: u64)`.
- Bounded output codec packets receiver. Mono48k Opus20ms. Mic/test/deafen/privacy gates enforced atomically before work. Command errors/status emitted through Event. Supports synthetic mode only when env NIP_NATIVE_SYNTHETIC=1 (no physical sound or capture), with observable metrics helper if needed for tests. No generic fake hardware status in normal app.

Network API in network.rs:
- `pub fn spawn(profile: PathBuf, events: std::sync::mpsc::Sender<Event>, audio: AudioHandle, frames: tokio::sync::mpsc::Receiver<Vec<u8>>) -> tokio::sync::mpsc::UnboundedSender<Command>`.
- Spawn dedicated runtime worker2, handle commands in background; UI never blocks.
- Route Join/Leave/Mute/Deafen/StartTest/StopTest/AudioSettings/RefreshDevices to audio. Save audio prefs with profile. Detect AudioStatus metadata changes for server presence (may poll100ms or audio command explicit result async). Gate encoded frames at send time via outgoing_allowed.
- Server controls reliable framedJSON bounded64k on bidirectional stream; audio datagrams serverauthoritative peerID u64 prefix, compressed Opus payload <=1200; authenticate before routing. Rooms/chat/events via contracts.
- storage.rs owns Windows DPAPI Store. UI never sees tokens/certkeys; Event::Invitation only on explicit Invite. Store files in providedprofile only, no import/overwrite oldElectron profiles. Invitation validation length/port/IP(DNS if supported) no log.

UI API in ui.rs:
- `pub struct NipApp; pub fn NipApp::new(commands: UnboundedSender<Command>, events: std::sync::mpsc::Receiver<Event>) -> Self`.
- Implements eframe::App (0.37 uses ui method; inspect downloaded source), updates state from events and sends commands. Do not create fake rooms/messages/devices. Retain palette and use spacing/hierarchy vs cards.
- main.rs wires AudioEngine + network::spawn + UI. CLI `--data-dir <path>` profile; default %APPDATA%/NIPNative; `--synthetic` sets synthetic mode before spawningthreads safely edition2024 processenv set_var unsafe so parse config beforethreads or read dedicated env from launchscripts. Native dev command passes NIP_NATIVE_SYNTHETIC env externally, no needunsafe inmain.
- Main may enumerate hardware, never start capture automatically. Drop/UI close sends Shutdown.

Ownership: ticket1 native/src/network.rs/storage.rs; ticket2 native/src/audio.rs; ticket3 native/src/ui.rs/main.rs. Root owns Cargo.toml/contracts.rs/lib.rs/scripts/tests integration/docs/installer. Use CARGO_TARGET_DIR=C:/Users/santi/Documents/ChatGPT/NIP/native-prototype/target for sharedcache (Cargo serializes builds). No commits of target/executables/profiles/secrets. Tests per ticket only modules needed until all modules exist.
