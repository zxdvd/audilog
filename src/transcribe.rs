use crate::{
    audio::RATE,
    session::{self, Meta, Segment},
};
use anyhow::{Context, Result, ensure};
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

pub fn run(
    dir: &Path,
    meta: &mut Meta,
    model: &Path,
    cpu: bool,
    cancelled: Arc<AtomicBool>,
) -> Result<()> {
    let mut context_params = WhisperContextParameters::default();
    context_params.use_gpu(!cpu && cfg!(target_os = "macos"));
    eprintln!("Loading model…");
    let ctx = WhisperContext::new_with_params(
        model.to_str().context("Model path must be valid UTF-8")?,
        context_params,
    )
    .context("Cannot load Whisper model. Try --cpu if Metal initialization fails")?;
    let mut segments = Vec::new();
    for track in &meta.sources {
        ensure!(
            !cancelled.load(Ordering::Relaxed),
            "Transcription cancelled; audio is saved"
        );
        eprintln!("Transcribing {}…", track.file);
        let mut wav = hound::WavReader::open(dir.join(&track.file))?;
        ensure!(
            wav.spec().sample_rate == RATE
                && wav.spec().channels == 1
                && wav.spec().bits_per_sample == 16,
            "Session track must be mono 16 kHz PCM16 WAV"
        );
        let mut state = ctx.create_state()?;
        // Bound memory for long meetings; timestamps are re-based for each 10-minute chunk.
        let mut samples = wav.samples::<i16>();
        let mut base_samples = 0u64;
        loop {
            let chunk: Vec<i16> = samples
                .by_ref()
                .take(RATE as usize * 600)
                .collect::<Result<_, _>>()?;
            if chunk.is_empty() {
                break;
            }
            let mut pcm: Vec<f32> = chunk.iter().map(|s| *s as f32 / 32768.0).collect();
            let chunk_len = pcm.len();
            if pcm.iter().any(|s| s.abs() > 0.0001) {
                // Whisper requires at least one second of input.
                pcm.resize(pcm.len().max(RATE as usize), 0.0);
                let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
                params.set_language(if meta.language == "auto" {
                    None
                } else {
                    Some(&meta.language)
                });
                params.set_translate(false);
                params.set_no_context(true);
                params.set_print_progress(false);
                params.set_print_realtime(false);
                params.set_print_timestamps(false);
                // whisper-rs 0.16's closure adapter casts a boxed trait object to
                // the closure type. Use the C callback with a stable AtomicBool instead.
                // SAFETY: cancelled owns this atomic throughout synchronous state.full;
                // the callback only performs a thread-safe load and never unwinds.
                unsafe {
                    params.set_abort_callback(Some(abort_callback));
                    params.set_abort_callback_user_data(Arc::as_ptr(&cancelled).cast_mut().cast());
                }
                state.full(params, &pcm).context("Whisper failed; retry saved audio with audilog transcribe SESSION (add --cpu if Metal fails)")?;
                let base = track.start_offset_ms + base_samples * 1000 / RATE as u64;
                let duration = chunk_len as u64 * 1000 / RATE as u64;
                for seg in state.as_iter() {
                    let start = (seg.start_timestamp().max(0) as u64 * 10).min(duration);
                    let end = (seg.end_timestamp().max(0) as u64 * 10).min(duration);
                    let text = seg.to_str_lossy()?.trim().to_owned();
                    if !text.is_empty() && end > start {
                        segments.push(Segment {
                            start_ms: base + start,
                            end_ms: base + end,
                            source: track.source,
                            text,
                        });
                    }
                }
            }
            ensure!(
                !cancelled.load(Ordering::Relaxed),
                "Transcription cancelled; audio is saved"
            );
            base_samples += chunk_len as u64;
        }
    }
    session::save_transcript(dir, segments)?;
    meta.status = "complete".into();
    session::save_meta(dir, meta)?;
    println!(
        "✓ Transcript saved: {}",
        dir.join("transcript.md").display()
    );
    Ok(())
}

unsafe extern "C" fn abort_callback(data: *mut std::ffi::c_void) -> bool {
    // SAFETY: run supplies a live AtomicBool for the entire inference call.
    unsafe { &*data.cast::<AtomicBool>() }.load(Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn abort_callback_reads_only_the_owned_atomic() {
        let flag = AtomicBool::new(false);
        let ptr = (&flag as *const AtomicBool).cast_mut().cast();
        assert!(!unsafe { abort_callback(ptr) });
        flag.store(true, Ordering::Relaxed);
        assert!(unsafe { abort_callback(ptr) });
    }
}
