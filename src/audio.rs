use crate::config::{Config, Silence};
use anyhow::{Context, Result, anyhow};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, Sample, SampleFormat, SizedSample};
use std::sync::{Arc, Mutex};

pub const WHISPER_RATE: u32 = 16_000;

pub struct Recorder {
    stream: cpal::Stream,
    buf: Arc<Mutex<Vec<f32>>>,
    rate: u32,
}

/// What the configured microphone resolves to right now.
#[derive(Clone, Debug, PartialEq)]
pub enum Mic {
    /// Recording will use `name`; `default` when nothing specific is chosen.
    Ready { name: String, default: bool },
    /// A specific microphone is configured but isn't connected.
    Missing(String),
    /// This machine has no input device at all.
    None,
}

impl Mic {
    pub fn is_ready(&self) -> bool {
        matches!(self, Mic::Ready { .. })
    }

    /// For the header: what will record, or that it is unavailable.
    pub fn label(&self) -> String {
        match self {
            Mic::Ready {
                name,
                default: true,
            } => format!("{name} (default)"),
            Mic::Ready { name, .. } => name.clone(),
            Mic::Missing(wanted) => format!("{wanted} (unavailable)"),
            Mic::None => "unavailable".to_string(),
        }
    }

    /// Why recording can't start, in words the user can act on.
    pub fn problem(&self) -> Option<String> {
        match self {
            Mic::Ready { .. } => None,
            Mic::Missing(wanted) => Some(format!("The microphone '{wanted}' isn't connected. Connect it, or pick another with I in whispertui.")),
            Mic::None => Some("No microphone is available. Connect one, or check System Settings > Sound > Input.".to_string()),
        }
    }
}

/// The input devices present, and which one the system would use.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Inputs {
    pub devices: Vec<String>,
    pub default: Option<String>,
}

/// Every input device, with the host's default included even when the host
/// doesn't enumerate it (ALSA's "Default Audio Device" is only reachable that way).
fn all_inputs(host: &cpal::Host) -> (Vec<(String, cpal::Device)>, Option<String>) {
    let mut devices: Vec<(String, cpal::Device)> = host
        .input_devices()
        .into_iter()
        .flatten()
        .map(|d| (device_name(&d), d))
        .collect();
    let default = host.default_input_device();
    let default_name = default.as_ref().map(device_name);
    if let (Some(name), Some(d)) = (&default_name, default)
        && !devices.iter().any(|(n, _)| n == name)
    {
        devices.push((name.clone(), d));
    }
    (devices, default_name)
}

pub fn inputs() -> Inputs {
    let host = cpal::default_host();
    let (found, default) = all_inputs(&host);
    let mut devices: Vec<String> = found
        .into_iter()
        .filter(|(_, d)| d.default_input_config().is_ok())
        .map(|(n, _)| n)
        .filter(|n| !n.is_empty())
        .collect();
    devices.sort();
    devices.dedup();
    let default = default.filter(|n| devices.contains(n));
    Inputs { devices, default }
}

/// Which microphone `wanted` (the `input_device` setting: a case-insensitive
/// substring of a device name, or empty for the system default) means.
pub fn resolve(wanted: &str, devices: &[String], default: Option<&str>) -> Mic {
    let wanted = wanted.trim();
    if wanted.is_empty() {
        return match default {
            Some(name) => Mic::Ready {
                name: name.to_string(),
                default: true,
            },
            None => Mic::None,
        };
    }
    let lower = wanted.to_lowercase();
    match devices.iter().find(|d| d.to_lowercase().contains(&lower)) {
        Some(name) => Mic::Ready {
            name: name.clone(),
            default: false,
        },
        None => Mic::Missing(wanted.to_string()),
    }
}

pub fn mic_state(wanted: &str) -> Mic {
    let i = inputs();
    resolve(wanted, &i.devices, i.default.as_deref())
}

/// One line of the microphone picker.
#[derive(Clone, Debug, PartialEq)]
pub struct InputRow {
    /// What goes in the config: empty for the system default.
    pub value: String,
    pub label: String,
    pub detail: String,
    pub active: bool,
}

