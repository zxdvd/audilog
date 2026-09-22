use crate::{
    audio,
    session::{AudioSource, Track},
};
use anyhow::{Context, Result, bail, ensure};
use cpal::{
    FromSample, SampleFormat, SizedSample,
    traits::{DeviceTrait, HostTrait, StreamTrait},
};
use crossbeam_channel::{Receiver, Sender};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

pub struct AudioFrame {
    pub source: AudioSource,
    pub timestamp: Duration,
    pub sample_rate: u32,
    pub channels: u16,
    pub sample_index: u64,
    pub samples: Vec<f32>,
}
pub trait AudioCapture {
    fn start(&mut self, tx: Sender<AudioFrame>, epoch: Instant) -> Result<()>;
    fn stop(&mut self) -> Result<()>;
}
pub struct CpalCapture {
    pub source: AudioSource,
    pub name: String,
    device: cpal::Device,
    config: cpal::SupportedStreamConfig,
    stream: Option<cpal::Stream>,
    pub errors: Receiver<cpal::Error>,
    error_tx: Sender<cpal::Error>,
    pub dropped: Arc<AtomicU64>,
    pub frames: Arc<AtomicU64>,
}
impl CpalCapture {
    pub fn new(source: AudioSource, input: Option<&str>) -> Result<Self> {
        let host = cpal::default_host();
        let device = match source {
            AudioSource::Microphone => {
                if let Some(name) = input {
                    host.input_devices()?
                        .find(|d| d.description().is_ok_and(|d| d.name() == name))
                        .with_context(|| {
                            format!("Input device not found: {name}. Run audilog devices")
                        })?
                } else {
                    host.default_input_device()
                        .context("No default microphone")?
                }
            }
            AudioSource::System => {
                ensure!(
                    cfg!(any(target_os = "macos", target_os = "windows")),
                    "System audio requires macOS 14.6+ or Windows. Linux v0.1 supports microphones and file transcription"
                );
                host.default_output_device()
                    .context("No default output device")?
            }
            AudioSource::File => bail!("Files do not use capture"),
        };
        let name = device.description()?.name().to_string();
        let config = if source == AudioSource::System {
            device.default_output_config()?
        } else {
            device.default_input_config()?
        };
        let (error_tx, errors) = crossbeam_channel::bounded(16);
        Ok(Self {
            source,
            name,
            device,
            config,
            stream: None,
            errors,
            error_tx,
            dropped: Arc::new(AtomicU64::new(0)),
            frames: Arc::new(AtomicU64::new(0)),
        })
    }
    fn build<T>(&self, tx: Sender<AudioFrame>, epoch: Instant) -> Result<cpal::Stream>
    where
        T: SizedSample,
        f32: FromSample<T>,
    {
        let source = self.source;
        let sample_rate = self.config.sample_rate();
        let channels = self.config.channels();
        let errors = self.error_tx.clone();
        let dropped = self.dropped.clone();
        let frames = self.frames.clone();
        let mut sample_index = 0;
        Ok(self.device.build_input_stream(
            self.config.config(),
            move |data: &[T], info| {
                let n = data.len() as u64 / channels as u64;
                let ts = info.timestamp();
                let latency = ts.callback.duration_since(ts.capture);
                let timestamp = epoch.elapsed().saturating_sub(latency);
                let frame = AudioFrame {
                    source,
                    timestamp,
                    sample_rate,
                    channels,
                    sample_index,
                    samples: data.iter().map(|s| s.to_sample::<f32>()).collect(),
                };
                sample_index += n;
                frames.store(sample_index, Ordering::Relaxed);
                if tx.try_send(frame).is_err() {
                    dropped.fetch_add(n, Ordering::Relaxed);
                }
            },
            move |err| {
                let _ = errors.try_send(err);
            },
            None,
        )?)
    }
}
impl AudioCapture for CpalCapture {
    fn start(&mut self, tx: Sender<AudioFrame>, epoch: Instant) -> Result<()> {
        let stream = match self.config.sample_format() {
            SampleFormat::F32 => self.build::<f32>(tx, epoch)?,
            SampleFormat::F64 => self.build::<f64>(tx, epoch)?,
            SampleFormat::I8 => self.build::<i8>(tx, epoch)?,
            SampleFormat::I16 => self.build::<i16>(tx, epoch)?,
            SampleFormat::I32 => self.build::<i32>(tx, epoch)?,
            SampleFormat::I64 => self.build::<i64>(tx, epoch)?,
            SampleFormat::U8 => self.build::<u8>(tx, epoch)?,
            SampleFormat::U16 => self.build::<u16>(tx, epoch)?,
            SampleFormat::U32 => self.build::<u32>(tx, epoch)?,
            SampleFormat::U64 => self.build::<u64>(tx, epoch)?,
            f => bail!("Unsupported sample format: {f}"),
        };
        stream.play()?;
        self.stream = Some(stream);
        Ok(())
    }
    fn stop(&mut self) -> Result<()> {
        self.stream.take();
        Ok(())
    }
}
pub fn devices() -> Result<()> {
    let host = cpal::default_host();
    println!("Microphones (--input requires the exact name):");
    for d in host.input_devices()? {
        println!("  {}", d.description()?.name());
    }
    if let Some(d) = host.default_output_device() {
        println!("System audio output: {}", d.description()?.name());
    }
    Ok(())
}
pub fn spawn_writer(
    path: PathBuf,
    source: AudioSource,
    name: String,
    rx: Receiver<AudioFrame>,
    failed: Arc<AtomicBool>,
) -> std::thread::JoinHandle<Result<Track>> {
    std::thread::spawn(move || {
        let result = write_track(path, source, name, rx);
        if result.is_err() {
            failed.store(true, Ordering::Relaxed);
        }
        result
    })
}
fn write_track(
    path: PathBuf,
    source: AudioSource,
    name: String,
    rx: Receiver<AudioFrame>,
) -> Result<Track> {
    let mut wav = audio::writer(&path)?;
    let mut converter = None;
    let mut expected = 0;
    let mut offset = 0;
    let mut count = 0u64;
    let mut peak = 0f32;
    let mut dropped = 0;
    for frame in rx {
        ensure!(frame.source == source, "Incorrect track routing");
        if converter.is_none() {
            // Account for frames lost before the worker received its first callback.
            offset = frame.timestamp.as_millis() as u64;
            expected = frame.sample_index;
            converter = Some(audio::Converter::new(frame.sample_rate)?);
        }
        let c = converter.as_mut().unwrap();
        let mut gap = frame.sample_index.saturating_sub(expected);
        dropped += gap;
        while gap > 0 {
            let n = gap.min(4096);
            let silence = c.push(&vec![0.0; n as usize], 1)?;
            count += silence.len() as u64;
            audio::write(&mut wav, &silence)?;
            gap -= n;
        }
        expected = frame.sample_index + frame.samples.len() as u64 / frame.channels as u64;
        let samples = c.push(&frame.samples, frame.channels)?;
        for s in &samples {
            peak = peak.max(s.abs());
        }
        count += samples.len() as u64;
        audio::write(&mut wav, &samples)?;
        // Keep the WAV header current, so an interrupted process leaves recoverable audio.
        if count / (audio::RATE as u64 * 5)
            != (count.saturating_sub(samples.len() as u64)) / (audio::RATE as u64 * 5)
        {
            wav.flush()?;
        }
    }
    if let Some(c) = converter.as_mut() {
        let tail = c.finish()?;
        for s in &tail {
            peak = peak.max(s.abs());
        }
        count += tail.len() as u64;
        audio::write(&mut wav, &tail)?;
    }
    wav.finalize()?;
    Ok(Track {
        source,
        file: path.file_name().unwrap().to_string_lossy().into(),
        device: name,
        start_offset_ms: offset,
        duration_ms: count * 1000 / audio::RATE as u64,
        dropped_frames: dropped,
        peak,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dropped_frames_preserve_track_timeline_and_offset() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("mic.wav");
        let (tx, rx) = crossbeam_channel::bounded(2);
        for index in [0, 3200] {
            tx.send(AudioFrame {
                source: AudioSource::Microphone,
                timestamp: Duration::from_millis(125 + index / 16),
                sample_rate: 16000,
                channels: 1,
                sample_index: index,
                samples: vec![0.5; 1600],
            })
            .unwrap();
        }
        drop(tx);
        let t = write_track(path.clone(), AudioSource::Microphone, "test".into(), rx).unwrap();
        assert_eq!(t.start_offset_ms, 125);
        assert_eq!(t.duration_ms, 300);
        assert_eq!(t.dropped_frames, 1600);
        let samples: Vec<i16> = hound::WavReader::open(path)
            .unwrap()
            .into_samples()
            .map(Result::unwrap)
            .collect();
        assert_eq!(samples.len(), 4800);
        assert!(samples[1600..3200].iter().all(|s| *s == 0));
        assert!(samples[3200..].iter().all(|s| *s > 0));
    }
}
