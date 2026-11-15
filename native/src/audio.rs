use crate::contracts::{AudioSettings, AudioStatus, Command, Event};
use crate::dsp::{Equalizer, VoiceProcessor};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

const SAMPLES: usize = 960;
const CAPTURE: u64 = 1;
const SEND: u64 = 2;
const RECEIVE: u64 = 4;
const TEST: u64 = 8;

pub struct AudioEngine;
#[derive(Clone)]
pub struct AudioHandle {
    _lifetime: Arc<HandleLifetime>,
    shared: Arc<Shared>,
    incoming: mpsc::SyncSender<(u64, u64, Vec<u8>)>,
}
struct HandleLifetime(Arc<Shared>);
struct Control {
    shortcuts: [u16; 2],
    shortcut_actions: VecDeque<Command>,
    status: AudioStatus,
    prior_mute: Option<bool>,
    revision: u64,
    shutdown: bool,
}
struct Shared {
    control: Mutex<Control>,
    // Low four bits are gates; upper bits identify the capture generation atomically.
    flags: AtomicU64,
    hardware_generation: AtomicU64,
    failure: Mutex<Option<(u64, bool, String)>>,
    buffer_interruptions: AtomicU64,
    events: mpsc::Sender<Event>,
    synthetic: bool,
    input_unavailable: AtomicBool,
    transport_ready: AtomicBool,
    receive_generation: AtomicU64,
    metrics: Mutex<SyntheticMetrics>,
}
/// Synthetic sink measurements only. No microphone samples or recordings are retained.
#[derive(Clone, Copy, Debug, Default)]
pub struct SyntheticMetrics {
    pub captured_frames: u64,
    pub encoded_frames: u64,
    pub monitor_frames: u64,
    pub remote_frames: u64,
    pub input_rms: f32,
    pub monitor_rms: f32,
    pub remote_rms: f32,
    pub active_capture: bool,
    pub concealed_frames: u64,
    pub reordered_packets: u64,
    pub fec_attempts: u64,
}
impl Shared {
    fn device_error(&self, generation: u64, input: bool, error: cpal::Error) {
        if self.hardware_generation.load(Ordering::SeqCst) != generation {
            return;
        }
        match error.kind() {
            // WASAPI continues with the next captured buffer after a discontinuity.
            cpal::ErrorKind::Xrun => {
                self.buffer_interruptions.fetch_add(1, Ordering::Relaxed);
            }
            cpal::ErrorKind::DeviceChanged | cpal::ErrorKind::RealtimeDenied => {}
            _ => self.record_stream_error(
                generation,
                input,
                format!(
                    "{}: {error}. Revisá el dispositivo y volvé a probar.",
                    if input {
                        "El micrófono dejó de entregar audio"
                    } else {
                        "La salida de audio falló"
                    }
                ),
            ),
        }
    }
    #[cfg(test)]
    fn stream_error(&self, generation: u64, message: String) {
        self.record_stream_error(generation, false, message);
    }
    fn record_stream_error(&self, generation: u64, input: bool, message: String) {
        if self.hardware_generation.load(Ordering::SeqCst) == generation {
            *self.failure.lock().unwrap() = Some((generation, input, message));
        }
    }
    fn take_stream_error(&self) -> Option<(bool, String)> {
        self.failure
            .lock()
            .unwrap()
            .take()
            .and_then(|(generation, input, message)| {
                (self.hardware_generation.load(Ordering::SeqCst) == generation)
                    .then_some((input, message))
            })
    }
    fn publish(&self, c: &Control) {
        let s = &c.status;
        let active = !c.shutdown && !s.deafened && (s.calling || s.testing);
        let flags = if active {
            (if self.input_unavailable.load(Ordering::SeqCst) {
                0
            } else {
                CAPTURE
            }) | if s.testing {
                TEST
            } else {
                RECEIVE
                    | if !self.transport_ready.load(Ordering::SeqCst)
                        || self.input_unavailable.load(Ordering::SeqCst)
                        || s.muted
                        || (s.settings.advanced.push_to_talk && !s.push_to_talk_active)
                    {
                        0
                    } else {
                        SEND
                    }
            }
        } else {
            0
        };
        let token = (self.flags.load(Ordering::SeqCst) & !15).wrapping_add(16) | flags;
        self.flags.store(token, Ordering::SeqCst);
        let _ = self.events.send(Event::Audio(s.clone()));
    }
    fn error(&self, message: String) {
        self.flags.fetch_and(!15, Ordering::SeqCst);
        let mut c = self.control.lock().unwrap();
        if let Some(prior) = c.prior_mute.take() {
            c.status.muted = prior;
        }
        c.status.calling = false;
        c.status.testing = false;
        c.status.level = 0.0;
        c.status.raw_level = 0.0;
        c.status.input_clipping = false;
        c.status.ready = false;
        c.status.error = Some(message.clone());
        c.revision += 1;
        self.publish(&c);
        let _ = self.events.send(Event::Error(message));
    }
}
impl AudioEngine {
    pub fn spawn(
        events: mpsc::Sender<Event>,
        settings: AudioSettings,
    ) -> Result<(AudioHandle, tokio::sync::mpsc::Receiver<Vec<u8>>), String> {
        Self::spawn_with_mode(
            events,
            settings,
            std::env::var("NIP_NATIVE_SYNTHETIC").as_deref() == Ok("1"),
        )
    }
    /// Diagnostic engine: real Opus with a 440 Hz source and a measured in-memory sink.
    /// Never opens physical audio devices. The application uses `spawn` by default.
    pub fn spawn_synthetic(
        events: mpsc::Sender<Event>,
        settings: AudioSettings,
    ) -> Result<(AudioHandle, tokio::sync::mpsc::Receiver<Vec<u8>>), String> {
        Self::spawn_with_mode(events, settings, true)
    }
    /// Output-only synthetic hardware boundary; never opens a physical device.
    pub fn spawn_synthetic_without_input(
        events: mpsc::Sender<Event>,
        settings: AudioSettings,
    ) -> Result<(AudioHandle, tokio::sync::mpsc::Receiver<Vec<u8>>), String> {
        Self::spawn_with_input(events, settings, true, false)
    }
    fn spawn_with_mode(
        events: mpsc::Sender<Event>,
        settings: AudioSettings,
        synthetic: bool,
    ) -> Result<(AudioHandle, tokio::sync::mpsc::Receiver<Vec<u8>>), String> {
        Self::spawn_with_input(events, settings, synthetic, true)
    }
    fn spawn_with_input(
        events: mpsc::Sender<Event>,
        settings: AudioSettings,
        synthetic: bool,
        input_available: bool,
    ) -> Result<(AudioHandle, tokio::sync::mpsc::Receiver<Vec<u8>>), String> {
        validate(&settings)?;
        let encoder = opus::Encoder::new(48000, opus::Channels::Mono, opus::Application::Voip)
            .map_err(|e| format!("No se pudo iniciar Opus: {e}"))?;
        let shared = Arc::new(Shared {
            control: Mutex::new(Control {
                shortcuts: [0; 2],
                shortcut_actions: VecDeque::new(),
                status: AudioStatus {
                    settings,
                    ..Default::default()
                },
                prior_mute: None,
                revision: 0,
                shutdown: false,
            }),
            flags: AtomicU64::new(0),
            hardware_generation: AtomicU64::new(0),
            failure: Mutex::new(None),
            buffer_interruptions: AtomicU64::new(0),
            events,
            synthetic,
            input_unavailable: AtomicBool::new(!input_available),
            transport_ready: AtomicBool::new(true),
            receive_generation: AtomicU64::new(0),
            metrics: Mutex::new(SyntheticMetrics::default()),
        });
        let (incoming, receiver) = mpsc::sync_channel(64);
        let (sender, packets) = tokio::sync::mpsc::channel(3);
        let worker = shared.clone();
        thread::Builder::new()
            .name("nip-audio".into())
            .spawn(move || run(worker, receiver, sender, encoder))
            .map_err(|e| format!("No se pudo iniciar el motor de audio: {e}"))?;
        Ok((
            AudioHandle {
                _lifetime: Arc::new(HandleLifetime(shared.clone())),
                shared,
                incoming,
            },
            packets,
        ))
    }
}
pub(crate) fn validate(s: &AudioSettings) -> Result<(), String> {
    if !s.input_level.is_finite()
        || !(0.0..=1.0).contains(&s.input_level)
        || !s.volume.is_finite()
        || !(0.0..=1.0).contains(&s.volume)
    {
        return Err("Nivel y volumen deben estar entre 0 y 100%.".into());
    }
    let a = &s.advanced;
    if !a.threshold_db.is_finite()
        || !(-72.0..=-12.0).contains(&a.threshold_db)
        || !(40..=1000).contains(&a.release_ms)
        || !a.monitor_volume.is_finite()
        || !(0.0..=1.0).contains(&a.monitor_volume)
        || ![0, 16000, 32000, 64000].contains(&a.bitrate)
        || a.noise_suppression > 4
        || a.echo_delay_ms > 500
        || !a.gain_headroom_db.is_finite()
        || !(0.0..=20.0).contains(&a.gain_headroom_db)
        || !a.gain_max_db.is_finite()
        || !(0.0..=50.0).contains(&a.gain_max_db)
        || a.opus_complexity > 10
        || a.packet_loss_percent > 100
        || !valid_hotkey(a.push_to_talk_key)
    {
        return Err(
            "Revisá el umbral, la pausa, el volumen de prueba y la calidad de voz en Avanzado."
                .into(),
        );
    }
    for eq in [&a.microphone_eq, &a.playback_eq] {
        if !eq.preamp_db.is_finite()
            || !(-24.0..=12.0).contains(&eq.preamp_db)
            || eq
                .bands_db
                .iter()
                .any(|db| !db.is_finite() || !(-12.0..=12.0).contains(db))
        {
            return Err(
                "Ecualización inválida: preamplificador −24 a +12 dB; bandas −12 a +12 dB.".into(),
            );
        }
    }
    Ok(())
}
impl AudioHandle {
    /// Network transitions close publication without changing capture or user controls.
    pub fn set_transport_ready(&self, ready: bool) {
        let control = self.shared.control.lock().unwrap();
        if !ready {
            self.shared
                .receive_generation
                .fetch_add(1, Ordering::SeqCst);
        }
        self.shared.transport_ready.store(ready, Ordering::SeqCst);
        self.shared.publish(&control);
    }
    pub fn flush_receive(&self) {
        let control = self.shared.control.lock().unwrap();
        self.shared
            .receive_generation
            .fetch_add(1, Ordering::SeqCst);
        self.shared.publish(&control);
    }
    pub fn configure_shortcuts(&self, keys: [u16; 2]) {
        let mut c = self.shared.control.lock().unwrap();
        c.shortcuts = keys;
        c.shortcut_actions.clear();
    }
    pub fn take_shortcut_actions(&self) -> Vec<Command> {
        self.shared
            .control
            .lock()
            .unwrap()
            .shortcut_actions
            .drain(..)
            .collect()
    }
    pub fn command(&self, command: Command) -> Result<(), String> {
        if let Command::AudioSettings(s) = &command {
            validate(s)?;
        }
        let mut c = self.shared.control.lock().unwrap();
        if c.shutdown {
            return Err("El motor de audio está cerrado.".into());
        }
        match command {
            Command::Join => {
                if !self.shared.synthetic {
                    self.shared.input_unavailable.store(false, Ordering::SeqCst);
                }
                c.status.error = None;
                if c.status.settings.advanced.join_muted {
                    c.status.muted = true;
                }
                c.status.calling = true;
            }
            Command::Leave | Command::Disconnect | Command::Shutdown => {
                if matches!(command, Command::Disconnect | Command::Shutdown) {
                    c.status.participants.clear();
                }
                if let Some(prior) = c.prior_mute.take() {
                    c.status.muted = prior;
                }
                c.status.calling = false;
                c.status.testing = false;
                c.status.level = 0.0;
                if matches!(command, Command::Shutdown) {
                    c.shutdown = true;
                }
            }
            Command::Mute => {
                if !c.status.testing {
                    c.status.error = None;
                    c.status.muted = !c.status.muted;
                }
            }
            Command::Deafen => {
                if let Some(prior) = c.prior_mute.take() {
                    c.status.muted = prior;
                }
                c.status.testing = false;
                c.status.deafened = !c.status.deafened;
            }
            Command::StartTest => {
                if self.shared.synthetic && self.shared.input_unavailable.load(Ordering::SeqCst) {
                    return Err("No hay micrófono disponible para la prueba privada.".into());
                }
                self.shared.input_unavailable.store(false, Ordering::SeqCst);
                c.status.error = None;
                if c.status.deafened {
                    return Err("Desactivá ensordecer antes de probar el micrófono.".into());
                }
                // Disable publication before enabling any monitor capture.
                self.shared.flags.fetch_and(!SEND, Ordering::SeqCst);
                if !c.status.testing {
                    c.prior_mute = Some(c.status.muted);
                    c.status.testing = true;
                }
            }
            Command::StopTest => {
                c.status.testing = false;
                if let Some(prior) = c.prior_mute.take() {
                    c.status.muted = prior;
                }
            }
            Command::AudioSettings(settings) => {
                // Device switches end private tests and close old streams before opening replacements.
                if settings.input != c.status.settings.input
                    || settings.output != c.status.settings.output
                {
                    if !self.shared.synthetic {
                        self.shared.input_unavailable.store(false, Ordering::SeqCst);
                    }
                    c.status.testing = false;
                    if let Some(prior) = c.prior_mute.take() {
                        c.status.muted = prior;
                    }
                }
                c.status.settings = settings;
            }
            Command::RefreshDevices => {}
            sound @ (Command::TestSound | Command::PlayCue { .. }) => {
                if c.status.deafened {
                    return Err("Desactivá Ensordecer para probar la salida.".into());
                }
                if !c.status.sound_testing {
                    c.status.sound_testing = true;
                    let shared = self.shared.clone();
                    let settings = c.status.settings.clone();
                    let (frequency, duration_ms) = match sound {
                        Command::PlayCue { joined } => (if joined { 660.0 } else { 330.0 }, 160),
                        _ => (440.0, 2000),
                    };
                    thread::Builder::new()
                        .name("nip-output-test".into())
                        .spawn(move || {
                            let result = test_output(&shared, &settings, frequency, duration_ms);
                            let mut c = shared.control.lock().unwrap();
                            c.status.sound_testing = false;
                            let _ = shared.events.send(Event::Audio(c.status.clone()));
                            if let Err(error) = result {
                                let _ = shared.events.send(Event::Error(error));
                            }
                        })
                        .map_err(|_| {
                            c.status.sound_testing = false;
                            "No se pudo iniciar la prueba de salida."
                        })?;
                }
            }
            Command::ParticipantAudio(participant) => {
                if !participant.volume.is_finite() || !(0.0..=2.0).contains(&participant.volume) {
                    return Err("Volumen por persona inválido.".into());
                }
                c.status.participants.retain(|p| p.id != participant.id);
                c.status.participants.push(participant);
            }
            _ => return Ok(()),
        }
        c.revision += 1;
        self.shared.publish(&c);
        Ok(())
    }
    pub fn status(&self) -> AudioStatus {
        self.shared.control.lock().unwrap().status.clone()
    }
    pub fn outgoing_allowed(&self) -> bool {
        self.shared.flags.load(Ordering::SeqCst) & SEND != 0
    }
    pub fn incoming(&self, peer: u64, packet: Vec<u8>) {
        if packet.len() <= 1200 && self.shared.flags.load(Ordering::SeqCst) & RECEIVE != 0 {
            let generation = self.shared.receive_generation.load(Ordering::SeqCst);
            let _ = self.incoming.try_send((peer, generation, packet));
        }
    }
    pub fn remove_peer(&self, peer: u64) {
        self.shared
            .control
            .lock()
            .unwrap()
            .status
            .participants
            .retain(|p| p.id != peer);
        self.flush_receive();
    }
    pub fn synthetic_metrics(&self) -> Option<SyntheticMetrics> {
        self.shared
            .synthetic
            .then(|| *self.shared.metrics.lock().unwrap())
    }
}
impl Drop for HandleLifetime {
    fn drop(&mut self) {
        // Executed exactly once when the last public handle is dropped; callback Arcs do not retain it.
        self.0.flags.fetch_and(!15, Ordering::SeqCst);
        let mut c = self.0.control.lock().unwrap();
        c.shutdown = true;
    }
}
struct Hardware {
    shared: Arc<Shared>,
    generation: u64,
    _input: Option<cpal::Stream>,
    input_error: Option<String>,
    _output: cpal::Stream,
    capture: rtrb::Consumer<(f32, u64)>,
    playback: rtrb::Producer<(f32, u64)>,
    input_name: String,
    output_name: String,
    output_rate: u32,
    output_resampler: DeviceResampler,
}

