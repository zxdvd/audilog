use anyhow::{Context, Result, bail, ensure};
use directories::{ProjectDirs, UserDirs};
use indicatif::{ProgressBar, ProgressStyle};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{self, IsTerminal, Read, Write},
    path::{Path, PathBuf},
    time::Duration,
};

pub const DEFAULT: &str = "large-v3-turbo-q5_0";
pub const MODELS: &[(&str, &str, u64)] = &[
    (
        "tiny",
        "be07e048e1e599ad46341c8d2a135645097a538221678b7acdd1b1919c6e1b21",
        77691713,
    ),
    (
        "small",
        "1be3a9b2063867b937e64e2ec7483364a79917e157fa98c5d94b5c1fffea987b",
        487601967,
    ),
    (
        "large-v3-turbo",
        "1fc70f774d38eb169993ac391eea357ef47c88757ef72ee5943879b7e8e2bc69",
        1624555275,
    ),
    (
        DEFAULT,
        "394221709cd5ad1f40c46e6031ca61bce88931e6e088c188294c6d5a55ffa7e2",
        574041195,
    ),
];

pub fn dirs() -> Result<ProjectDirs> {
    ProjectDirs::from("", "", "audilog").context("Cannot locate platform directories")
}
pub fn output_root() -> Result<PathBuf> {
    UserDirs::new()
        .and_then(|d| d.document_dir().map(|p| p.join("audilog")))
        .context("Documents directory is unavailable; specify --output PATH")
}
pub fn path(name: &str) -> Result<PathBuf> {
    ensure!(MODELS.iter().any(|m| m.0 == name), "Unknown model: {name}");
    Ok(dirs()?
        .cache_dir()
        .join("models")
        .join(format!("ggml-{name}.bin")))
}
pub fn selected() -> Result<String> {
    let p = dirs()?.config_dir().join("model");
    match fs::read_to_string(&p) {
        Ok(s) => {
            let s = s.trim().to_owned();
            path(&s)?;
            Ok(s)
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(DEFAULT.to_owned()),
        Err(e) => Err(e).with_context(|| format!("Read {}", p.display())),
    }
}
pub fn use_model(name: &str) -> Result<()> {
    ensure!(
        path(name)?.is_file(),
        "Install it first: audilog model install {name}"
    );
    fs::create_dir_all(dirs()?.config_dir())?;
    fs::write(dirs()?.config_dir().join("model"), name)?;
    println!("Default model: {name}");
    Ok(())
}
pub fn list() -> Result<()> {
    let current = selected()?;
    for (name, _, size) in MODELS {
        println!(
            "{name:24} {:>5} MiB  {}{}",
            size / 1024 / 1024,
            if path(name)?.is_file() {
                "installed"
            } else {
                "-"
            },
            if current == *name { " *" } else { "" }
        );
    }
    println!("Models: {}", dirs()?.cache_dir().join("models").display());
    Ok(())
}
pub fn install(name: &str) -> Result<PathBuf> {
    let dest = path(name)?;
    let (_, expected, size) = MODELS.iter().find(|m| m.0 == name).unwrap();
    fs::create_dir_all(dest.parent().unwrap())?;
    // A unique partial file allows simultaneous downloads without clobbering one another.
    let part = dest.with_extension(format!("{}.part", std::process::id()));
    let result = (|| -> Result<()> {
        let mut out = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&part)
            .with_context(|| {
                format!(
                    "Create {}; remove a stale .part file if necessary",
                    part.display()
                )
            })?;
        let client = reqwest::blocking::Client::builder()
            .connect_timeout(Duration::from_secs(30))
            .timeout(Duration::from_secs(3600))
            .build()?;
        let url =
            format!("https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-{name}.bin");
        let mut response = client.get(url).send()?.error_for_status()?;
        let bar = ProgressBar::new(*size);
        bar.set_style(ProgressStyle::with_template(
            "{msg} {wide_bar} {bytes}/{total_bytes}",
        )?);
        bar.set_message(format!("Downloading {name}"));
        let mut hasher = Sha256::new();
        let mut buf = [0u8; 65536];
        let mut total = 0;
        loop {
            let n = response.read(&mut buf)?;
            if n == 0 {
                break;
            }
            total += n as u64;
            ensure!(total <= *size, "Model download exceeds expected size");
            out.write_all(&buf[..n])?;
            hasher.update(&buf[..n]);
            bar.inc(n as u64);
        }
        ensure!(
            total == *size && format!("{:x}", hasher.finalize()) == *expected,
            "Model checksum mismatch; download was not installed"
        );
        out.sync_all()?;
        drop(out);
        fs::rename(&part, &dest)?;
        bar.finish_with_message("Verified");
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&part);
    }
    result?;
    println!("✓ Model installed: {}", dest.display());
    Ok(dest)
}
pub fn resolve(choice: Option<&str>, yes: bool) -> Result<(String, PathBuf)> {
    let name = choice.map(str::to_owned).unwrap_or(selected()?);
    if !MODELS.iter().any(|m| m.0 == name) {
        let p = Path::new(&name);
        ensure!(p.is_file(), "Unknown model or missing model file: {name}");
        return Ok((name.clone(), p.to_path_buf()));
    }
    let p = path(&name)?;
    if !p.exists() {
        if !yes {
            ensure!(
                io::stdin().is_terminal(),
                "Model missing. Run `audilog model install {name}` or pass --yes"
            );
            eprint!(
                "Model {name} is missing ({} MiB). Download? [Y/n] ",
                MODELS.iter().find(|m| m.0 == name).unwrap().2 / 1024 / 1024
            );
            io::stderr().flush()?;
            let mut answer = String::new();
            io::stdin().read_line(&mut answer)?;
            if !matches!(answer.trim().to_lowercase().as_str(), "" | "y" | "yes") {
                bail!("Download cancelled");
            }
        }
        install(&name)?;
    }
    Ok((name, p))
}
