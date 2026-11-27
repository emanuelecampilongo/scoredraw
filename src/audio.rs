use crate::data::Settings;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, Sample, SampleFormat, SizedSample, Stream, StreamConfig};
use std::collections::VecDeque;
use std::f32::consts::TAU;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const WINDOW: usize = 2048;

type AudioError = Box<dyn std::error::Error>;

struct Metronome {
    _stream: Stream,
    error: Arc<Mutex<Option<String>>>,
    tempo: Arc<AtomicU32>,
    visual: Arc<AtomicU32>,
}

impl Metronome {
    fn start(bpm: u16, beats: u8) -> Result<Self, AudioError> {
        let device = cpal::default_host().default_output_device().ok_or(tr!(
            "Uscita audio non disponibile.",
            "No audio output available."
        ))?;
        let supported = device.default_output_config()?;
        let config = supported.config();
        let error = Arc::default();
        let tempo = Arc::new(AtomicU32::new(tempo_value(bpm, beats)));
        let visual = Arc::new(AtomicU32::new(0));
        let stream = match supported.sample_format() {
            SampleFormat::F32 => output::<f32>(&device, &config, &tempo, &error, &visual)?,
            SampleFormat::F64 => output::<f64>(&device, &config, &tempo, &error, &visual)?,
            SampleFormat::I16 => output::<i16>(&device, &config, &tempo, &error, &visual)?,
            SampleFormat::I32 => output::<i32>(&device, &config, &tempo, &error, &visual)?,
            SampleFormat::U16 => output::<u16>(&device, &config, &tempo, &error, &visual)?,
            format => {
                return Err(trf!(
                    "Formato audio di uscita non supportato: {format}.",
                    "Unsupported audio output format: {format}."
                )
                .into());
            }
        };
        stream.play()?;
        Ok(Self {
            _stream: stream,
            error,
            tempo,
            visual,
        })
    }
}

fn tempo_value(bpm: u16, beats: u8) -> u32 {
    u32::from(bpm.clamp(30, 300)) << 8 | u32::from(beats.clamp(1, 12))
}

fn output<T: SizedSample + FromSample<f32>>(
    device: &cpal::Device,
    config: &StreamConfig,
    tempo: &Arc<AtomicU32>,
    error: &Arc<Mutex<Option<String>>>,
    visual: &Arc<AtomicU32>,
) -> Result<Stream, cpal::BuildStreamError> {
    let channels = usize::from(config.channels);
    let rate = config.sample_rate;
    let tempo = tempo.clone();
    let visual = visual.clone();
    let mut counter = 0u32;
    let mut last = tempo.load(Ordering::Relaxed);
    let mut beat = Beat::new(rate, (last >> 8) as u16, last as u8);
    let error = error.clone();
    device.build_output_stream(
        config,
        move |samples: &mut [T], _| {
            let value = tempo.load(Ordering::Relaxed);
            if value != last {
                beat.set_tempo(rate, (value >> 8) as u16, value as u8);
                last = value;
            }
            for frame in samples.chunks_mut(channels) {
                if beat.remaining <= 0.0 {
                    counter = counter.wrapping_add(16);
                    visual.store(counter | beat.beat as u32, Ordering::Relaxed);
                }
                frame.fill(T::from_sample(beat.next()));
            }
        },
        move |err| report(&error, err.to_string()),
        Some(Duration::from_secs(2)),
    )
}

struct Beat {
    remaining: f64,
    interval: f64,
    beat: usize,
    beats: usize,
    active: usize,
    sample: usize,
    clicks: [Vec<f32>; 2],
}

impl Beat {
    fn new(rate: u32, bpm: u16, beats: u8) -> Self {
        let click = |frequency: f32, gain: f32| {
            (0..(rate * 3 / 100))
                .map(|frame| {
                    let t = frame as f32 / rate as f32;
                    (TAU * frequency * t).sin() * gain * (1.0 - t / 0.03).powi(4)
                })
                .collect()
        };
        Self {
            remaining: 0.0,
            interval: f64::from(rate) * 60.0 / f64::from(bpm),
            beat: 0,
            beats: usize::from(beats),
            active: 0,
            sample: 0,
            clicks: [click(980.0, 0.23), click(1480.0, 0.33)],
        }
    }

