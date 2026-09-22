First usable release of audilog: record audio and transcribe locally.

- Default microphone recording; Ctrl+C saves audio and runs embedded Whisper.
- Microphone + system audio or system-only recording on macOS 14.6+ / Windows.
- Timestamped JSON and Markdown transcripts, durable session metadata, and retry from saved audio.
- Local GGML model management with SHA-256 verification; default large-v3-turbo-q5_0 (~547 MiB).
- Metal inference on macOS; CPU fallback; common audio file import.
- `doctor` probes actual audio signal and checks model/storage readiness.

Download the archive for your OS/architecture and verify it against SHA256SUMS.
Run `audilog model install`, then `audilog doctor`, then `audilog --language zh`.

macOS is the primary V1 platform. Microphone and system-audio access must be granted to the launching terminal in macOS Privacy & Security settings. Archives use ad-hoc signing and are not Apple-notarized. Windows/Linux are CI-built; physical audio hardware on those platforms has not been verified. Linux system capture is not supported in V1.

No GUI, cloud upload, summary, real-time subtitles, or speaker diarization. See README for installation and known limitations.
