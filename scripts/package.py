"""Create portable release archives with a version check and SHA-256 checksum."""
import hashlib
import pathlib
import shutil
import subprocess
import sys
import tarfile
import tempfile
import zipfile

target, version = sys.argv[1:]
is_windows = "windows" in target
binary = "audilog.exe" if is_windows else "audilog"
source = pathlib.Path("target/release") / binary
actual = subprocess.check_output([str(source.resolve()), "--version"], text=True).strip()
assert actual == f"audilog {version.removeprefix('v')}", (actual, version)
dist = pathlib.Path("dist")
dist.mkdir(exist_ok=True)
name = f"audilog-{version}-{target}"
with tempfile.TemporaryDirectory() as tmp:
    stage = pathlib.Path(tmp) / name
    stage.mkdir()
    for file in [source, pathlib.Path("README.md"), pathlib.Path("LICENSE")]:
        shutil.copy2(file, stage / file.name)
    if sys.platform == "darwin":
        # Ad-hoc signing is required for Apple Silicon; Developer ID notarization is separate.
        subprocess.run(["codesign", "--force", "--sign", "-", str(stage / binary)], check=True)
    archive = dist / (name + (".zip" if is_windows else ".tar.gz"))
    if is_windows:
        with zipfile.ZipFile(archive, "w", zipfile.ZIP_DEFLATED) as z:
            for file in stage.iterdir():
                z.write(file, f"{name}/{file.name}")
    else:
        with tarfile.open(archive, "w:gz") as tar:
            tar.add(stage, arcname=name)
    digest = hashlib.sha256(archive.read_bytes()).hexdigest()
    archive.with_name(archive.name + ".sha256").write_text(f"{digest}  {archive.name}\n")
    print(archive)
