mod audio;
mod capture;
mod model;
mod session;
mod transcribe;

use anyhow::{Context, Result, bail, ensure};
use capture::{AudioCapture, CpalCapture};
use clap::{Parser, Subcommand};
use session::{AudioSource, Meta, Track};
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

#[derive(Parser, Debug)]
#[command(
    version,
    about = "Record audio. Transcribe locally. No account or cloud upload."
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
    /// Record microphone and system audio as separate tracks (macOS/Windows)
    #[arg(long, global = true, conflicts_with = "system_only")]
    system: bool,
    /// Record only system audio (macOS/Windows)
    #[arg(long, global = true, conflicts_with = "input")]
    system_only: bool,
    /// Exact microphone name from `audilog devices`
    #[arg(long, global = true)]
    input: Option<String>,
    /// Whisper language code, such as zh or en; defaults to auto detection
    #[arg(long, global = true)]
    language: Option<String>,
    /// Session parent directory (default: Documents/audilog)
    #[arg(long, global = true)]
    output: Option<PathBuf>,
    /// Model name or path to a custom GGML model
    #[arg(long, global = true)]
    model: Option<String>,
    /// Accept the model download without prompting
    #[arg(short = 'y', long, global = true)]
    yes: bool,
    /// Disable Metal and use CPU inference
    #[arg(long, global = true)]
    cpu: bool,
    /// Save audio without transcription or a model download
    #[arg(long, global = true)]
    no_transcribe: bool,
    /// Stop recording automatically after N seconds
    #[arg(long, global = true, value_parser = clap::value_parser!(u64).range(1..))]
    seconds: Option<u64>,
}
#[derive(Subcommand, Debug)]
enum Command {
    /// Record until Ctrl+C, then transcribe (also the default command)
    Record,
    /// List microphone devices and the default system output
    Devices,
    /// Transcribe an audio file, or retry a saved session directory
    Transcribe { file: PathBuf },
    /// Probe audio for 3 seconds and check storage/model readiness; play sound and speak
    Doctor,
    /// List, download, or select a local Whisper model
    Model {
        #[command(subcommand)]
        action: ModelCommand,
    },
}
#[derive(Subcommand, Debug)]
enum ModelCommand {
    List,
    Install { name: Option<String> },
    Use { name: String },
}