    fn set_tempo(&mut self, rate: u32, bpm: u16, beats: u8) {
        let next = f64::from(rate) * 60.0 / f64::from(bpm);
        self.remaining *= next / self.interval;
        self.interval = next;
        self.beats = usize::from(beats);
        self.beat %= self.beats;
    }

    fn next(&mut self) -> f32 {
        if self.remaining <= 0.0 {
            self.active = usize::from(self.beat == 0);
            self.sample = 0;
            self.beat = (self.beat + 1) % self.beats;
            self.remaining += self.interval;
        }
        let value = self.clicks[self.active]
            .get(self.sample)
            .copied()
            .unwrap_or(0.0);
        self.sample += 1;
        self.remaining -= 1.0;
        value
    }
}

struct Tuner {
    _stream: Stream,
    error: Arc<Mutex<Option<String>>>,
    pitch: Arc<AtomicU32>,
}

struct MicBuffer {
    data: VecDeque<f32>,
    next: f32,
    count: u32,
    updated: Option<Instant>,
}

impl Tuner {
    fn start() -> Result<Self, AudioError> {
        let device = cpal::default_host().default_input_device().ok_or(tr!(
            "Microfono non disponibile.",
            "No microphone available."
        ))?;
        let supported = device.default_input_config()?;
        let config = supported.config();
        let decimation = (config.sample_rate / 12_000).max(1);
        let samples = Arc::new(Mutex::new(MicBuffer {
            data: VecDeque::with_capacity(WINDOW),
            next: 0.0,
            count: 0,
            updated: None,
        }));
        let error = Arc::default();
        let stream = match supported.sample_format() {
            SampleFormat::F32 => input::<f32>(&device, &config, decimation, &samples, &error)?,
            SampleFormat::F64 => input::<f64>(&device, &config, decimation, &samples, &error)?,
            SampleFormat::I16 => input::<i16>(&device, &config, decimation, &samples, &error)?,
            SampleFormat::I32 => input::<i32>(&device, &config, decimation, &samples, &error)?,
            SampleFormat::U16 => input::<u16>(&device, &config, decimation, &samples, &error)?,
            format => {
                return Err(trf!(
                    "Formato del microfono non supportato: {format}.",
                    "Unsupported microphone format: {format}."
                )
                .into());
            }
        };
        stream.play()?;
        let pitch = Arc::new(AtomicU32::new(0));
        let target = Arc::downgrade(&pitch);
        let rate = config.sample_rate as f32 / decimation as f32;
        std::thread::Builder::new()
            .name("analisi-intonazione".into())
            .spawn(move || {
                let mut signal = [0.0; WINDOW];
                loop {
                    {
                        let Some(target) = target.upgrade() else {
                            break;
                        };
                        let ready = samples.try_lock().is_ok_and(|buffer| {
                            if buffer.data.len() != WINDOW
                                || !buffer.updated.is_some_and(|stamp| {
                                    stamp.elapsed() <= Duration::from_millis(400)
                                })
                            {
                                return false;
                            }
                            for (out, input) in signal.iter_mut().zip(&buffer.data) {
                                *out = *input;
                            }
                            true
                        });
                        let hz = ready
                            .then(|| detect_pitch(&signal, rate))
                            .flatten()
                            .unwrap_or(0.0);
                        target.store(hz.to_bits(), Ordering::Relaxed);
                    }
                    std::thread::sleep(Duration::from_millis(85));
                }
            })?;
        Ok(Self {
            _stream: stream,
            error,
            pitch,
        })
    }

    fn frequency(&self) -> Option<f32> {
        let hz = f32::from_bits(self.pitch.load(Ordering::Relaxed));
        (hz > 0.0).then_some(hz)
    }
}

fn input<T: SizedSample>(
    device: &cpal::Device,
    config: &StreamConfig,
    decimation: u32,
    samples: &Arc<Mutex<MicBuffer>>,
    error: &Arc<Mutex<Option<String>>>,
) -> Result<Stream, cpal::BuildStreamError>
where
    f32: FromSample<T>,
{
    let channels = usize::from(config.channels);
    let samples = samples.clone();
    let error = error.clone();
    device.build_input_stream(
        config,
        move |input: &[T], _| {
            let Ok(mut samples) = samples.try_lock() else {
                return;
            };
            for frame in input.chunks_exact(channels) {
                let sum: f32 = frame.iter().copied().map(Sample::to_sample::<f32>).sum();
                if !sum.is_finite() {
                    continue;
                }
                samples.next += sum / channels as f32;
                samples.count += 1;
                if samples.count == decimation {
                    let value = samples.next / decimation as f32;
                    if samples.data.len() == WINDOW {
                        samples.data.pop_front();
                    }
                    samples.data.push_back(value);
                    samples.next = 0.0;
                    samples.count = 0;
                }
            }
            samples.updated = Some(Instant::now());
        },
        move |err| report(&error, err.to_string()),
        Some(Duration::from_secs(2)),
    )
}