// The hardware boundary resamples only; DSP and transport always consume mono 48 kHz.
// PacketResampler allocates its sinc buffers here, before either callback starts.
struct DeviceResampler {
    inner: Option<fixed_resample::PacketResampler<f32, fixed_resample::Interleaved<f32>>>,
    epoch: Option<u64>,
}
impl DeviceResampler {
    fn new(input_rate: u32, output_rate: u32) -> Self {
        Self {
            inner: (input_rate != output_rate).then(|| {
                fixed_resample::PacketResampler::new(
                    1,
                    input_rate,
                    output_rate,
                    fixed_resample::ResamplerConfig {
                        quality: fixed_resample::ResampleQuality::HighWithLowLatency,
                        chunk_size: 256,
                    },
                )
            }),
            epoch: None,
        }
    }
    fn process(&mut self, samples: &[f32], epoch: u64, mut output: impl FnMut(&[f32])) {
        if let Some(inner) = &mut self.inner {
            if self.epoch != Some(epoch) {
                inner.reset();
            }
            let input = fixed_resample::audioadapter_buffers::direct::InterleavedSlice::new(
                samples,
                1,
                samples.len(),
            )
            .expect("mono slice dimensions");
            // Preserve the bounded filter delay. Version 0.13.0's interleaved
            // trim path can panic when the delay exceeds the first output block.
            inner.process(&input, None, None, |packet, _| output(packet), None, false);
        } else {
            output(samples);
        }
        self.epoch = Some(epoch);
    }
}
impl Drop for Hardware {
    fn drop(&mut self) {
        // Retire callbacks before CPAL streams are dropped. Their late errors belong to old devices.
        let _ = self.shared.hardware_generation.compare_exchange(
            self.generation,
            self.generation + 1,
            Ordering::SeqCst,
            Ordering::SeqCst,
        );
    }
}
struct Peer {
    decoder: opus::Decoder,
    packets: BTreeMap<u64, Vec<u8>>,
    next_sequence: u64,
    highest_sequence: u64,
    startup_ticks: u8,
    missing: u8,
}
impl Peer {
    fn receive(&mut self, sequence: u64, packet: Vec<u8>) -> bool {
        let reordered = sequence < self.highest_sequence;
        self.highest_sequence = self.highest_sequence.max(sequence);
        if sequence < self.next_sequence {
            if self.startup_ticks == 0 {
                return reordered;
            }
            self.next_sequence = sequence;
        }
        if sequence.saturating_sub(self.next_sequence) > 64 {
            self.packets.clear();
            self.next_sequence = sequence;
            self.startup_ticks = 2;
        }
        self.packets.entry(sequence).or_insert(packet);
        while self.packets.len() > 8 {
            self.packets.pop_first();
        }
        reordered
    }
    fn frame(&mut self) -> Option<([f32; SAMPLES], bool, bool)> {
        if self.startup_ticks > 0 {
            self.startup_ticks -= 1;
            return None;
        }
        let mut pcm = [0.0; SAMPLES];
        let normal = self.packets.remove(&self.next_sequence);
        if normal.is_none() && self.packets.is_empty() && self.missing >= 3 {
            return None;
        }
        let fec = normal.is_none()
            && self
                .packets
                .contains_key(&self.next_sequence.saturating_add(1));
        let encoded = normal
            .as_deref()
            .or_else(|| {
                self.packets
                    .get(&self.next_sequence.saturating_add(1))
                    .map(Vec::as_slice)
            })
            .unwrap_or(&[]);
        let decoded = self.decoder.decode_float(encoded, &mut pcm, fec).ok() == Some(SAMPLES);
        self.next_sequence = self.next_sequence.wrapping_add(1);
        self.missing = if normal.is_some() {
            0
        } else {
            self.missing.saturating_add(1)
        };
        decoded.then_some((pcm, normal.is_none(), fec))
    }
}
fn rms(samples: &[f32]) -> f32 {
    (samples.iter().map(|x| x * x).sum::<f32>() / samples.len() as f32).sqrt()
}
#[derive(Default)]
struct MicrophoneProcessor {
    previous_input: f32,
    previous_output: f32,
    hold_frames: u32,
}
impl MicrophoneProcessor {
    fn process(&mut self, samples: &mut [f32], settings: &AudioSettings) -> (f32, bool) {
        let a = &settings.advanced;
        let coefficient = 1.0 / (1.0 + std::f32::consts::TAU * 80.0 / 48000.0);
        for sample in samples.iter_mut() {
            if a.high_pass {
                let filtered = coefficient * (self.previous_output + *sample - self.previous_input);
                self.previous_input = *sample;
                self.previous_output = filtered;
                *sample = filtered;
            } else {
                self.previous_input = 0.0;
                self.previous_output = 0.0;
            }
            *sample = (*sample * settings.input_level).clamp(-1.0, 1.0);
        }
        let level = rms(samples);
        let open = if !a.voice_activation {
            true
        } else if level >= 10.0f32.powf(a.threshold_db / 20.0) {
            self.hold_frames = a.release_ms.div_ceil(20);
            true
        } else if self.hold_frames > 0 {
            self.hold_frames -= 1;
            true
        } else {
            false
        };
        if !open {
            samples.fill(0.0);
        }
        (level, open)
    }
}
fn process_playback(samples: &mut [f32], volume: f32, limiter: bool) {
    let peak = samples
        .iter()
        .fold(0.0f32, |peak, sample| peak.max(sample.abs() * volume));
    let gain = if limiter && peak > 1.0 {
        volume / peak
    } else {
        volume
    };
    for sample in samples {
        *sample = (*sample * gain).clamp(-1.0, 1.0);
    }
}
fn run(
    shared: Arc<Shared>,
    incoming: mpsc::Receiver<(u64, u64, Vec<u8>)>,
    outgoing: tokio::sync::mpsc::Sender<Vec<u8>>,
    mut encoder: opus::Encoder,
) {
    let mut hardware: Option<Hardware> = None;
    let mut peers = HashMap::<u64, Peer>::new();
    let mut previous_devices = None;
    let mut revision = u64::MAX;
    let mut phase = 0.0f32;
    let mut tick = Instant::now();
    let mut packet = [0u8; 1192];
    let mut sequence = 0u64;
    let mut last_flags = 0;
    let mut receive_generation = shared.receive_generation.load(Ordering::SeqCst);
    let mut last_status_event = Instant::now();
    let mut last_capture = Instant::now();
    let mut microphone = MicrophoneProcessor::default();
    let mut voice = VoiceProcessor::default();
    let mut input_eq = Equalizer::default();
    let mut output_eq = Equalizer::default();
    let mut render_reference = [0.0; SAMPLES];
    let mut last_encoder = None;
    let mut last_device_check = Instant::now();
    let mut device_signature = None;
    let mut shortcut_keys = [0; 2];
    let mut shortcut_down = [false; 2];
    let mut clipped_at: Option<Instant> = None;
    loop {
        {
            let mut c = shared.control.lock().unwrap();
            if c.shortcuts != shortcut_keys {
                shortcut_keys = c.shortcuts;
                shortcut_down = shortcut_keys.map(hotkey_down);
            } else {
                for (index, key) in shortcut_keys.into_iter().enumerate() {
                    let pressed = hotkey_down(key);
                    if pressed && !shortcut_down[index] && c.shortcut_actions.len() < 8 {
                        c.shortcut_actions.push_back(if index == 0 {
                            Command::Mute
                        } else {
                            Command::Deafen
                        });
                    }
                    shortcut_down[index] = pressed;
                }
            }
            let pressed = c.status.calling
                && !c.status.testing
                && c.status.settings.advanced.push_to_talk
                && hotkey_down(c.status.settings.advanced.push_to_talk_key);
            if pressed != c.status.push_to_talk_active {
                c.status.push_to_talk_active = pressed;
                shared.publish(&c);
            }
        }
        if let Some((input, error)) = shared.take_stream_error() {
            if input {
                if let Some(h) = &mut hardware {
                    h._input = None;
                }
                shared.input_unavailable.store(true, Ordering::SeqCst);
                let mut c = shared.control.lock().unwrap();
                c.status.active_input.clear();
                c.status.error = Some(format!("Modo escucha: {error}"));
                if let Some(prior) = c.prior_mute.take() {
                    c.status.muted = prior;
                }
                c.status.testing = false;
                shared.publish(&c);
            } else {
                hardware = None;
                let settings = shared.control.lock().unwrap().status.settings.clone();
                if !settings.advanced.auto_device || shared.synthetic {
                    shared.error(error);
                } else {
                    shared.input_unavailable.store(true, Ordering::SeqCst);
                    let mut control = shared.control.lock().unwrap();
                    control.status.active_input.clear();
                    control.status.error = Some(format!(
                        "Modo escucha: se recupera la salida sin reabrir el micrófono. {error}"
                    ));
                    shared.publish(&control);
                }
            }
        }
        let (status, current_revision, shutdown) = {
            let c = shared.control.lock().unwrap();
            (c.status.clone(), c.revision, c.shutdown)
        };
        if shutdown {
            break;
        }
        let flags = shared.flags.load(Ordering::SeqCst);
        if !shared.synthetic
            && status.settings.advanced.auto_device
            && last_device_check.elapsed() > Duration::from_secs(2)
        {
            last_device_check = Instant::now();
            let host = cpal::default_host();
            let signature = (
                host.default_input_device()
                    .and_then(|d| d.description().ok().map(|d| d.name().to_owned())),
                host.default_output_device()
                    .and_then(|d| d.description().ok().map(|d| d.name().to_owned())),
                enumerate().unwrap_or_default(),
            );
            if device_signature
                .as_ref()
                .is_some_and(|old| old != &signature)
            {
                shared.input_unavailable.store(true, Ordering::SeqCst);
                let mut control = shared.control.lock().unwrap();
                control.status.error = Some("Modo escucha: cambió la topología de audio. Seleccioná el micrófono o volvé a entrar a voz para habilitar captura.".into());
                control.status.active_input.clear();
                control.status.testing = false;
                if let Some(prior) = control.prior_mute.take() {
                    control.status.muted = prior;
                }
                shared.publish(&control);
                drop(control);
                hardware = None;
                voice = VoiceProcessor::default();
                render_reference.fill(0.0);
                shared.control.lock().unwrap().revision += 1;
            }
            device_signature = Some(signature);
        }
        let epoch = flags;
        if current_revision != revision {
            revision = current_revision;
            let a = &status.settings.advanced;
            let encoder_config = (
                a.bitrate,
                a.opus_complexity,
                a.opus_vbr,
                a.opus_dtx,
                a.opus_fec,
                a.packet_loss_percent,
            );
            if last_encoder != Some(encoder_config) {
                let bitrate = a.bitrate;
                let result = (|| {
                    encoder.set_complexity(a.opus_complexity as i32)?;
                    encoder.set_vbr(a.opus_vbr)?;
                    encoder.set_dtx(a.opus_dtx)?;
                    encoder.set_inband_fec(a.opus_fec)?;
                    encoder.set_packet_loss_perc(a.packet_loss_percent as i32)?;
                    encoder.set_bitrate(if bitrate == 0 {
                        opus::Bitrate::Auto
                    } else {
                        opus::Bitrate::Bits(bitrate as i32)
                    })
                })();
                if let Err(error) = result {
                    shared.error(format!("No se pudo aplicar la calidad de voz: {error}"));
                    continue;
                }
                last_encoder = Some(encoder_config);
            }
            let devices = (
                status.settings.input.clone(),
                status.settings.output.clone(),
            );
            if previous_devices.as_ref() != Some(&devices) {
                hardware = None;
                microphone = MicrophoneProcessor::default();
                voice = VoiceProcessor::default();
                input_eq = Equalizer::default();
                output_eq = Equalizer::default();
                render_reference.fill(0.0);
                previous_devices = Some(devices);
            }
            let (inputs, outputs) = if shared.synthetic {
                (
                    if shared.input_unavailable.load(Ordering::SeqCst) {
                        vec![]
                    } else {
                        vec!["Entrada sintética 440 Hz".into()]
                    },
                    vec!["Salida sintética".into()],
                )
            } else {
                match enumerate() {
                    Ok(devices) => devices,
                    Err(error) => {
                        shared.control.lock().unwrap().status.error = Some(error);
                        (vec![], vec![])
                    }
                }
            };
            let mut c = shared.control.lock().unwrap();
            c.status.inputs = inputs;
            c.status.outputs = outputs;
            let _ = shared.events.send(Event::Audio(c.status.clone()));
        }
        if flags & CAPTURE != 0 && hardware.as_ref().is_some_and(|h| h._input.is_none()) {
            hardware = None;
        }
        if flags & (CAPTURE | RECEIVE | TEST) == 0 {
            hardware = None;
        } else if !shared.synthetic && hardware.is_none() {
            let opened =
                open_hardware(&shared, &status.settings, status.testing).or_else(|error| {
                    if status.settings.advanced.auto_device
                        && (!status.settings.input.is_empty() || !status.settings.output.is_empty())
                    {
                        let mut fallback = status.settings.clone();
                        fallback.input.clear();
                        fallback.output.clear();
                        open_hardware(&shared, &fallback, status.testing)
                    } else {
                        Err(error)
                    }
                });
            match opened {
                Ok(h) => {
                    let mut c = shared.control.lock().unwrap();
                    shared
                        .input_unavailable
                        .store(h._input.is_none(), Ordering::SeqCst);
                    c.status.error = h.input_error.clone();
                    c.status.active_input = h.input_name.clone();
                    c.status.active_output = h.output_name.clone();
                    c.status.ready = true;
                    shared.publish(&c);
                    drop(c);
                    hardware = Some(h);
                    last_capture = Instant::now();
                    voice = VoiceProcessor::default();
                    render_reference.fill(0.0);
                }
                Err(e) => {
                    shared.hardware_generation.fetch_add(1, Ordering::SeqCst);
                    shared.error(e);
                    continue;
                }
            }
        }
        if flags & 15 != last_flags {
            microphone = MicrophoneProcessor::default();
            voice = VoiceProcessor::default();
            render_reference.fill(0.0);
            peers.clear();
            while incoming.try_recv().is_ok() {}
            if let Some(h) = &mut hardware {
                while h.capture.pop().is_ok() {}
            }
            last_flags = flags & 15;
        }
        if flags & (CAPTURE | RECEIVE | TEST) == 0 {
            let mut c = shared.control.lock().unwrap();
            c.status.ready = false;
            c.status.captured_frames = 0;
            if c.status.level != 0.0 || c.status.raw_level != 0.0 {
                c.status.level = 0.0;
                c.status.raw_level = 0.0;
                c.status.input_clipping = false;
                let _ = shared.events.send(Event::Audio(c.status.clone()));
            }
            drop(c);
            if shared.synthetic {
                let mut metrics = shared.metrics.lock().unwrap();
                metrics.active_capture = false;
                metrics.input_rms = 0.0;
                metrics.monitor_rms = 0.0;
                metrics.remote_rms = 0.0;
            }
            thread::sleep(Duration::from_millis(if shortcut_keys == [0; 2] {
                20
            } else {
                2
            }));
            continue;
        }
        let current_receive_generation = shared.receive_generation.load(Ordering::SeqCst);
        if current_receive_generation != receive_generation {
            peers.clear();
            receive_generation = current_receive_generation;
        }
        for _ in 0..64 {
            let Ok((id, generation, bytes)) = incoming.try_recv() else {
                break;
            };
            if generation != receive_generation {
                continue;
            }
            if flags & RECEIVE == 0 || (peers.len() >= 32 && !peers.contains_key(&id)) {
                continue;
            }
            if bytes.len() <= 8 || bytes.len() > 1200 {
                continue;
            }
            let sequence = u64::from_be_bytes(bytes[..8].try_into().unwrap());
            let peer = peers.entry(id).or_insert_with(|| Peer {
                decoder: opus::Decoder::new(48000, opus::Channels::Mono)
                    .expect("valid Opus configuration"),
                packets: BTreeMap::new(),
                next_sequence: sequence,
                highest_sequence: sequence,
                startup_ticks: 2,
                missing: 0,
            });
            if peer.receive(sequence, bytes[8..].to_vec()) && shared.synthetic {
                shared.metrics.lock().unwrap().reordered_packets += 1;
            }
        }
        if tick.elapsed() >= Duration::from_millis(20) {
            tick = Instant::now();
            let mut input = [0.0; SAMPLES];
            let mut ready = flags & CAPTURE != 0;
            if ready {
                if shared.synthetic {
                    for x in &mut input {
                        *x = phase.sin() * 0.25;
                        phase = (phase + std::f32::consts::TAU * 440.0 / 48000.0)
                            % std::f32::consts::TAU;
                    }
                } else if let Some(h) = &mut hardware {
                    // Prefer recent audio and never let callback buffers grow unbounded.
                    while h.capture.slots() > SAMPLES * 2 {
                        let _ = h.capture.pop();
                    }
                    if h.capture.slots() >= SAMPLES {
                        last_capture = Instant::now();
                        for x in &mut input {
                            let (sample, sample_epoch) = h.capture.pop().unwrap_or((0.0, epoch));
                            *x = sample;
                            if sample_epoch != epoch {
                                ready = false;
                            }
                        }
                    } else {
                        ready = false;
                    }
                }
            }
            let processing_started = Instant::now();
            let raw_level = if ready { rms(&input) } else { 0.0 };
            if ready && input.iter().any(|sample| sample.abs() >= 0.999) {
                clipped_at = Some(Instant::now());
            }
            if ready {
                if let Err(error) =
                    voice.process(&mut input, &render_reference, &status.settings.advanced)
                {
                    shared.error(error);
                    continue;
                }
                input_eq.process(&mut input, &status.settings.advanced.microphone_eq);
            }
            let (level, gate_open) = if ready {
                microphone.process(&mut input, &status.settings)
            } else {
                (0.0, false)
            };
            let processing_us = processing_started.elapsed().as_micros() as u64;
            if hardware.as_ref().is_some_and(|h| h._input.is_some())
                && last_capture.elapsed() > Duration::from_secs(3)
            {
                if let Some(h) = &mut hardware {
                    h._input = None;
                }
                shared.input_unavailable.store(true, Ordering::SeqCst);
                let mut c = shared.control.lock().unwrap();
                c.status.active_input.clear();
                c.status.error = Some("Modo escucha: el micrófono abrió, pero Windows no entregó muestras durante 3 segundos. Revisá el dispositivo y sus permisos; después volvé a probar.".into());
                if let Some(prior) = c.prior_mute.take() {
                    c.status.muted = prior;
                }
                c.status.testing = false;
                shared.publish(&c);
                continue;
            }
            let current = shared.flags.load(Ordering::SeqCst);
            if ready
                && gate_open
                && flags & SEND != 0
                && current & SEND != 0
                && shared.flags.load(Ordering::SeqCst) == epoch
            {
                match encoder.encode_float(&input, &mut packet) {
                    Ok(n) => {
                        let mut framed = Vec::with_capacity(n + 8);
                        framed.extend_from_slice(&sequence.to_be_bytes());
                        framed.extend_from_slice(&packet[..n]);
                        sequence = sequence.wrapping_add(1);
                        if shared.flags.load(Ordering::SeqCst) & SEND != 0
                            && shared.flags.load(Ordering::SeqCst) == epoch
                            && outgoing.try_send(framed).is_ok()
                            && shared.synthetic
                        {
                            shared.metrics.lock().unwrap().encoded_frames += 1;
                        }
                    }
                    Err(e) => {
                        hardware = None;
                        shared.error(format!("No se pudo codificar audio: {e}"));
                        continue;
                    }
                }
            }
            let mut playback = [0.0; SAMPLES];
            let monitor = ready && current & TEST != 0;
            let mut remote = false;
            if monitor {
                playback.copy_from_slice(&input);
            } else if current & RECEIVE != 0 {
                for (id, peer) in peers.iter_mut() {
                    if let Some((frame, concealed, fec)) = peer.frame() {
                        if shared.synthetic {
                            let mut metrics = shared.metrics.lock().unwrap();
                            metrics.concealed_frames += u64::from(concealed);
                            metrics.fec_attempts += u64::from(fec);
                        }
                        let preference = status.participants.iter().find(|p| p.id == *id);
                        let gain = preference.map_or(1.0, |p| if p.muted { 0.0 } else { p.volume });
                        remote = true;
                        for (to, from) in playback.iter_mut().zip(frame) {
                            *to += from * gain;
                        }
                    }
                }
            }
            let volume = status.settings.volume
                * if monitor {
                    status.settings.advanced.monitor_volume
                } else {
                    1.0
                };
            output_eq.process(&mut playback, &status.settings.advanced.playback_eq);
            process_playback(&mut playback, volume, status.settings.advanced.mix_limiter);
            render_reference.fill(0.0);
            if let Some(h) = &mut hardware {
                let needed = (h.output_rate as usize / 50) + 512;
                if h.playback.slots() >= needed {
                    render_reference.copy_from_slice(&playback);
                    let queue = &mut h.playback;
                    h.output_resampler.process(&playback, epoch, |packet| {
                        for sample in packet {
                            let _ = queue.push((*sample, epoch));
                        }
                    });
                }
            }
            if shared.synthetic {
                let mut m = shared.metrics.lock().unwrap();
                m.active_capture = current & CAPTURE != 0;
                if ready {
                    m.captured_frames += 1;
                }
                m.input_rms = level;
                m.monitor_rms = if monitor { rms(&playback) } else { 0.0 };
                m.remote_rms = if remote { rms(&playback) } else { 0.0 };
                if monitor {
                    m.monitor_frames += 1;
                }
                if remote {
                    m.remote_frames += 1;
                }
            }
            let mut c = shared.control.lock().unwrap();
            c.status.level = level;
            c.status.raw_level = raw_level;
            c.status.input_clipping =
                clipped_at.is_some_and(|at| at.elapsed() < Duration::from_secs(1));
            c.status.gate_open = gate_open;
            c.status.buffer_interruptions = shared.buffer_interruptions.load(Ordering::Relaxed);
            c.status.processing_us = processing_us;
            if ready {
                c.status.captured_frames += 1;
            }
            c.status.ready = shared.synthetic || hardware.is_some();
            if last_status_event.elapsed() >= Duration::from_millis(100) {
                let _ = shared.events.send(Event::Audio(c.status.clone()));
                last_status_event = Instant::now();
            }
        }
        thread::sleep(Duration::from_millis(2));
    }
    drop(hardware);
    shared.flags.fetch_and(!15, Ordering::SeqCst);
    if shared.synthetic {
        let mut m = shared.metrics.lock().unwrap();
        m.active_capture = false;
        m.input_rms = 0.0;
        m.monitor_rms = 0.0;
        m.remote_rms = 0.0;
    }
}
pub fn valid_hotkey(key: u16) -> bool {
    (0x70..=0x7B).contains(&key) || [0x05, 0x06].contains(&key)
}
fn test_output(
    shared: &Arc<Shared>,
    settings: &AudioSettings,
    frequency: f32,
    duration_ms: u64,
) -> Result<(), String> {
    let count = duration_ms as usize * 48;
    let mut samples: Vec<f32> = (0..count)
        .map(|i| {
            let envelope = (i as f32 / 480.0).min(1.0) * ((count - 1 - i) as f32 / 480.0).min(1.0);
            (i as f32 * std::f32::consts::TAU * frequency / 48000.0).sin() * 0.1 * envelope
        })
        .collect();
    Equalizer::default().process(&mut samples, &settings.advanced.playback_eq);
    process_playback(&mut samples, settings.volume, settings.advanced.mix_limiter);
    let (errors, failures) = mpsc::sync_channel(1);
    let stream = if shared.synthetic {
        None
    } else {
        let host = cpal::default_host();
        let device = select_device(&host, false, &settings.output)?;
        let config = config(&device, false)?;
        if config.sample_rate() != 48000 {
            let mut converter = DeviceResampler::new(48000, config.sample_rate());
            let mut converted =
                Vec::with_capacity(duration_ms as usize * config.sample_rate() as usize / 1000);
            converter.process(&samples, 0, |packet| converted.extend_from_slice(packet));
            samples = converted;
        }
        let stream = match config.sample_format() {
            cpal::SampleFormat::F32 => {
                tone_stream::<f32>(&device, config.config(), samples, errors)
            }
            cpal::SampleFormat::F64 => {
                tone_stream::<f64>(&device, config.config(), samples, errors)
            }
            cpal::SampleFormat::I16 => {
                tone_stream::<i16>(&device, config.config(), samples, errors)
            }
            cpal::SampleFormat::I32 => {
                tone_stream::<i32>(&device, config.config(), samples, errors)
            }
            cpal::SampleFormat::I64 => {
                tone_stream::<i64>(&device, config.config(), samples, errors)
            }
            cpal::SampleFormat::U8 => tone_stream::<u8>(&device, config.config(), samples, errors),
            cpal::SampleFormat::U16 => {
                tone_stream::<u16>(&device, config.config(), samples, errors)
            }
            _ => unreachable!(),
        }
        .map_err(|e| format!("No se pudo abrir la salida: {e}"))?;
        stream
            .play()
            .map_err(|e| format!("No se pudo reproducir la prueba: {e}"))?;
        Some(stream)
    };
    let start = Instant::now();
    while start.elapsed() < Duration::from_millis(duration_ms + 100) {
        if let Ok(error) = failures.try_recv() {
            return Err(format!("Falló la prueba de salida: {error}"));
        }
        let c = shared.control.lock().unwrap();
        if c.shutdown || c.status.deafened {
            break;
        }
        drop(c);
        thread::sleep(Duration::from_millis(10));
    }
    drop(stream);
    Ok(())
}
fn tone_stream<T: cpal::SizedSample + cpal::FromSample<f32>>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    samples: Vec<f32>,
    errors: mpsc::SyncSender<cpal::Error>,
) -> Result<cpal::Stream, cpal::Error> {
    let channels = config.channels as usize;
    let mut index = 0;
    device.build_output_stream(
        config,
        move |output: &mut [T], _| {
            for frame in output.chunks_mut(channels) {
                let sample = samples.get(index).copied().unwrap_or(0.0);
                index += 1;
                for x in frame {
                    *x = T::from_sample(sample);
                }
            }
        },
        move |error| {
            if !matches!(
                error.kind(),
                cpal::ErrorKind::Xrun
                    | cpal::ErrorKind::DeviceChanged
                    | cpal::ErrorKind::RealtimeDenied
            ) {
                let _ = errors.try_send(error);
            }
        },
        None,
    )
}
pub fn hotkey_down(key: u16) -> bool {
    if !valid_hotkey(key) {
        return false;
    }
    #[cfg(windows)]
    {
        unsafe { windows_sys::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState(key as i32) < 0 }
    }
    #[cfg(not(windows))]
    {
        false
    }
}
fn enumerate() -> Result<(Vec<String>, Vec<String>), String> {
    let host = cpal::default_host();
    let names = |devices: Result<cpal::InputDevices<cpal::Devices>, cpal::Error>| {
        devices
            .map(|d| {
                d.filter_map(|d| d.description().ok().map(|d| d.name().to_owned()))
                    .collect()
            })
            .map_err(|e| format!("Windows no pudo listar los dispositivos de audio: {e}. Volvé a actualizar la lista."))
    };
    Ok((names(host.input_devices())?, names(host.output_devices())?))
}
fn select_device(host: &cpal::Host, input: bool, name: &str) -> Result<cpal::Device, String> {
    let device = if name.is_empty() {
        if input {
            host.default_input_device()
        } else {
            host.default_output_device()
        }
    } else {
        let devices = if input {
            host.input_devices()
        } else {
            host.output_devices()
        }
        .map_err(|e| format!("No se pudieron enumerar dispositivos: {e}"))?;
        devices
            .into_iter()
            .find(|d| d.description().is_ok_and(|d| d.name() == name))
    };
    device.ok_or_else(|| {
        format!(
            "No hay dispositivo de {} disponible. Elegí otro en Ajustes.",
            if input { "entrada" } else { "salida" }
        )
    })
}
fn config(device: &cpal::Device, input: bool) -> Result<cpal::SupportedStreamConfig, String> {
    let configs: Vec<_> = if input {
        device.supported_input_configs().map(|c| c.collect())
    } else {
        device.supported_output_configs().map(|c| c.collect())
    }
    .map_err(|e| format!("No se pudo consultar el dispositivo: {e}"))?;
    choose_config(configs).ok_or_else(|| format!(
        "La {} «{}» no ofrece un formato compatible entre 8 y 192 kHz. Elegí otro dispositivo o revisá su formato en Sonido de Windows.",
        if input { "entrada" } else { "salida" },
        device.description().map(|d| d.name().to_owned()).unwrap_or_default()
    ))
}
fn choose_config(
    configs: Vec<cpal::SupportedStreamConfigRange>,
) -> Option<cpal::SupportedStreamConfig> {
    configs
        .into_iter()
        .filter(|c| {
            matches!(
                c.sample_format(),
                cpal::SampleFormat::F32
                    | cpal::SampleFormat::F64
                    | cpal::SampleFormat::I16
                    | cpal::SampleFormat::I32
                    | cpal::SampleFormat::U16
                    | cpal::SampleFormat::U8
                    | cpal::SampleFormat::I64
            )
        })
        .filter_map(|c| {
            let min = c.min_sample_rate().max(8000);
            let max = c.max_sample_rate().min(192000);
            (min <= max)
                .then(|| c.try_with_sample_rate(48000u32.clamp(min, max)))
                .flatten()
        })
        .min_by_key(|c| {
            (
                c.sample_rate().abs_diff(48000),
                c.channels(),
                match c.sample_format() {
                    cpal::SampleFormat::F32 => 0,
                    cpal::SampleFormat::I16 => 1,
                    _ => 2,
                },
            )
        })
}
fn open_hardware(
    shared: &Arc<Shared>,
    settings: &AudioSettings,
    require_input: bool,
) -> Result<Hardware, String> {
    let generation = shared.hardware_generation.fetch_add(1, Ordering::SeqCst) + 1;
    let host = cpal::default_host();
    let output = select_device(&host, false, &settings.output)?;
    let output_name = output
        .description()
        .map(|d| d.name().to_owned())
        .unwrap_or_default();
    let oc = config(&output, false)?;
    let output_rate = oc.sample_rate();
    let (producer, consumer) = rtrb::RingBuffer::new(SAMPLES * 4);
    let (playback, sink) = rtrb::RingBuffer::new((output_rate as usize / 50 + 512) * 4);
    let opened_input = (|| {
        if shared.input_unavailable.load(Ordering::SeqCst) {
            return Err("La entrada está detenida. Volvé a entrar a voz o iniciá una prueba para reintentar.".into());
        }
        let input = select_device(&host, true, &settings.input)?;
        let input_name = input
            .description()
            .map(|d| d.name().to_owned())
            .unwrap_or_default();
        let ic = config(&input, true)?;
        let i = match ic.sample_format() {
            cpal::SampleFormat::F32 => {
                input_stream::<f32>(&input, ic.config(), producer, shared.clone())
            }
            cpal::SampleFormat::I16 => {
                input_stream::<i16>(&input, ic.config(), producer, shared.clone())
            }
            cpal::SampleFormat::U16 => {
                input_stream::<u16>(&input, ic.config(), producer, shared.clone())
            }
            cpal::SampleFormat::I32 => {
                input_stream::<i32>(&input, ic.config(), producer, shared.clone())
            }
            cpal::SampleFormat::F64 => {
                input_stream::<f64>(&input, ic.config(), producer, shared.clone())
            }
            cpal::SampleFormat::U8 => {
                input_stream::<u8>(&input, ic.config(), producer, shared.clone())
            }
            cpal::SampleFormat::I64 => {
                input_stream::<i64>(&input, ic.config(), producer, shared.clone())
            }
            _ => unreachable!(),
        }
        .map_err(|e| {
            format!("No se pudo abrir el micrófono. Revisá permisos y dispositivo: {e}")
        })?;
        i.play()
            .map_err(|e| format!("No se pudo iniciar el micrófono: {e}"))?;
        Ok::<_, String>((
            i,
            format!("{input_name} · {} Hz → 48000 Hz", ic.sample_rate()),
        ))
    })();
    let (i, input_name, input_error) = match opened_input {
        Ok((input, name)) => (Some(input), name, None),
        Err(error) if !require_input => {
            (None, String::new(), Some(format!("Modo escucha: {error}")))
        }
        Err(error) => return Err(error),
    };
    let o = match oc.sample_format() {
        cpal::SampleFormat::F32 => output_stream::<f32>(&output, oc.config(), sink, shared.clone()),
        cpal::SampleFormat::I16 => output_stream::<i16>(&output, oc.config(), sink, shared.clone()),
        cpal::SampleFormat::U16 => output_stream::<u16>(&output, oc.config(), sink, shared.clone()),
        cpal::SampleFormat::I32 => output_stream::<i32>(&output, oc.config(), sink, shared.clone()),
        cpal::SampleFormat::F64 => output_stream::<f64>(&output, oc.config(), sink, shared.clone()),
        cpal::SampleFormat::U8 => output_stream::<u8>(&output, oc.config(), sink, shared.clone()),
        cpal::SampleFormat::I64 => output_stream::<i64>(&output, oc.config(), sink, shared.clone()),
        _ => unreachable!(),
    }
    .map_err(|e| format!("No se pudo abrir la salida de audio: {e}"))?;
    o.play()
        .map_err(|e| format!("No se pudo iniciar la salida: {e}"))?;
    Ok(Hardware {
        shared: shared.clone(),
        generation,
        _input: i,
        input_error,
        _output: o,
        capture: consumer,
        playback,
        input_name,
        output_name: format!("{output_name} · 48000 Hz → {output_rate} Hz"),
        output_rate,
        output_resampler: DeviceResampler::new(48000, output_rate),
    })
}
fn input_stream<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    mut producer: rtrb::Producer<(f32, u64)>,
    shared: Arc<Shared>,
) -> Result<cpal::Stream, cpal::Error>
where
    T: cpal::SizedSample,
    f32: cpal::FromSample<T>,
{
    let channels = config.channels as usize;
    let mut converter = DeviceResampler::new(config.sample_rate, 48000);
    let failure = shared.clone();
    let generation = shared.hardware_generation.load(Ordering::SeqCst);
    device.build_input_stream(
        config,
        move |data: &[T], _| {
            let epoch = shared.flags.load(Ordering::SeqCst);
            if epoch & CAPTURE == 0 {
                return;
            }
            let mut mono = [0.0; 256];
            for chunk in data.chunks(channels * mono.len()) {
                let mut count = 0;
                for frame in chunk.chunks_exact(channels) {
                    mono[count] = frame
                        .iter()
                        .map(|s| <f32 as cpal::Sample>::from_sample(*s))
                        .sum::<f32>()
                        / channels as f32;
                    count += 1;
                }
                converter.process(&mono[..count], epoch, |packet| {
                    for sample in packet {
                        let _ = producer.push((*sample, epoch));
                    }
                });
            }
        },
        move |error| {
            failure.device_error(generation, true, error);
        },
        Some(Duration::from_secs(2)),
    )
}
fn output_stream<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    mut sink: rtrb::Consumer<(f32, u64)>,
    shared: Arc<Shared>,
) -> Result<cpal::Stream, cpal::Error>
where
    T: cpal::SizedSample + cpal::FromSample<f32>,
{
    let channels = config.channels as usize;
    let failure = shared.clone();
    let generation = shared.hardware_generation.load(Ordering::SeqCst);
    device.build_output_stream(
        config,
        move |data: &mut [T], _| {
            for frame in data.chunks_mut(channels) {
                let epoch = shared.flags.load(Ordering::SeqCst);
                let value = playback_sample(sink.pop().ok(), epoch);
                frame.fill(T::from_sample(value));
            }
        },
        move |error| {
            failure.device_error(generation, false, error);
        },
        Some(Duration::from_secs(2)),
    )
}