pub fn input_rows(wanted: &str, inputs: &Inputs) -> Vec<InputRow> {
    let mic = resolve(wanted, &inputs.devices, inputs.default.as_deref());
    let chosen = wanted.trim().is_empty();
    let mut rows = vec![InputRow {
        value: String::new(),
        label: "System default".into(),
        detail: inputs
            .default
            .clone()
            .unwrap_or_else(|| "no input device".into()),
        active: chosen,
    }];
    for name in &inputs.devices {
        rows.push(InputRow {
            value: name.clone(),
            label: name.clone(),
            detail: if inputs.default.as_deref() == Some(name) {
                "system default".into()
            } else {
                String::new()
            },
            active: !chosen && matches!(&mic, Mic::Ready { name: n, .. } if n == name),
        });
    }
    // A chosen microphone that isn't plugged in still shows, so the setting isn't invisible.
    if let Mic::Missing(w) = mic {
        rows.push(InputRow {
            value: w.clone(),
            label: w,
            detail: "not connected".into(),
            active: true,
        });
    }
    rows
}

impl Recorder {
    pub fn start(cfg: &Config) -> Result<Recorder> {
        let host = cpal::default_host();
        let (devices, default) = all_inputs(&host);
        let names: Vec<String> = devices.iter().map(|(n, _)| n.clone()).collect();
        let chosen = match resolve(&cfg.input_device, &names, default.as_deref()) {
            Mic::Ready { name, .. } => name,
            other => {
                return Err(anyhow!(
                    other.problem().unwrap_or_else(|| "no microphone".into())
                ));
            }
        };
        let device = devices
            .into_iter()
            .find(|(n, _)| *n == chosen)
            .map(|(_, d)| d)
            .ok_or_else(|| anyhow!("microphone '{chosen}' went away"))?;

        let supported = device.default_input_config().context("input config")?;
        let channels = supported.channels() as usize;
        let rate = supported.sample_rate();
        let format = supported.sample_format();
        let config: cpal::StreamConfig = supported.into();

        let buf = Arc::new(Mutex::new(Vec::<f32>::new()));
        let stream = match format {
            SampleFormat::F32 => build::<f32>(&device, &config, channels, buf.clone())?,
            SampleFormat::I16 => build::<i16>(&device, &config, channels, buf.clone())?,
            SampleFormat::U16 => build::<u16>(&device, &config, channels, buf.clone())?,
            SampleFormat::I32 => build::<i32>(&device, &config, channels, buf.clone())?,
            other => return Err(anyhow!("unsupported sample format {other:?}")),
        };
        stream
            .play()
            .context("starting input stream (microphone permission?)")?;
        Ok(Recorder { stream, buf, rate })
    }

    /// Stop capturing; returns mono 16 kHz samples.
    pub fn finish(self) -> Vec<f32> {
        drop(self.stream);
        let raw = std::mem::take(&mut *self.buf.lock().unwrap());
        resample(&raw, self.rate, WHISPER_RATE)
    }
}

fn device_name(d: &cpal::Device) -> String {
    d.description()
        .map(|d| d.name().to_string())
        .unwrap_or_default()
}

fn build<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    channels: usize,
    buf: Arc<Mutex<Vec<f32>>>,
) -> Result<cpal::Stream>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let stream = device.build_input_stream(
        *config,
        move |data: &[T], _| {
            let mut b = buf.lock().unwrap();
            for frame in data.chunks(channels) {
                let sum: f32 = frame.iter().map(|s| f32::from_sample(*s)).sum();
                b.push(sum / channels as f32);
            }
        },
        |e| eprintln!("audio stream error: {e}"),
        None,
    )?;
    Ok(stream)
}

fn resample(input: &[f32], from: u32, to: u32) -> Vec<f32> {
    if from == to || input.is_empty() {
        return input.to_vec();
    }
    let ratio = from as f64 / to as f64;
    let n = (input.len() as f64 / ratio) as usize;
    (0..n)
        .map(|i| {
            let pos = i as f64 * ratio;
            let idx = pos as usize;
            let frac = (pos - idx as f64) as f32;
            let a = input[idx];
            let b = *input.get(idx + 1).unwrap_or(&a);
            a + (b - a) * frac
        })
        .collect()
}

/// Milliseconds of audio whose 20ms-frame RMS is above the threshold.
pub fn speech_ms(samples: &[f32], threshold_db: f32) -> u32 {
    let frame = (WHISPER_RATE / 50) as usize;
    let loud = samples
        .chunks(frame)
        .filter(|c| {
            let rms = (c.iter().map(|s| s * s).sum::<f32>() / c.len() as f32).sqrt();
            20.0 * rms.max(1e-9).log10() > threshold_db
        })
        .count();
    loud as u32 * 20
}

pub fn is_blank(samples: &[f32], s: &Silence) -> bool {
    s.enabled && speech_ms(samples, s.threshold_db) < s.min_speech_ms
}