fn report(error: &Mutex<Option<String>>, text: String) {
    if let Ok(mut slot) = error.lock() {
        *slot = Some(text);
    }
}

fn stream_error(error: &Mutex<Option<String>>) -> Option<String> {
    error.try_lock().ok()?.take()
}

fn detect_pitch(samples: &[f32], rate: f32) -> Option<f32> {
    let max_lag = ((rate / 26.0) as usize).min(samples.len() / 2);
    let min_lag = ((rate / 1600.0) as usize).max(2);
    if max_lag <= min_lag
        || samples.iter().map(|x| x * x).sum::<f32>() < 0.000016 * samples.len() as f32
    {
        return None;
    }
    // YIN's cumulative mean normalised difference, with the same comparison window for each lag.
    let mut values = vec![1.0f32; max_lag + 1];
    let mut sum = 0.0f32;
    let count = samples.len() - max_lag;
    for lag in 1..=max_lag {
        let diff = samples[..count]
            .iter()
            .zip(&samples[lag..lag + count])
            .map(|(x, y)| {
                let d = x - y;
                d * d
            })
            .sum::<f32>();
        sum += diff;
        if sum > 1e-8 {
            values[lag] = diff * lag as f32 / sum;
        }
        let center = lag - 1;
        if center >= min_lag
            && values[center] < 0.16
            && values[center] <= values[center - 1]
            && values[center] < values[lag]
        {
            let (left, middle, right) = (values[center - 1], values[center], values[lag]);
            let curvature = left + right - 2.0 * middle;
            let offset = if curvature > 1e-6 {
                ((left - right) / (2.0 * curvature)).clamp(-0.5, 0.5)
            } else {
                0.0
            };
            return Some(rate / (center as f32 + offset));
        }
    }
    None
}

fn note(hz: f32, reference: u16) -> (String, String, f32) {
    const ITALIAN: [&str; 12] = [
        "Do", "Do♯", "Re", "Re♯", "Mi", "Fa", "Fa♯", "Sol", "Sol♯", "La", "La♯", "Si",
    ];
    const ENGLISH: [&str; 12] = [
        "C", "C♯", "D", "D♯", "E", "F", "F♯", "G", "G♯", "A", "A♯", "B",
    ];
    let midi = 69.0 + 12.0 * (hz / f32::from(reference)).log2();
    let nearest = midi.round() as i32;
    let cents = ((midi - nearest as f32) * 100.0).round() as i32;
    let names = if crate::language::english() {
        ENGLISH
    } else {
        ITALIAN
    };
    let name = names[nearest.rem_euclid(12) as usize];
    let octave = nearest.div_euclid(12) - 1;
    let status = if cents.abs() <= 3 {
        tr!("intonato", "in tune")
    } else if cents < 0 {
        tr!("crescere", "flat")
    } else {
        tr!("calare", "sharp")
    };
    (
        format!("{name}{octave}   {hz:.1} Hz   {cents:+} cent · {status}"),
        format!("{name}{octave}   {cents:+} cent"),
        cents as f32,
    )
}

#[derive(Default)]
pub struct Sound {
    metronome: Option<Metronome>,
    tuner: Option<Tuner>,
    status: [String; 2],
    pub pitch: Option<f32>,
    pub compact_pitch: String,
    last_beat: u32,
    flash: Option<(u8, Instant)>,
}

impl Sound {
    pub fn running(&self, slot: usize) -> bool {
        if slot == 0 {
            self.metronome.is_some()
        } else {
            self.tuner.is_some()
        }
    }

    pub fn status(&self, slot: usize) -> &str {
        if self.status[slot].is_empty() {
            return match slot {
                0 if self.metronome.is_some() => tr!("In riproduzione", "Playing"),
                0 => tr!("Fermo", "Stopped"),
                _ => tr!("Microfono fermo", "Microphone off"),
            };
        }
        &self.status[slot]
    }

