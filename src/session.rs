use anyhow::{Context, Result, ensure};
use chrono::Local;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AudioSource {
    Microphone,
    System,
    File,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Track {
    pub source: AudioSource,
    pub file: String,
    pub device: String,
    pub start_offset_ms: u64,
    pub duration_ms: u64,
    pub dropped_frames: u64,
    pub peak: f32,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct Meta {
    pub version: u8,
    pub started_at: String,
    pub duration_ms: u64,
    pub status: String,
    pub sources: Vec<Track>,
    pub model: String,
    pub language: String,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct Segment {
    pub start_ms: u64,
    pub end_ms: u64,
    pub source: AudioSource,
    pub text: String,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct Transcript {
    pub version: u8,
    pub segments: Vec<Segment>,
}

pub fn create(root: &Path) -> Result<PathBuf> {
    fs::create_dir_all(root).with_context(|| format!("Create {}", root.display()))?;
    let stem = Local::now().format("%Y-%m-%d_%H-%M-%S").to_string();
    for n in 0..10000 {
        let p = root.join(if n == 0 {
            stem.clone()
        } else {
            format!("{stem}-{n}")
        });
        match fs::create_dir(&p) {
            Ok(()) => return Ok(p),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.into()),
        }
    }
    anyhow::bail!("Too many sessions with the same timestamp")
}
pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let part = path.with_extension("tmp");
    let mut f = fs::File::create(&part)?;
    f.write_all(bytes)?;
    f.sync_all()?;
    drop(f);
    fs::rename(part, path)?;
    Ok(())
}
pub fn save_meta(dir: &Path, meta: &Meta) -> Result<()> {
    atomic_write(&dir.join("meta.json"), &serde_json::to_vec_pretty(meta)?)
}
pub fn read_meta(dir: &Path) -> Result<Meta> {
    let meta: Meta = serde_json::from_slice(&fs::read(dir.join("meta.json"))?)?;
    ensure!(meta.version == 1, "Unsupported session version");
    for t in &meta.sources {
        ensure!(
            Path::new(&t.file).components().count() == 1 && !t.file.starts_with('.'),
            "Invalid track filename"
        );
    }
    Ok(meta)
}
pub fn timestamp(ms: u64) -> String {
    format!(
        "{:02}:{:02}:{:02}.{:03}",
        ms / 3600000,
        ms / 60000 % 60,
        ms / 1000 % 60,
        ms % 1000
    )
}
pub fn save_transcript(dir: &Path, mut segments: Vec<Segment>) -> Result<()> {
    segments.sort_by_key(|s| s.start_ms);
    let mut md = String::from("# Transcript\n\n");
    for s in &segments {
        let source = match s.source {
            AudioSource::Microphone => "microphone",
            AudioSource::System => "system",
            AudioSource::File => "file",
        };
        md.push_str(&format!(
            "**[{} → {}] {source}**\n\n{}\n\n",
            timestamp(s.start_ms),
            timestamp(s.end_ms),
            s.text.trim()
        ));
    }
    atomic_write(
        &dir.join("transcript.json"),
        &serde_json::to_vec_pretty(&Transcript {
            version: 1,
            segments,
        })?,
    )?;
    atomic_write(&dir.join("transcript.md"), md.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sessions_never_overwrite_and_transcript_sorts_sources() {
        let root = tempfile::tempdir().unwrap();
        let a = create(root.path()).unwrap();
        let b = create(root.path()).unwrap();
        assert_ne!(a, b);
        save_transcript(
            &a,
            vec![
                Segment {
                    start_ms: 2000,
                    end_ms: 3000,
                    source: AudioSource::Microphone,
                    text: "你好".into(),
                },
                Segment {
                    start_ms: 500,
                    end_ms: 1500,
                    source: AudioSource::System,
                    text: "Hello".into(),
                },
            ],
        )
        .unwrap();
        let t: Transcript =
            serde_json::from_slice(&fs::read(a.join("transcript.json")).unwrap()).unwrap();
        assert_eq!(t.segments[0].source, AudioSource::System);
        let md = fs::read_to_string(a.join("transcript.md")).unwrap();
        assert!(md.find("Hello").unwrap() < md.find("你好").unwrap());
        assert_eq!(timestamp(3728123), "01:02:08.123");
    }
    #[test]
    fn session_rejects_track_path_escape() {
        let d = tempfile::tempdir().unwrap();
        fs::write(d.path().join("meta.json"), r#"{"version":1,"started_at":"now","duration_ms":0,"status":"recorded","model":"tiny","language":"auto","sources":[{"source":"file","file":"../outside.wav","device":"","start_offset_ms":0,"duration_ms":0,"dropped_frames":0,"peak":0}]}"#).unwrap();
        assert!(read_meta(d.path()).is_err());
    }
}
