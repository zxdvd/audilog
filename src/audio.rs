use anyhow::{Context, Result, ensure};
use rubato::{FftFixedIn, Resampler};
use std::{
    fs::File,
    io::{BufWriter, ErrorKind},
    path::Path,
};
use symphonia::core::{
    audio::SampleBuffer, codecs::DecoderOptions, errors::Error, formats::FormatOptions,
    io::MediaSourceStream, meta::MetadataOptions, probe::Hint,
};

pub const RATE: u32 = 16000;
// Streaming band-limited resampling; retain filter state across callbacks and trim its delay.
pub struct Converter {
    resampler: Option<FftFixedIn<f32>>,
    pending: Vec<f32>,
    input_rate: u32,
    input_frames: usize,
    output_frames: usize,
    skip: usize,
}
impl Converter {
    pub fn new(rate: u32) -> Result<Self> {
        ensure!(rate > 0, "Invalid sample rate");
        let resampler = if rate == RATE {
            None
        } else {
            Some(FftFixedIn::new(rate as usize, RATE as usize, 1024, 2, 1)?)
        };
        let skip = resampler.as_ref().map_or(0, |r| r.output_delay());
        Ok(Self {
            resampler,
            pending: Vec::new(),
            input_rate: rate,
            input_frames: 0,
            output_frames: 0,
            skip,
        })
    }
    pub fn push(&mut self, samples: &[f32], channels: u16) -> Result<Vec<f32>> {
        ensure!(
            channels > 0 && samples.len().is_multiple_of(channels as usize),
            "Invalid audio channels"
        );
        let mono: Vec<f32> = samples
            .chunks_exact(channels as usize)
            .map(|frame| {
                frame
                    .iter()
                    .map(|x| if x.is_finite() { *x } else { 0.0 })
                    .sum::<f32>()
                    / channels as f32
            })
            .collect();
        self.input_frames += mono.len();
        if self.resampler.is_none() {
            self.output_frames += mono.len();
            return Ok(mono);
        }
        self.pending.extend(mono);
        let mut result = Vec::new();
        let r = self.resampler.as_mut().unwrap();
        while self.pending.len() >= r.input_frames_next() {
            let n = r.input_frames_next();
            let block: Vec<_> = self.pending.drain(..n).collect();
            let output = r.process(&[block], None)?;
            let skip = self.skip.min(output[0].len());
            self.skip -= skip;
            result.extend_from_slice(&output[0][skip..]);
        }
        self.output_frames += result.len();
        Ok(result)
    }
    pub fn finish(&mut self) -> Result<Vec<f32>> {
        let target = (self.input_frames as u64 * RATE as u64 / self.input_rate as u64) as usize;
        let remaining = target.saturating_sub(self.output_frames);
        let Some(r) = self.resampler.as_mut() else {
            return Ok(Vec::new());
        };
        let mut result = Vec::new();
        while result.len() < remaining {
            self.pending.resize(r.input_frames_next(), 0.0);
            let output = r.process(&[std::mem::take(&mut self.pending)], None)?;
            let skip = self.skip.min(output[0].len());
            self.skip -= skip;
            result.extend_from_slice(&output[0][skip..]);
        }
        result.truncate(remaining);
        self.output_frames += result.len();
        Ok(result)
    }
}
pub fn writer(path: &Path) -> Result<hound::WavWriter<BufWriter<File>>> {
    Ok(hound::WavWriter::create(
        path,
        hound::WavSpec {
            channels: 1,
            sample_rate: RATE,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        },
    )?)
}
pub fn write(writer: &mut hound::WavWriter<BufWriter<File>>, samples: &[f32]) -> Result<()> {
    for s in samples {
        writer.write_sample((s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)?;
    }
    Ok(())
}
// Decode common audio files into the same durable PCM format used by capture.
pub fn import(input: &Path, output: &Path) -> Result<u64> {
    let stream = MediaSourceStream::new(Box::new(File::open(input)?), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = input.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let probed = symphonia::default::get_probe().format(
        &hint,
        stream,
        &FormatOptions::default(),
        &MetadataOptions::default(),
    )?;
    let mut format = probed.format;
    let track = format.default_track().context("No audio track")?;
    let id = track.id;
    let mut decoder =
        symphonia::default::get_codecs().make(&track.codec_params, &DecoderOptions::default())?;
    let mut converter: Option<Converter> = None;
    let mut out = writer(output)?;
    let mut count = 0u64;
    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(Error::IoError(e)) if e.kind() == ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(e.into()),
        };
        if packet.track_id() != id {
            continue;
        }
        let decoded = decoder.decode(&packet)?;
        let spec = *decoded.spec();
        if converter.is_none() {
            converter = Some(Converter::new(spec.rate)?);
        }
        let c = converter.as_mut().unwrap();
        ensure!(
            c.input_rate == spec.rate,
            "Sample rate changes within this file are unsupported"
        );
        let mut buf = SampleBuffer::<f32>::new(decoded.capacity() as u64, spec);
        buf.copy_interleaved_ref(decoded);
        let samples = c.push(buf.samples(), spec.channels.count() as u16)?;
        count += samples.len() as u64;
        write(&mut out, &samples)?;
    }
    if let Some(c) = converter.as_mut() {
        let tail = c.finish()?;
        count += tail.len() as u64;
        write(&mut out, &tail)?;
    }
    out.finalize()?;
    ensure!(count > 0, "The input contains no audio samples");
    Ok(count * 1000 / RATE as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn resampling_keeps_duration_across_callback_boundaries() {
        for rate in [8000, 16000, 44100, 48000, 96000] {
            let input: Vec<_> = (0..rate * 2 + 73)
                .map(|n| (n as f32 * 1000.0 * std::f32::consts::TAU / rate as f32).sin() * 0.5)
                .collect();
            let mut c = Converter::new(rate).unwrap();
            let mut result = Vec::new();
            for chunk in input.chunks(173) {
                result.extend(c.push(chunk, 1).unwrap());
            }
            result.extend(c.finish().unwrap());
            assert_eq!(result.len(), input.len() * RATE as usize / rate as usize);
            let rms = (result[1000..30000].iter().map(|s| s * s).sum::<f32>() / 29000.0).sqrt();
            assert!((rms - 0.35355).abs() < 0.02, "rate={rate} rms={rms}");
        }
    }
    #[test]
    fn downsampling_removes_aliasing() {
        let input: Vec<_> = (0..48000)
            .map(|n| (n as f32 * 12000.0 * std::f32::consts::TAU / 48000.0).sin())
            .collect();
        let mut c = Converter::new(48000).unwrap();
        let mut out = c.push(&input, 1).unwrap();
        out.extend(c.finish().unwrap());
        let rms = (out[1000..15000].iter().map(|x| x * x).sum::<f32>() / 14000.0).sqrt();
        assert!(rms < 0.005, "aliasing RMS {rms}");
    }
    #[test]
    fn stereo_import_and_malformed_file() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("stereo.wav");
        let dst = dir.path().join("audio.wav");
        let mut w = hound::WavWriter::create(
            &src,
            hound::WavSpec {
                channels: 2,
                sample_rate: 48000,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            },
        )
        .unwrap();
        for _ in 0..48000 {
            w.write_sample(10000i16).unwrap();
            w.write_sample(-10000i16).unwrap();
        }
        w.finalize().unwrap();
        assert_eq!(import(&src, &dst).unwrap(), 1000);
        let mut r = hound::WavReader::open(&dst).unwrap();
        assert_eq!(r.duration(), 16000);
        assert!(r.samples::<i16>().all(|s| s.unwrap() == 0));
        std::fs::write(&src, b"not audio").unwrap();
        assert!(import(&src, &dst).is_err());
    }
}