pub fn write_wav(path: &std::path::Path, samples: &[f32]) -> Result<()> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: WHISPER_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(path, spec)?;
    for s in samples {
        w.write_sample((s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)?;
    }
    w.finalize()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn empty_setting_means_the_system_default() {
        let devs = names(&["MacBook Pro Microphone", "USB Mic"]);
        let mic = resolve("", &devs, Some("MacBook Pro Microphone"));
        assert_eq!(
            mic,
            Mic::Ready {
                name: "MacBook Pro Microphone".into(),
                default: true
            }
        );
        assert_eq!(mic.label(), "MacBook Pro Microphone (default)");
        assert!(mic.is_ready() && mic.problem().is_none());
        assert_eq!(
            resolve("  ", &devs, Some("USB Mic")).label(),
            "USB Mic (default)"
        );
    }

    #[test]
    fn a_named_microphone_matches_by_substring_ignoring_case() {
        let devs = names(&["MacBook Pro Microphone", "Yeti Stereo Microphone"]);
        let mic = resolve("yeti", &devs, Some("MacBook Pro Microphone"));
        assert_eq!(
            mic,
            Mic::Ready {
                name: "Yeti Stereo Microphone".into(),
                default: false
            }
        );
        assert_eq!(mic.label(), "Yeti Stereo Microphone");
    }

    #[test]
    fn no_microphone_is_unavailable_and_blocks_recording() {
        let none = resolve("", &[], None);
        assert_eq!(none, Mic::None);
        assert_eq!(none.label(), "unavailable");
        assert!(!none.is_ready());
        assert!(
            none.problem()
                .unwrap()
                .contains("No microphone is available")
        );
        // Devices exist but the system has no default: still nothing to record from.
        assert_eq!(resolve("", &names(&["USB Mic"]), None), Mic::None);
    }

    #[test]
    fn a_chosen_microphone_that_is_unplugged_does_not_fall_back_silently() {
        let devs = names(&["MacBook Pro Microphone"]);
        let mic = resolve("AirPods", &devs, Some("MacBook Pro Microphone"));
        assert_eq!(mic, Mic::Missing("AirPods".into()));
        assert_eq!(mic.label(), "AirPods (unavailable)");
        assert!(!mic.is_ready());
        let why = mic.problem().unwrap();
        assert!(
            why.contains("'AirPods' isn't connected") && why.contains("I in whispertui"),
            "{why}"
        );
    }

    #[test]
    fn picker_rows_mark_the_active_choice() {
        let inputs = Inputs {
            devices: names(&["MacBook Pro Microphone", "USB Mic"]),
            default: Some("MacBook Pro Microphone".into()),
        };
        let rows = input_rows("", &inputs);
        assert_eq!(rows.len(), 3);
        assert_eq!(
            (
                rows[0].value.as_str(),
                rows[0].label.as_str(),
                rows[0].detail.as_str(),
                rows[0].active
            ),
            ("", "System default", "MacBook Pro Microphone", true)
        );
        assert_eq!(
            (rows[1].detail.as_str(), rows[1].active),
            ("system default", false)
        );
        assert_eq!((rows[2].value.as_str(), rows[2].active), ("USB Mic", false));

        let rows = input_rows("usb", &inputs);
        assert!(
            !rows[0].active && rows[2].active,
            "choosing a device un-marks the default row"
        );

        let rows = input_rows("AirPods", &inputs);
        let last = rows.last().unwrap();
        assert_eq!(
            (last.label.as_str(), last.detail.as_str(), last.active),
            ("AirPods", "not connected", true)
        );
        assert_eq!(rows.iter().filter(|r| r.active).count(), 1);

        let rows = input_rows("", &Inputs::default());
        assert_eq!(
            (rows.len(), rows[0].detail.as_str()),
            (1, "no input device")
        );
    }

    #[test]
    fn silence_is_blank_and_tone_is_not() {
        let s = Silence::default();
        assert!(is_blank(&vec![0.0; 32_000], &s));
        assert!(is_blank(&vec![0.001; 32_000], &s)); // ~-60 dBFS hiss
        let tone: Vec<f32> = (0..32_000).map(|i| 0.3 * (i as f32 * 0.1).sin()).collect();
        assert!(!is_blank(&tone, &s));
        assert!(is_blank(
            &tone,
            &Silence {
                min_speech_ms: 5_000,
                ..s.clone()
            }
        ));
        assert!(!is_blank(
            &vec![0.0; 100],
            &Silence {
                enabled: false,
                ..s
            }
        ));
    }
}