fn main() {
    if let Err(e) = run() {
        eprintln!("Error: {e:#}");
        std::process::exit(1);
    }
}
fn run() -> Result<()> {
    let cli = Cli::parse();
    if let Some(lang) = cli.language.as_deref() {
        ensure!(
            lang == "auto" || whisper_rs::get_lang_id(lang).is_some(),
            "Unknown Whisper language: {lang}"
        );
    }
    let stop = Arc::new(AtomicBool::new(false));
    let signal = stop.clone();
    ctrlc::set_handler(move || {
        signal.store(true, Ordering::Relaxed);
    })?;
    match &cli.command {
        Some(Command::Devices) => capture::devices(),
        Some(Command::Model { action }) => match action {
            ModelCommand::List => model::list(),
            ModelCommand::Install { name } => {
                model::install(name.as_deref().unwrap_or(model::DEFAULT))?;
                Ok(())
            }
            ModelCommand::Use { name } => model::use_model(name),
        },
        Some(Command::Doctor) => doctor(&cli, stop),
        Some(Command::Transcribe { file }) => transcribe_file(&cli, file, stop),
        Some(Command::Record) | None => record(&cli, stop),
    }
}
fn root(cli: &Cli) -> Result<PathBuf> {
    cli.output
        .clone()
        .map(Ok)
        .unwrap_or_else(model::output_root)
}
fn initial_meta(cli: &Cli, name: String) -> Meta {
    Meta {
        version: 1,
        started_at: chrono::Local::now().to_rfc3339(),
        duration_ms: 0,
        status: "recording".into(),
        sources: Vec::new(),
        model: name,
        language: cli.language.clone().unwrap_or_else(|| "auto".into()),
    }
}
fn permission_help() {
    #[cfg(target_os = "macos")]
    eprintln!(
        "macOS: allow your terminal app in System Settings → Privacy & Security → Microphone and Screen & System Audio Recording → System Audio Only. Restart the terminal after changing permission. Silence alone does not prove permission was denied."
    );
}
fn record(cli: &Cli, stop: Arc<AtomicBool>) -> Result<()> {
    let resolved = if cli.no_transcribe {
        None
    } else {
        Some(model::resolve(cli.model.as_deref(), cli.yes)?)
    };
    let mut captures = Vec::new();
    if !cli.system_only {
        captures.push(CpalCapture::new(
            AudioSource::Microphone,
            cli.input.as_deref(),
        )?);
    }
    if cli.system || cli.system_only {
        captures.push(CpalCapture::new(AudioSource::System, None)?);
    }
    let dir = session::create(&root(cli)?)?;
    println!("Session: {}", dir.display());
    let mut meta = initial_meta(
        cli,
        resolved
            .as_ref()
            .map(|r| r.0.clone())
            .unwrap_or_else(|| "none".into()),
    );
    for cap in &captures {
        let file = match cap.source {
            AudioSource::Microphone if !cli.system => "audio.wav",
            AudioSource::Microphone => "mic.wav",
            _ => "system.wav",
        };
        meta.sources.push(Track {
            source: cap.source,
            file: file.into(),
            device: cap.name.clone(),
            start_offset_ms: 0,
            duration_ms: 0,
            dropped_frames: 0,
            peak: 0.0,
        });
    }
    session::save_meta(&dir, &meta)?;
    let epoch = Instant::now();
    let failed = Arc::new(AtomicBool::new(false));
    let mut workers = Vec::new();
    let mut capture_error = None;
    for (cap, track) in captures.iter_mut().zip(&meta.sources) {
        let (tx, rx) = crossbeam_channel::bounded(256);
        workers.push(capture::spawn_writer(
            dir.join(&track.file),
            cap.source,
            cap.name.clone(),
            rx,
            failed.clone(),
        ));
        if let Err(e) = cap.start(tx, epoch) {
            capture_error = Some(e);
            break;
        }
    }
    if capture_error.is_none() {
        println!("● Recording… Ctrl+C to stop and transcribe.");
        while !stop.load(Ordering::Relaxed) && !failed.load(Ordering::Relaxed) {
            for cap in &captures {
                if let Ok(e) = cap.errors.try_recv() {
                    capture_error = Some(anyhow::anyhow!(e));
                    break;
                }
                if epoch.elapsed() > Duration::from_secs(8)
                    && cap.frames.load(Ordering::Relaxed) == 0
                {
                    capture_error = Some(anyhow::anyhow!(
                        "No audio callbacks from {}. Check permissions/device",
                        cap.name
                    ));
                    break;
                }
            }
            if capture_error.is_some()
                || cli
                    .seconds
                    .is_some_and(|s| epoch.elapsed() >= Duration::from_secs(s))
            {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    for cap in &mut captures {
        cap.stop()?;
    }
    let mut writer_error = None;
    for (i, worker) in workers.into_iter().enumerate() {
        match worker
            .join()
            .map_err(|_| anyhow::anyhow!("Audio writer panicked"))
            .and_then(|r| r)
        {
            Ok(mut t) => {
                t.dropped_frames = captures[i].dropped.load(Ordering::Relaxed);
                meta.sources[i] = t;
            }
            Err(e) => writer_error = Some(e),
        }
    }
    meta.duration_ms = epoch.elapsed().as_millis() as u64;
    meta.status = if capture_error.is_some() || writer_error.is_some() {
        "capture_failed"
    } else {
        "recorded"
    }
    .into();
    session::save_meta(&dir, &meta)?;
    if let Some(e) = writer_error {
        return Err(e).context(format!(
            "Audio write failed; partial session: {}",
            dir.display()
        ));
    }
    println!("✓ Audio saved: {}", dir.display());
    for track in &meta.sources {
        if track.peak < 0.0001 {
            eprintln!(
                "Warning: {} contains silence or no samples. Play sound/speak and run audilog doctor.",
                track.file
            );
            permission_help();
        }
        if track.dropped_frames > 0 {
            eprintln!(
                "Warning: {} dropped {} input frames; gaps were padded with silence where possible.",
                track.file, track.dropped_frames
            );
        }
    }
    if let Some(e) = capture_error {
        permission_help();
        return Err(e).context("Capture failed; saved audio was retained");
    }
    if let Some((_, path)) = resolved {
        stop.store(false, Ordering::Relaxed);
        transcribe::run(&dir, &mut meta, &path, cli.cpu, stop)?;
    }
    Ok(())
}
fn transcribe_file(cli: &Cli, input: &Path, stop: Arc<AtomicBool>) -> Result<()> {
    ensure!(input.exists(), "Input does not exist: {}", input.display());
    let (name, model) = model::resolve(cli.model.as_deref(), cli.yes)?;
    let (dir, mut meta) = if input.is_dir() {
        (input.to_path_buf(), session::read_meta(input)?)
    } else {
        let dir = session::create(&root(cli)?)?;
        println!("Session: {}", dir.display());
        let mut meta = initial_meta(cli, name.clone());
        meta.status = "importing".into();
        session::save_meta(&dir, &meta)?;
        let duration = audio::import(input, &dir.join("audio.wav"))?;
        meta.duration_ms = duration;
        meta.sources.push(Track {
            source: AudioSource::File,
            file: "audio.wav".into(),
            device: input.display().to_string(),
            start_offset_ms: 0,
            duration_ms: duration,
            dropped_frames: 0,
            peak: 0.0,
        });
        (dir, meta)
    };
    ensure!(!meta.sources.is_empty(), "Session has no audio tracks");
    meta.model = name;
    if let Some(lang) = &cli.language {
        meta.language = lang.clone();
    }
    ensure!(
        meta.language == "auto" || whisper_rs::get_lang_id(&meta.language).is_some(),
        "Unknown session language"
    );
    meta.status = "transcribing".into();
    session::save_meta(&dir, &meta)?;
    transcribe::run(&dir, &mut meta, &model, cli.cpu, stop)
}
fn doctor(cli: &Cli, stop: Arc<AtomicBool>) -> Result<()> {
    println!(
        "audilog {}\nSystem: {} / {}",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH
    );
    println!(
        "Whisper: {}",
        if cfg!(target_os = "macos") && !cli.cpu {
            "Metal compiled in (runtime availability checked during transcription)"
        } else {
            "CPU"
        }
    );
    let mut ready = true;
    let choice = cli.model.clone().unwrap_or(model::selected()?);
    let model_path = model::path(&choice).unwrap_or_else(|_| PathBuf::from(&choice));
    if model_path.is_file() {
        println!("Model: {} ✓ (file present)", model_path.display());
    } else {
        ready = false;
        println!("Model: missing — audilog model install {choice}");
    }
    let root = root(cli)?;
    std::fs::create_dir_all(&root)?;
    let probe_path = root.join(format!(".audilog-probe-{}", std::process::id()));
    let probe = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe_path)?;
    drop(probe);
    std::fs::remove_file(probe_path)?;
    println!("Storage: {} ✓", root.display());
    println!("Audio probe: speak and play system sound now (3 seconds per source; not saved).");
    let sources = if cfg!(any(target_os = "macos", target_os = "windows")) {
        vec![AudioSource::Microphone, AudioSource::System]
    } else {
        vec![AudioSource::Microphone]
    };
    for source in sources {
        let result = (|| -> Result<(String, usize, f32)> {
            let mut cap = CpalCapture::new(source, cli.input.as_deref())?;
            let (tx, rx) = crossbeam_channel::bounded(256);
            cap.start(tx, Instant::now())?;
            let start = Instant::now();
            let mut count = 0;
            let mut peak = 0.0f32;
            while start.elapsed() < Duration::from_secs(3) && !stop.load(Ordering::Relaxed) {
                if let Ok(err) = cap.errors.try_recv() {
                    return Err(err.into());
                }
                if let Ok(frame) = rx.recv_timeout(Duration::from_millis(100)) {
                    count += frame.samples.len();
                    for s in frame.samples {
                        peak = peak.max(s.abs());
                    }
                }
            }
            cap.stop()?;
            Ok((cap.name, count, peak))
        })();
        match result {
            Ok((name, count, peak)) if count > 0 && peak > 0.0001 => println!(
                "{source:?}: signal detected ✓ ({name}, peak {:.1} dBFS)",
                20.0 * peak.log10()
            ),
            Ok((name, count, _)) => {
                ready = false;
                println!(
                    "{source:?}: {} — permission/signal unconfirmed ({name})",
                    if count == 0 {
                        "no callbacks"
                    } else {
                        "silence"
                    }
                );
            }
            Err(e) => {
                ready = false;
                println!("{source:?}: failed — {e:#}");
            }
        }
    }
    if !ready {
        permission_help();
        bail!("Some checks need attention; see results above");
    }
    println!(
        "Ready. Model integrity/runtime is checked on download/load; audio permissions were tested through capture."
    );
    Ok(())
}
