use std::process::Command;
fn cmd() -> Command {
    Command::new(env!("CARGO_BIN_EXE_audilog"))
}
#[test]
fn cli_help_and_validation_do_not_need_audio_or_network() {
    let result = cmd().arg("--help").output().unwrap();
    assert!(result.status.success());
    let help = String::from_utf8(result.stdout).unwrap();
    for s in [
        "record",
        "devices",
        "doctor",
        "transcribe",
        "--system-only",
        "--language",
    ] {
        assert!(help.contains(s));
    }
    assert!(cmd().arg("--version").status().unwrap().success());
    for args in [
        vec!["--system", "--system-only"],
        vec!["--system-only", "--input", "x"],
        vec!["--seconds", "0"],
        vec!["--language", "invalid-lang"],
        vec!["transcribe", "this-file-does-not-exist-123.wav"],
    ] {
        assert!(!cmd().args(args).output().unwrap().status.success());
    }
}