fn playback_sample(sample: Option<(f32, u64)>, epoch: u64) -> f32 {
    match sample {
        Some((value, produced)) if produced == epoch && epoch & (RECEIVE | TEST) != 0 => value,
        _ => 0.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};
    #[test]
    fn muted_room_transition_discards_queued_receive_without_waiting_for_queue_space() {
        let (source, mut packets) = engine();
        let (sink, _) = engine();
        source.command(Command::Join).unwrap();
        sink.command(Command::Join).unwrap();
        sink.command(Command::Mute).unwrap();
        let mut captured = Vec::new();
        wait_for(|| {
            while let Ok(packet) = packets.try_recv() {
                captured.push(packet);
            }
            captured.len() >= 16
        });
        for packet in &captured {
            sink.incoming(9, packet.clone());
        }
        wait_for(|| sink.synthetic_metrics().unwrap().remote_rms > 0.01);
        for packet in &captured {
            sink.incoming(9, packet.clone());
        }
        sink.set_transport_ready(false);
        sink.set_transport_ready(true);
        let before = sink.synthetic_metrics().unwrap().remote_frames;
        thread::sleep(Duration::from_millis(100));
        let after = sink.synthetic_metrics().unwrap();
        assert!(
            after.remote_frames <= before + 1,
            "old room frames survived transition"
        );
        assert_eq!(after.remote_rms, 0.0);
        assert!(sink.status().muted && !sink.outgoing_allowed());
    }
    #[test]
    fn transport_transition_blocks_publication_without_changing_user_controls() {
        let (audio, mut packets) = engine();
        audio.command(Command::Join).unwrap();
        wait_for(|| packets.try_recv().is_ok());
        let previous = audio.status();
        audio.set_transport_ready(false);
        assert!(!audio.outgoing_allowed());
        let gated = audio.status();
        assert_eq!(
            (
                gated.calling,
                gated.muted,
                gated.deafened,
                gated.push_to_talk_active
            ),
            (
                previous.calling,
                previous.muted,
                previous.deafened,
                previous.push_to_talk_active
            )
        );
        while packets.try_recv().is_ok() {}
        std::thread::sleep(Duration::from_millis(60));
        assert!(packets.try_recv().is_err());
        assert!(audio.synthetic_metrics().unwrap().active_capture);
        audio.set_transport_ready(true);
        wait_for(|| packets.try_recv().is_ok());
        audio.command(Command::Mute).unwrap();
        audio.set_transport_ready(false);
        audio.set_transport_ready(true);
        assert!(audio.status().muted && !audio.outgoing_allowed());
    }
    #[test]
    fn reordered_and_lost_packets_are_bounded_and_concealed_without_reopening_send() {
        let (source, mut packets) = engine();
        let (sink, mut outgoing) = engine();
        let mut settings = AudioSettings::default();
        settings.advanced.opus_fec = true;
        settings.advanced.packet_loss_percent = 20;
        settings.advanced.bitrate = 32000;
        source.command(Command::AudioSettings(settings)).unwrap();
        source.command(Command::Join).unwrap();
        sink.command(Command::Join).unwrap();
        sink.command(Command::Mute).unwrap();
        wait_for(|| sink.status().ready);
        let mut captured = Vec::new();
        wait_for(|| {
            while let Ok(packet) = packets.try_recv() {
                captured.push(packet);
            }
            captured.len() >= 8
        });
        // Four starts in order, one adjacent reordering and one missing 20 ms frame.
        for index in [0, 2, 1, 3, 5, 6, 7] {
            sink.incoming(9, captured[index].clone());
        }
        wait_for(|| sink.synthetic_metrics().unwrap().concealed_frames > 0);
        let metrics = sink.synthetic_metrics().unwrap();
        assert!(metrics.remote_frames > 0 && metrics.remote_rms.is_finite());
        assert!(metrics.reordered_packets > 0);
        assert!(metrics.fec_attempts > 0);
        assert!(outgoing.try_recv().is_err() && !sink.outgoing_allowed());
        sink.command(Command::Deafen).unwrap();
        assert!(!sink.outgoing_allowed());
        wait_for(|| sink.synthetic_metrics().unwrap().remote_rms == 0.0);
    }
    #[test]
    fn listener_without_microphone_receives_without_capture_or_publication() {
        let (events, _) = mpsc::channel();
        let (sink, mut outgoing) =
            AudioEngine::spawn_synthetic_without_input(events, AudioSettings::default()).unwrap();
        let (source, mut packets) = engine();
        source.command(Command::Join).unwrap();
        sink.command(Command::Join).unwrap();
        wait_for(|| {
            while let Ok(packet) = packets.try_recv() {
                sink.incoming(9, packet);
            }
            sink.synthetic_metrics().unwrap().remote_rms > 0.01
        });
        assert!(sink.status().calling);
        assert!(!sink.outgoing_allowed());
        assert!(!sink.synthetic_metrics().unwrap().active_capture);
        assert_eq!(sink.synthetic_metrics().unwrap().captured_frames, 0);
        assert!(outgoing.try_recv().is_err());
        assert!(sink.command(Command::StartTest).is_err());
        sink.command(Command::Mute).unwrap();
        sink.command(Command::Mute).unwrap();
        assert!(!sink.outgoing_allowed());
        sink.command(Command::Deafen).unwrap();
        assert!(!sink.outgoing_allowed());
        sink.command(Command::Disconnect).unwrap();
        assert!(!sink.status().calling);
    }
    #[test]
    fn buffer_discontinuities_do_not_stop_private_monitor_or_call() {
        let (audio, mut packets) = engine();
        audio.command(Command::StartTest).unwrap();
        wait_for(|| audio.synthetic_metrics().unwrap().monitor_frames > 1);
        let before = audio.synthetic_metrics().unwrap().monitor_frames;
        for _ in 0..20 {
            audio
                .shared
                .device_error(0, true, cpal::ErrorKind::Xrun.into());
        }
        wait_for(|| audio.synthetic_metrics().unwrap().monitor_frames > before + 2);
        assert!(audio.status().testing && audio.status().error.is_none());
        assert_eq!(
            audio.shared.buffer_interruptions.load(Ordering::Relaxed),
            20
        );
        assert!(!audio.outgoing_allowed() && packets.try_recv().is_err());
        audio.command(Command::StopTest).unwrap();
        audio.command(Command::Join).unwrap();
        wait_for(|| packets.try_recv().is_ok());
        audio
            .shared
            .device_error(0, false, cpal::ErrorKind::Xrun.into());
        wait_for(|| packets.try_recv().is_ok());
        assert!(audio.status().calling && audio.status().error.is_none());
        audio
            .shared
            .device_error(0, true, cpal::ErrorKind::DeviceNotAvailable.into());
        wait_for(|| audio.status().error.is_some());
        assert!(audio.status().calling);
        assert!(!audio.outgoing_allowed());
    }
    #[test]
    fn voice_gate_respects_threshold_and_release_without_hiding_input_meter() {
        let mut processor = MicrophoneProcessor::default();
        let mut settings = AudioSettings::default();
        settings.advanced.voice_activation = true;
        settings.advanced.threshold_db = -40.0;
        settings.advanced.release_ms = 40;
        let mut quiet = [0.001; SAMPLES];
        let (level, open) = processor.process(&mut quiet, &settings);
        assert!(!open && level > 0.0009 && quiet.iter().all(|s| *s == 0.0));
        assert!(processor.process(&mut [0.1; SAMPLES], &settings).1);
        assert!(processor.process(&mut [0.0; SAMPLES], &settings).1);
        assert!(processor.process(&mut [0.0; SAMPLES], &settings).1);
        assert!(!processor.process(&mut [0.0; SAMPLES], &settings).1);
    }
    #[test]
    fn high_pass_removes_dc_while_preserving_voice_band() {
        let mut settings = AudioSettings::default();
        settings.advanced.high_pass = true;
        let mut processor = MicrophoneProcessor::default();
        let mut last = 1.0;
        for _ in 0..10 {
            last = processor.process(&mut [0.25; SAMPLES], &settings).0;
        }
        assert!(last < 0.0001);
        let mut voice = [0.0; SAMPLES];
        for (i, sample) in voice.iter_mut().enumerate() {
            *sample = (std::f32::consts::TAU * 440.0 * i as f32 / 48000.0).sin() * 0.25;
        }
        assert!(processor.process(&mut voice, &settings).0 > 0.15);
    }
    #[test]
    fn output_peak_protection_preserves_mix_shape_and_quiet_audio() {
        let mut mixed = [2.0, 1.0, -2.0, 0.0];
        process_playback(&mut mixed, 1.0, true);
        assert_eq!(mixed, [1.0, 0.5, -1.0, 0.0]);
        let mut quiet = [0.2, -0.4];
        process_playback(&mut quiet, 0.5, true);
        assert_eq!(quiet, [0.1, -0.2]);
    }
    #[test]
    fn advanced_private_monitor_settings_change_signal_without_publishing() {
        let (audio, mut packets) = engine();
        audio.command(Command::StartTest).unwrap();
        wait_for(|| audio.synthetic_metrics().unwrap().monitor_rms > 0.1);
        let mut settings = AudioSettings::default();
        settings.advanced.monitor_volume = 0.0;
        audio
            .command(Command::AudioSettings(settings.clone()))
            .unwrap();
        wait_for(|| {
            let m = audio.synthetic_metrics().unwrap();
            m.monitor_rms == 0.0 && m.input_rms > 0.1
        });
        settings.advanced.monitor_volume = 1.0;
        settings.advanced.voice_activation = true;
        settings.advanced.threshold_db = -12.0;
        audio
            .command(Command::AudioSettings(settings.clone()))
            .unwrap();
        wait_for(|| {
            !audio.status().gate_open && audio.synthetic_metrics().unwrap().monitor_rms == 0.0
        });
        settings.advanced.threshold_db = -45.0;
        settings.advanced.bitrate = 64000;
        audio.command(Command::AudioSettings(settings)).unwrap();
        wait_for(|| {
            audio.status().gate_open && audio.synthetic_metrics().unwrap().monitor_rms > 0.1
        });
        assert!(!audio.outgoing_allowed());
        assert!(packets.try_recv().is_err());
    }
    #[test]
    fn invalid_advanced_audio_values_are_rejected() {
        let mut settings = AudioSettings::default();
        settings.advanced.threshold_db = f32::NAN;
        assert!(validate(&settings).is_err());
        settings.advanced = Default::default();
        settings.advanced.bitrate = 12345;
        assert!(validate(&settings).is_err());
        settings.advanced = Default::default();
        settings.advanced.monitor_volume = 1.1;
        assert!(validate(&settings).is_err());
    }
    #[test]
    fn errors_from_retired_streams_cannot_stop_replacement_devices() {
        let (audio, packets) = engine();
        audio.command(Command::Shutdown).unwrap();
        wait_for(|| packets.is_closed());
        let shared = &audio.shared;
        shared.hardware_generation.store(10, Ordering::SeqCst);
        shared.stream_error(10, "old microphone".into());
        shared.hardware_generation.store(11, Ordering::SeqCst);
        assert!(shared.take_stream_error().is_none());
        shared.stream_error(10, "late callback".into());
        assert!(shared.take_stream_error().is_none());
        shared.stream_error(11, "new output failed: device removed".into());
        assert_eq!(
            shared
                .take_stream_error()
                .map(|(_, message)| message)
                .as_deref(),
            Some("new output failed: device removed")
        );
    }
    #[test]
    fn device_config_prefers_48khz_and_accepts_44100_and_96000() {
        let range = |format, rate| {
            cpal::SupportedStreamConfigRange::new(
                1,
                rate,
                rate,
                cpal::SupportedBufferSize::Unknown,
                format,
            )
        };
        let i32_config = choose_config(vec![range(cpal::SampleFormat::I32, 48000)]).unwrap();
        assert_eq!(i32_config.sample_format(), cpal::SampleFormat::I32);
        assert_eq!(
            choose_config(vec![range(cpal::SampleFormat::F32, 44100)])
                .unwrap()
                .sample_rate(),
            44100
        );
        assert_eq!(
            choose_config(vec![range(cpal::SampleFormat::F32, 96000)])
                .unwrap()
                .sample_rate(),
            96000
        );
        let preferred = choose_config(vec![
            range(cpal::SampleFormat::I32, 48000),
            range(cpal::SampleFormat::F32, 48000),
        ])
        .unwrap();
        assert_eq!(preferred.sample_format(), cpal::SampleFormat::F32);
    }
    #[test]
    fn hardware_resampling_preserves_voice_and_discards_previous_gate_samples() {
        for rate in [44100, 96000] {
            let mut converter = DeviceResampler::new(rate, 48000);
            let input: Vec<_> = (0..rate / 5)
                .map(|n| (std::f32::consts::TAU * 1000.0 * n as f32 / rate as f32).sin() * 0.25)
                .collect();
            let mut output = Vec::new();
            for chunk in input.chunks(257) {
                converter.process(chunk, 16 | CAPTURE, |packet| {
                    output.extend_from_slice(packet)
                });
            }
            assert!(
                output.len() > 9000 && output.len() <= 9600,
                "rate {rate}: {}",
                output.len()
            );
            let steady = &output[1000..];
            assert!((rms(steady) - 0.1768).abs() < 0.01);
            let crossings = steady
                .windows(2)
                .filter(|w| w[0] <= 0.0 && w[1] > 0.0)
                .count();
            let frequency = crossings as f32 * 48000.0 / steady.len() as f32;
            assert!((frequency - 1000.0).abs() < 10.0, "{frequency}");
            let mut after_gate = Vec::new();
            converter.process(&vec![0.0; rate as usize / 10], 32 | CAPTURE, |packet| {
                after_gate.extend_from_slice(packet)
            });
            assert!(
                after_gate.iter().all(|sample| sample.abs() < 0.00001),
                "old gate leaked"
            );
        }
    }
    #[test]
    fn playback_discards_samples_from_before_private_test_or_deafen() {
        let remote = 16 | RECEIVE;
        let test = 32 | TEST;
        assert_eq!(playback_sample(Some((0.5, remote)), remote), 0.5);
        assert_eq!(playback_sample(Some((0.5, remote)), test), 0.0);
        assert_eq!(playback_sample(Some((0.5, test)), remote), 0.0);
        assert_eq!(playback_sample(Some((0.5, remote)), 48), 0.0);
    }
    fn engine() -> (AudioHandle, tokio::sync::mpsc::Receiver<Vec<u8>>) {
        let (events, _) = std::sync::mpsc::channel();
        AudioEngine::spawn_synthetic(events, AudioSettings::default()).unwrap()
    }
    fn wait_for(mut condition: impl FnMut() -> bool) {
        let start = Instant::now();
        while !condition() {
            assert!(start.elapsed() < Duration::from_secs(3), "audio timeout");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    #[test]
    fn synthetic_call_emits_decodable_mono_opus() {
        let (audio, mut packets) = engine();
        assert!(!audio.outgoing_allowed());
        audio.command(Command::Join).unwrap();
        let mut packet = None;
        wait_for(|| {
            packet = packets.try_recv().ok();
            packet.is_some()
        });
        let mut decoder = opus::Decoder::new(48000, opus::Channels::Mono).unwrap();
        let mut pcm = [0.0; 960];
        assert_eq!(
            decoder
                .decode_float(&packet.unwrap()[8..], &mut pcm, false)
                .unwrap(),
            960
        );
        assert!(pcm.iter().any(|v| v.abs() > 0.01));
        audio.command(Command::Shutdown).unwrap();
    }
    #[test]
    fn private_monitor_never_publishes_and_restores_mute() {
        let (audio, mut packets) = engine();
        audio.command(Command::Join).unwrap();
        wait_for(|| packets.try_recv().is_ok());
        audio.command(Command::StartTest).unwrap();
        assert!(!audio.outgoing_allowed());
        while packets.try_recv().is_ok() {}
        wait_for(|| audio.synthetic_metrics().unwrap().monitor_frames >= 3);
        assert!(audio.synthetic_metrics().unwrap().monitor_rms > 0.1);
        let encoded = audio.synthetic_metrics().unwrap().encoded_frames;
        audio.command(Command::Mute).unwrap();
        std::thread::sleep(Duration::from_millis(100));
        assert!(!audio.outgoing_allowed());
        assert_eq!(audio.synthetic_metrics().unwrap().encoded_frames, encoded);
        assert!(packets.try_recv().is_err());
        audio.command(Command::StopTest).unwrap();
        assert!(!audio.status().muted);
        assert!(audio.outgoing_allowed());
        audio.command(Command::Mute).unwrap();
        audio.command(Command::StartTest).unwrap();
        audio.command(Command::Mute).unwrap();
        audio.command(Command::StopTest).unwrap();
        assert!(audio.status().muted);
        assert!(!audio.outgoing_allowed());
        audio.command(Command::Shutdown).unwrap();
    }
    #[test]
    fn deafen_stops_monitor_and_capture_without_changing_logical_mute() {
        let (audio, _) = engine();
        audio.command(Command::Join).unwrap();
        audio.command(Command::Mute).unwrap();
        audio.command(Command::StartTest).unwrap();
        wait_for(|| audio.synthetic_metrics().unwrap().monitor_frames > 0);
        audio.command(Command::Deafen).unwrap();
        assert!(!audio.status().testing);
        assert!(audio.status().muted);
        assert!(!audio.outgoing_allowed());
        assert!(audio.command(Command::StartTest).is_err());
        wait_for(|| !audio.synthetic_metrics().unwrap().active_capture);
        audio.command(Command::Deafen).unwrap();
        assert!(audio.status().muted);
        assert!(!audio.status().testing);
        audio.command(Command::Leave).unwrap();
        wait_for(|| !audio.synthetic_metrics().unwrap().active_capture);
        assert!(!audio.status().calling);
        audio.command(Command::Shutdown).unwrap();
    }
    #[test]
    fn input_gain_and_app_volume_change_private_monitor_signal() {
        let (audio, _) = engine();
        audio.command(Command::StartTest).unwrap();
        wait_for(|| audio.synthetic_metrics().unwrap().monitor_rms > 0.1);
        let mut settings = AudioSettings {
            input_level: 0.5,
            volume: 0.5,
            ..Default::default()
        };
        audio
            .command(Command::AudioSettings(settings.clone()))
            .unwrap();
        wait_for(|| {
            let m = audio.synthetic_metrics().unwrap();
            m.input_rms > 0.08 && m.input_rms < 0.10 && m.monitor_rms > 0.04 && m.monitor_rms < 0.05
        });
        assert!(
            audio.status().raw_level > 0.16,
            "diagnostic raw level precedes app gain and filters"
        );
        settings.volume = 0.0;
        audio
            .command(Command::AudioSettings(settings.clone()))
            .unwrap();
        wait_for(|| audio.synthetic_metrics().unwrap().monitor_rms == 0.0);
        settings.volume = 1.0;
        settings.input_level = 0.0;
        audio.command(Command::AudioSettings(settings)).unwrap();
        wait_for(|| audio.synthetic_metrics().unwrap().input_rms == 0.0);
        assert!(
            audio.status().raw_level > 0.16,
            "zero app gain does not hide a working physical input in diagnostics"
        );
        audio.command(Command::Shutdown).unwrap();
    }
    #[test]
    fn remote_opus_plays_and_deafen_silences_it() {
        let (source, mut packets) = engine();
        let (sink, _) = engine();
        source.command(Command::Join).unwrap();
        sink.command(Command::Join).unwrap();
        wait_for(|| {
            while let Ok(packet) = packets.try_recv() {
                sink.incoming(9, packet);
            }
            let metrics = sink.synthetic_metrics().unwrap();
            metrics.remote_frames > 3 && metrics.remote_rms > 0.01
        });
        sink.command(Command::ParticipantAudio(
            crate::contracts::ParticipantAudio {
                id: 9,
                volume: 1.0,
                muted: true,
            },
        ))
        .unwrap();
        let before = sink.synthetic_metrics().unwrap().remote_frames;
        wait_for(|| {
            while let Ok(packet) = packets.try_recv() {
                sink.incoming(9, packet);
            }
            let metrics = sink.synthetic_metrics().unwrap();
            metrics.remote_frames > before + 2 && metrics.remote_rms == 0.0
        });
        assert!(
            source.outgoing_allowed(),
            "local mute does not mute the sender"
        );
        sink.command(Command::ParticipantAudio(
            crate::contracts::ParticipantAudio {
                id: 9,
                volume: 0.2,
                muted: false,
            },
        ))
        .unwrap();
        wait_for(|| {
            while let Ok(packet) = packets.try_recv() {
                sink.incoming(9, packet);
            }
            let level = sink.synthetic_metrics().unwrap().remote_rms;
            level > 0.01 && level < 0.05
        });
        sink.command(Command::Deafen).unwrap();
        let count = sink.synthetic_metrics().unwrap().remote_frames;
        std::thread::sleep(Duration::from_millis(80));
        while let Ok(packet) = packets.try_recv() {
            sink.incoming(9, packet);
        }
        std::thread::sleep(Duration::from_millis(80));
        assert_eq!(sink.synthetic_metrics().unwrap().remote_frames, count);
        assert_eq!(sink.synthetic_metrics().unwrap().remote_rms, 0.0);
        source.command(Command::Shutdown).unwrap();
        sink.command(Command::Shutdown).unwrap();
    }
    #[test]
    fn ptt_join_muted_and_private_output_test_keep_publication_closed() {
        let (audio, mut packets) = engine();
        let mut settings = AudioSettings::default();
        settings.advanced.join_muted = true;
        settings.advanced.push_to_talk = true;
        audio.command(Command::AudioSettings(settings)).unwrap();
        audio.command(Command::Join).unwrap();
        assert!(audio.status().muted && !audio.outgoing_allowed());
        audio.command(Command::Mute).unwrap();
        {
            let mut c = audio.shared.control.lock().unwrap();
            c.status.push_to_talk_active = true;
            audio.shared.publish(&c);
            assert!(audio.outgoing_allowed());
            c.status.push_to_talk_active = false;
            audio.shared.publish(&c);
            assert!(!audio.outgoing_allowed());
        }
        audio.command(Command::Disconnect).unwrap();
        audio.command(Command::TestSound).unwrap();
        std::thread::sleep(Duration::from_millis(60));
        assert!(audio.status().sound_testing);
        assert!(!audio.synthetic_metrics().unwrap().active_capture && !audio.outgoing_allowed());
        assert!(packets.try_recv().is_err());
        audio.command(Command::Shutdown).unwrap();
        wait_for(|| !audio.status().sound_testing);
    }
    #[test]
    fn device_change_ends_test_and_invalid_levels_are_rejected() {
        let (audio, _) = engine();
        audio.command(Command::Mute).unwrap();
        audio.command(Command::StartTest).unwrap();
        audio
            .command(Command::AudioSettings(AudioSettings {
                input: "Otra entrada".into(),
                ..Default::default()
            }))
            .unwrap();
        assert!(!audio.status().testing);
        assert!(audio.status().muted);
        wait_for(|| !audio.synthetic_metrics().unwrap().active_capture);
        assert!(
            audio
                .command(Command::AudioSettings(AudioSettings {
                    volume: f32::NAN,
                    ..Default::default()
                }))
                .is_err()
        );
        audio.command(Command::Shutdown).unwrap();
        assert!(!audio.outgoing_allowed());
        assert!(audio.command(Command::Join).is_err());
    }
    #[test]
    fn dropping_final_handle_closes_worker_and_packet_channel() {
        let (audio, mut packets) = engine();
        audio.command(Command::Join).unwrap();
        wait_for(|| packets.try_recv().is_ok());
        let clone = audio.clone();
        drop(audio);
        assert!(clone.outgoing_allowed());
        drop(clone);
        wait_for(|| {
            while packets.try_recv().is_ok() {}
            packets.is_closed()
        });
    }
    #[test]
    fn idle_audio_emits_no_periodic_status_and_opens_no_capture() {
        let (events, updates) = mpsc::channel();
        let (audio, _) = AudioEngine::spawn_synthetic(events, AudioSettings::default()).unwrap();
        updates.recv_timeout(Duration::from_secs(1)).unwrap();
        std::thread::sleep(Duration::from_millis(80));
        while updates.try_recv().is_ok() {}
        assert!(updates.recv_timeout(Duration::from_millis(150)).is_err());
        let metrics = audio.synthetic_metrics().unwrap();
        assert!(!metrics.active_capture);
        assert_eq!(metrics.captured_frames, 0);
        audio.command(Command::Shutdown).unwrap();
    }
}