    pub fn refresh_tempo(&mut self, settings: Settings) {
        if let Some(stream) = &self.metronome {
            stream
                .tempo
                .store(tempo_value(settings.bpm, settings.beats), Ordering::Relaxed);
            return;
        }
        match Metronome::start(settings.bpm, settings.beats) {
            Ok(stream) => {
                self.metronome = Some(stream);
                self.status[0].clear();
            }
            Err(error) => self.status[0] = error.to_string(),
        }
    }

    pub fn toggle(&mut self, slot: usize, settings: Settings) {
        if self.running(slot) {
            self.stop(slot);
            return;
        }
        if slot == 0 {
            self.refresh_tempo(settings);
        } else {
            match Tuner::start() {
                Ok(stream) => {
                    self.tuner = Some(stream);
                    self.status[1] = tr!(
                        "In ascolto… suona una nota tenuta",
                        "Listening… play a sustained note"
                    )
                    .into();
                }
                Err(error) => self.status[1] = error.to_string(),
            }
        }
    }

    pub fn stop(&mut self, slot: usize) {
        if slot == 0 {
            self.metronome = None;
            self.flash = None;
            self.last_beat = 0;
        } else {
            self.tuner = None;
            self.pitch = None;
            self.compact_pitch.clear();
        }
        self.status[slot].clear();
    }

    pub fn pulse(&self) -> Option<(u8, f32)> {
        let (beat, at) = self.flash?;
        let strength = (1.0 - at.elapsed().as_secs_f32() / 0.16).max(0.0);
        Some((beat, strength))
    }
    pub fn tick(&mut self, reference: u16) -> bool {
        let mut changed = false;

        if let Some(stream) = &self.metronome {
            let beat = stream.visual.load(Ordering::Relaxed);
            if beat != self.last_beat {
                self.last_beat = beat;
                self.flash = Some(((beat & 15) as u8, Instant::now()));
            }
        }
        if let Some((_, at)) = self.flash {
            changed = true;
            if at.elapsed() > Duration::from_millis(160) {
                self.flash = None;
            }
        }
        if let Some(metronome) = &self.metronome
            && let Some(error) = stream_error(&metronome.error)
        {
            self.metronome = None;
            self.flash = None;
            self.last_beat = 0;
            self.status[0] = error;
            changed = true;
        }
        if let Some(tuner) = &self.tuner {
            let (next, compact, pitch) = if let Some(error) = stream_error(&tuner.error) {
                self.tuner = None;
                (error, String::new(), None)
            } else if let Some(hz) = tuner.frequency() {
                let (label, compact, cents) = note(hz, reference);
                (label, compact, Some(cents))
            } else {
                (
                    tr!(
                        "In ascolto… suona una nota tenuta",
                        "Listening… play a sustained note"
                    )
                    .into(),
                    String::new(),
                    None,
                )
            };
            changed |= self.pitch != pitch;
            self.pitch = pitch;
            self.compact_pitch = compact;
            if next != self.status[1] {
                self.status[1] = next;
                changed = true;
            }
        }
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uses_the_configured_reference_to_compute_cents() {
        let (_, _, cents) = note(442.0, 442);
        assert_eq!(cents, 0.0);
        let (_, _, cents) = note(442.0, 440);
        assert_eq!(cents, 8.0);
    }

    #[test]
    fn extracts_musical_pitch_and_rejects_silence() {
        for frequency in [55.0, 82.4, 440.0, 1046.5] {
            let signal: Vec<_> = (0..WINDOW)
                .map(|sample| {
                    let phase = TAU * frequency * sample as f32 / 12_000.0;
                    phase.sin() * 0.4 + (phase * 2.0).sin() * 0.15
                })
                .collect();
            let found = detect_pitch(&signal, 12_000.0).unwrap();
            assert!(
                (found / frequency - 1.0).abs() < 0.008,
                "{frequency}: {found}"
            );
        }
        assert!(detect_pitch(&[0.0; WINDOW], 12_000.0).is_none());
    }

    #[test]
    fn counts_beats_by_audio_samples() {
        let mut beat = Beat::new(48_000, 120, 4);
        let mut starts = Vec::new();
        for frame in 0..96_001 {
            beat.next();
            if beat.sample == 1 {
                starts.push((frame, beat.active));
            }
        }
        assert_eq!(
            starts,
            [(0, 1), (24_000, 0), (48_000, 0), (72_000, 0), (96_000, 1)]
        );
    }
}
