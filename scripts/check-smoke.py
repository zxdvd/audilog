"""Check a real Whisper inference result, including timestamp bounds."""
import json
import pathlib
import sys

sessions = sorted(pathlib.Path(sys.argv[1]).glob("*/transcript.json"))
assert sessions, "No transcript was saved"
data = json.loads(sessions[-1].read_text())
segments = data["segments"]
text = " ".join(s["text"] for s in segments).lower()
assert "country" in text and "ask" in text, text
assert all(0 <= s["start_ms"] < s["end_ms"] <= 12000 for s in segments), segments
assert all(s["source"] == "file" for s in segments)
print("Real transcription verified:", text)
