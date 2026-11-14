use crate::contracts::{AudioSettings, AudioStatus, Command, Event};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::{
    collections::{HashMap, VecDeque},
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
    incoming: mpsc::SyncSender<(u64, Option<Vec<u8>>)>,
}
struct HandleLifetime(Arc<Shared>);
struct Control {
    status: AudioStatus,
    prior_mute: Option<bool>,
    revision: u64,
    shutdown: bool,
}
struct Shared {
    control: Mutex<Control>,
    // Low four bits are gates; upper bits identify the capture generation atomically.
    flags: AtomicU64,
    failed: AtomicBool,
    events: mpsc::Sender<Event>,
    synthetic: bool,
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
}
impl Shared {
    fn publish(&self, c: &Control) {
        let s = &c.status;
        let active = !c.shutdown && !s.deafened && (s.calling || s.testing);
        let flags = if active {
            CAPTURE
                | if s.testing {
                    TEST
                } else {
                    RECEIVE | if s.muted { 0 } else { SEND }
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
    fn spawn_with_mode(
        events: mpsc::Sender<Event>,
        settings: AudioSettings,
        synthetic: bool,
    ) -> Result<(AudioHandle, tokio::sync::mpsc::Receiver<Vec<u8>>), String> {
        validate(&settings)?;
        let encoder = opus::Encoder::new(48000, opus::Channels::Mono, opus::Application::Voip)
            .map_err(|e| format!("No se pudo iniciar Opus: {e}"))?;
        let shared = Arc::new(Shared {
            control: Mutex::new(Control {
                status: AudioStatus {
                    settings,
                    ..Default::default()
                },
                prior_mute: None,
                revision: 0,
                shutdown: false,
            }),
            flags: AtomicU64::new(0),
            failed: AtomicBool::new(false),
            events,
            synthetic,
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
fn validate(s: &AudioSettings) -> Result<(), String> {
    if !s.input_level.is_finite()
        || !(0.0..=1.0).contains(&s.input_level)
        || !s.volume.is_finite()
        || !(0.0..=1.0).contains(&s.volume)
    {
        return Err("Nivel y volumen deben estar entre 0 y 100%.".into());
    }
    Ok(())
}
impl AudioHandle {
    pub fn command(&self, command: Command) -> Result<(), String> {
        if let Command::AudioSettings(s) = &command {
            validate(s)?;
        }
        let mut c = self.shared.control.lock().unwrap();
        if c.shutdown {
            return Err("El motor de audio está cerrado.".into());
        }
        match command {
            Command::Join => c.status.calling = true,
            Command::Leave | Command::Disconnect | Command::Shutdown => {
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
                    c.status.testing = false;
                    if let Some(prior) = c.prior_mute.take() {
                        c.status.muted = prior;
                    }
                }
                c.status.settings = settings;
            }
            Command::RefreshDevices => {}
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
            let _ = self.incoming.try_send((peer, Some(packet)));
        }
    }
    pub fn remove_peer(&self, peer: u64) {
        let _ = self.incoming.try_send((peer, None));
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
    _input: cpal::Stream,
    _output: cpal::Stream,
    capture: rtrb::Consumer<(f32, u64)>,
    playback: rtrb::Producer<f32>,
}
struct Peer {
    decoder: opus::Decoder,
    frames: VecDeque<[f32; SAMPLES]>,
}
fn rms(samples: &[f32]) -> f32 {
    (samples.iter().map(|x| x * x).sum::<f32>() / samples.len() as f32).sqrt()
}
fn run(
    shared: Arc<Shared>,
    incoming: mpsc::Receiver<(u64, Option<Vec<u8>>)>,
    outgoing: tokio::sync::mpsc::Sender<Vec<u8>>,
    mut encoder: opus::Encoder,
) {
    let mut hardware: Option<Hardware> = None;
    let mut peers = HashMap::<u64, Peer>::new();
    let mut previous_devices = None;
    let mut revision = u64::MAX;
    let mut phase = 0.0f32;
    let mut tick = Instant::now();
    let mut packet = [0u8; 1200];
    let mut last_flags = 0;
    loop {
        if shared.failed.swap(false, Ordering::SeqCst) {
            hardware = None;
            shared.error(
                "El dispositivo de audio falló. Revisá la conexión y volvé a entrar a la llamada."
                    .into(),
            );
        }
        let (status, current_revision, shutdown) = {
            let c = shared.control.lock().unwrap();
            (c.status.clone(), c.revision, c.shutdown)
        };
        if shutdown {
            break;
        }
        let flags = shared.flags.load(Ordering::SeqCst);
        let epoch = flags;
        if current_revision != revision {
            revision = current_revision;
            let devices = (
                status.settings.input.clone(),
                status.settings.output.clone(),
            );
            if previous_devices.as_ref() != Some(&devices) {
                hardware = None;
                previous_devices = Some(devices);
            }
            let (inputs, outputs) = if shared.synthetic {
                (
                    vec!["Entrada sintética 440 Hz".into()],
                    vec!["Salida sintética".into()],
                )
            } else {
                enumerate()
            };
            let mut c = shared.control.lock().unwrap();
            c.status.inputs = inputs;
            c.status.outputs = outputs;
            let _ = shared.events.send(Event::Audio(c.status.clone()));
        }
        if flags & CAPTURE == 0 {
            hardware = None;
        } else if !shared.synthetic && hardware.is_none() {
            match open_hardware(&shared, &status.settings) {
                Ok(h) => hardware = Some(h),
                Err(e) => {
                    shared.error(e);
                    continue;
                }
            }
        }
        if flags & 15 != last_flags {
            peers.clear();
            while incoming.try_recv().is_ok() {}
            if let Some(h) = &mut hardware {
                while h.capture.pop().is_ok() {}
            }
            last_flags = flags & 15;
        }
        for _ in 0..64 {
            let Ok((id, bytes)) = incoming.try_recv() else {
                break;
            };
            let Some(bytes) = bytes else {
                peers.remove(&id);
                continue;
            };
            if flags & RECEIVE == 0 || (peers.len() >= 32 && !peers.contains_key(&id)) {
                continue;
            }
            let peer = peers.entry(id).or_insert_with(|| Peer {
                decoder: opus::Decoder::new(48000, opus::Channels::Mono)
                    .expect("valid Opus configuration"),
                frames: VecDeque::with_capacity(3),
            });
            let mut pcm = [0.0; SAMPLES];
            if peer.decoder.decode_float(&bytes, &mut pcm, false).ok() == Some(SAMPLES) {
                if peer.frames.len() == 3 {
                    peer.frames.pop_front();
                }
                peer.frames.push_back(pcm);
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
            for x in &mut input {
                *x = (*x * status.settings.input_level).clamp(-1.0, 1.0);
            }
            let level = if ready { rms(&input) } else { 0.0 };
            let current = shared.flags.load(Ordering::SeqCst);
            if ready
                && flags & SEND != 0
                && current & SEND != 0
                && shared.flags.load(Ordering::SeqCst) == epoch
            {
                match encoder.encode_float(&input, &mut packet) {
                    Ok(n) => {
                        if shared.flags.load(Ordering::SeqCst) & SEND != 0
                            && shared.flags.load(Ordering::SeqCst) == epoch
                            && outgoing.try_send(packet[..n].to_vec()).is_ok()
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
                for peer in peers.values_mut() {
                    if let Some(frame) = peer.frames.pop_front() {
                        remote = true;
                        for (to, from) in playback.iter_mut().zip(frame) {
                            *to += from;
                        }
                    }
                }
            }
            for x in &mut playback {
                *x = (*x * status.settings.volume).clamp(-1.0, 1.0);
            }
            if let Some(h) = &mut hardware {
                if h.playback.slots() >= SAMPLES {
                    for x in playback {
                        let _ = h.playback.push(x);
                    }
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
            let _ = shared.events.send(Event::Audio(c.status.clone()));
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
fn enumerate() -> (Vec<String>, Vec<String>) {
    let host = cpal::default_host();
    let names = |devices: Result<cpal::InputDevices<cpal::Devices>, cpal::Error>| {
        devices
            .map(|d| {
                d.filter_map(|d| d.description().ok().map(|d| d.name().to_owned()))
                    .collect()
            })
            .unwrap_or_default()
    };
    (names(host.input_devices()), names(host.output_devices()))
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
    configs.into_iter().filter(|c| matches!(c.sample_format(), cpal::SampleFormat::F32 | cpal::SampleFormat::I16 | cpal::SampleFormat::U16)).filter_map(|c| c.try_with_sample_rate(48000)).min_by_key(|c| c.channels()).ok_or_else(|| "El dispositivo debe admitir 48 kHz en formato float32, int16 o uint16. Elegí otro dispositivo.".into())
}
fn open_hardware(shared: &Arc<Shared>, settings: &AudioSettings) -> Result<Hardware, String> {
    let host = cpal::default_host();
    let input = select_device(&host, true, &settings.input)?;
    let output = select_device(&host, false, &settings.output)?;
    let ic = config(&input, true)?;
    let oc = config(&output, false)?;
    let (producer, consumer) = rtrb::RingBuffer::new(SAMPLES * 4);
    let (playback, sink) = rtrb::RingBuffer::new(SAMPLES * 4);
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
        _ => unreachable!(),
    }
    .map_err(|e| format!("No se pudo abrir el micrófono. Revisá permisos y dispositivo: {e}"))?;
    let o = match oc.sample_format() {
        cpal::SampleFormat::F32 => output_stream::<f32>(&output, oc.config(), sink, shared.clone()),
        cpal::SampleFormat::I16 => output_stream::<i16>(&output, oc.config(), sink, shared.clone()),
        cpal::SampleFormat::U16 => output_stream::<u16>(&output, oc.config(), sink, shared.clone()),
        _ => unreachable!(),
    }
    .map_err(|e| format!("No se pudo abrir la salida de audio: {e}"))?;
    o.play()
        .map_err(|e| format!("No se pudo iniciar la salida: {e}"))?;
    i.play()
        .map_err(|e| format!("No se pudo iniciar el micrófono: {e}"))?;
    Ok(Hardware {
        _input: i,
        _output: o,
        capture: consumer,
        playback,
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
    let failure = shared.clone();
    device.build_input_stream(
        config,
        move |data: &[T], _| {
            let epoch = shared.flags.load(Ordering::SeqCst);
            if epoch & CAPTURE == 0 {
                return;
            }
            for frame in data.chunks_exact(channels) {
                let mono = frame
                    .iter()
                    .map(|s| <f32 as cpal::Sample>::from_sample(*s))
                    .sum::<f32>()
                    / channels as f32;
                let _ = producer.push((mono, epoch));
            }
        },
        move |_| {
            failure.flags.fetch_and(!15, Ordering::SeqCst);
            failure.failed.store(true, Ordering::SeqCst);
        },
        Some(Duration::from_secs(2)),
    )
}
fn output_stream<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    mut sink: rtrb::Consumer<f32>,
    shared: Arc<Shared>,
) -> Result<cpal::Stream, cpal::Error>
where
    T: cpal::SizedSample + cpal::FromSample<f32>,
{
    let channels = config.channels as usize;
    let failure = shared.clone();
    device.build_output_stream(
        config,
        move |data: &mut [T], _| {
            let allowed = shared.flags.load(Ordering::Relaxed) & (RECEIVE | TEST) != 0;
            for frame in data.chunks_mut(channels) {
                let value = sink.pop().unwrap_or(0.0);
                frame.fill(T::from_sample(if allowed { value } else { 0.0 }));
            }
        },
        move |_| {
            failure.flags.fetch_and(!15, Ordering::SeqCst);
            failure.failed.store(true, Ordering::SeqCst);
        },
        Some(Duration::from_secs(2)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};
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
                .decode_float(&packet.unwrap(), &mut pcm, false)
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
        settings.volume = 0.0;
        audio
            .command(Command::AudioSettings(settings.clone()))
            .unwrap();
        wait_for(|| audio.synthetic_metrics().unwrap().monitor_rms == 0.0);
        settings.volume = 1.0;
        settings.input_level = 0.0;
        audio.command(Command::AudioSettings(settings)).unwrap();
        wait_for(|| audio.synthetic_metrics().unwrap().input_rms == 0.0);
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
            sink.synthetic_metrics().unwrap().remote_frames > 3
        });
        assert!(sink.synthetic_metrics().unwrap().remote_rms > 0.01);
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
}
