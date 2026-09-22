fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    // A CLI has no .app bundle; embed the usage descriptions in its Mach-O instead.
    let path = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap()).join("Info.plist");
    let version = std::env::var("CARGO_PKG_VERSION").unwrap();
    let plist = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>io.github.zxdvd.audilog</string>
<key>CFBundleName</key><string>audilog</string>
<key>CFBundleVersion</key><string>{version}</string>
<key>NSMicrophoneUsageDescription</key><string>Record microphone audio to local WAV files for offline transcription.</string>
<key>NSAudioCaptureUsageDescription</key><string>Record system audio to local WAV files for offline transcription.</string>
</dict></plist>"#
    );
    std::fs::write(&path, plist).unwrap();
    println!(
        "cargo:rustc-link-arg-bin=audilog=-Wl,-sectcreate,__TEXT,__info_plist,{}",
        path.display()
    );
}
