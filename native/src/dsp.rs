//! Local processing only. PCM never leaves the audio worker except through the existing Opus gate.
use crate::contracts::{AdvancedAudioSettings, EqualizerSettings};
use sonora::config::{
    AdaptiveDigital, EchoCanceller, GainController2, MaxProcessingRate, NoiseSuppression,
    NoiseSuppressionLevel,
};
use sonora::{AudioProcessing, Config, StreamConfig};

pub const EQ_FREQUENCIES: [f32; 10] = [
    31.0, 63.0, 125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0, 16000.0,
];

#[derive(Clone, Copy, Default)]
struct Biquad {
    b: [f32; 3],
    a: [f32; 2],
    z: [f32; 2],
}
impl Biquad {
    fn peaking(hz: f32, db: f32) -> Self {
        let gain = 10.0f32.powf(db / 40.0);
        let omega = std::f32::consts::TAU * hz / 48000.0;
        let alpha = omega.sin() / (2.0 * 1.4);
        let a0 = 1.0 + alpha / gain;
        Self {
            b: [
                (1.0 + alpha * gain) / a0,
                -2.0 * omega.cos() / a0,
                (1.0 - alpha * gain) / a0,
            ],
            a: [-2.0 * omega.cos() / a0, (1.0 - alpha / gain) / a0],
            z: [0.0; 2],
        }
    }
    fn sample(&mut self, x: f32) -> f32 {
        let y = self.b[0] * x + self.z[0];
        self.z[0] = self.b[1] * x - self.a[0] * y + self.z[1];
        self.z[1] = self.b[2] * x - self.a[1] * y;
        y
    }
}
#[derive(Default)]
pub struct Equalizer {
    settings: Option<EqualizerSettings>,
    filters: [Biquad; 10],
}
impl Equalizer {
    pub fn process(&mut self, samples: &mut [f32], settings: &EqualizerSettings) {
        if self.settings.as_ref() != Some(settings) {
            self.filters =
                std::array::from_fn(|i| Biquad::peaking(EQ_FREQUENCIES[i], settings.bands_db[i]));
            self.settings = Some(settings.clone());
        }
        if !settings.enabled {
            return;
        }
        let preamp = 10.0f32.powf(settings.preamp_db / 20.0);
        for sample in samples {
            let mut x = *sample * preamp;
            for filter in &mut self.filters {
                x = filter.sample(x);
            }
            *sample = x.clamp(-1.0, 1.0);
        }
    }
}

#[derive(Default)]
pub struct VoiceProcessor {
    config: Option<(u8, bool, bool, u32, u32, bool)>,
    processor: Option<AudioProcessing>,
}
impl VoiceProcessor {
    pub fn process(
        &mut self,
        capture: &mut [f32],
        render: &[f32],
        s: &AdvancedAudioSettings,
    ) -> Result<(), String> {
        let key = (
            s.noise_suppression,
            s.echo_cancellation,
            s.automatic_gain,
            s.gain_headroom_db.to_bits(),
            s.gain_max_db.to_bits(),
            s.processing_48khz,
        );
        if self.config != Some(key) {
            self.processor = if s.noise_suppression > 0 || s.echo_cancellation || s.automatic_gain {
                let config = Config {
                    echo_canceller: s.echo_cancellation.then(EchoCanceller::default),
                    noise_suppression: (s.noise_suppression > 0).then(|| NoiseSuppression {
                        level: match s.noise_suppression {
                            1 => NoiseSuppressionLevel::Low,
                            2 => NoiseSuppressionLevel::Moderate,
                            3 => NoiseSuppressionLevel::High,
                            _ => NoiseSuppressionLevel::VeryHigh,
                        },
                        ..Default::default()
                    }),
                    gain_controller2: s.automatic_gain.then(|| GainController2 {
                        adaptive_digital: Some(AdaptiveDigital {
                            headroom_db: s.gain_headroom_db,
                            max_gain_db: s.gain_max_db,
                            initial_gain_db: 0.0,
                            ..Default::default()
                        }),
                        ..Default::default()
                    }),
                    pipeline: sonora::config::Pipeline {
                        maximum_internal_processing_rate: if s.processing_48khz {
                            MaxProcessingRate::Rate48kHz
                        } else {
                            MaxProcessingRate::Rate32kHz
                        },
                        ..Default::default()
                    },
                    ..Default::default()
                };
                Some(
                    AudioProcessing::builder()
                        .config(config)
                        .capture_config(StreamConfig::new(48000, 1))
                        .render_config(StreamConfig::new(48000, 1))
                        .build(),
                )
            } else {
                None
            };
            self.config = Some(key);
        }
        if let Some(processor) = &mut self.processor {
            processor
                .set_stream_delay_ms(s.echo_delay_ms as i32)
                .map_err(|e| format!("Retardo de eco: {e}"))?;
            // WebRTC processes 10 ms; our transport uses 20 ms. Feed the actual prior playout block.
            for (input, reference) in capture.chunks_exact_mut(480).zip(render.chunks_exact(480)) {
                let mut output = [0.0; 480];
                let mut render_output = [0.0; 480];
                processor
                    .process_render_f32(&[reference], &mut [&mut render_output])
                    .map_err(|e| format!("Referencia de eco: {e}"))?;
                processor
                    .process_capture_f32(&[input], &mut [&mut output])
                    .map_err(|e| format!("Procesamiento de voz: {e}"))?;
                input.copy_from_slice(&output);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn tone(hz: f32) -> Vec<f32> {
        (0..48000)
            .map(|i| (std::f32::consts::TAU * hz * i as f32 / 48000.0).sin() * 0.01)
            .collect()
    }
    fn energy(samples: &[f32]) -> f32 {
        samples[24000..].iter().map(|x| x * x).sum::<f32>().sqrt()
    }
    #[test]
    fn equalizer_boosts_selected_band_and_bypass_is_exact() {
        let mut settings = EqualizerSettings::default();
        settings.bands_db[5] = 6.0;
        let original = tone(1000.0);
        let mut samples = original.clone();
        Equalizer::default().process(&mut samples, &settings);
        assert_eq!(samples, original);
        settings.enabled = true;
        Equalizer::default().process(&mut samples, &settings);
        let ratio = energy(&samples) / energy(&original);
        assert!((1.9..2.1).contains(&ratio));
        let mut low = tone(63.0);
        let baseline = energy(&low);
        Equalizer::default().process(&mut low, &settings);
        assert!((0.98..1.02).contains(&(energy(&low) / baseline)));
    }
    #[test]
    fn processing_modes_remain_finite_and_bypass_is_exact() {
        let mut processor = VoiceProcessor::default();
        let mut settings = AdvancedAudioSettings::default();
        let original = tone(440.0)[..960].to_vec();
        let render = [0.0; 960];
        let mut samples = original.clone();
        processor.process(&mut samples, &render, &settings).unwrap();
        assert_eq!(samples, original);
        for noise in 0..=4 {
            for rate in [false, true] {
                settings.noise_suppression = noise;
                settings.processing_48khz = rate;
                settings.echo_cancellation = true;
                settings.automatic_gain = true;
                for _ in 0..30 {
                    let mut samples = original.clone();
                    processor.process(&mut samples, &render, &settings).unwrap();
                    assert!(samples.iter().all(|x| x.is_finite() && x.abs() <= 1.0));
                }
            }
        }
    }
}
