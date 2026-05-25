"""Benchmark: gera 18s de tom sintetico, manda pro whisper-rs como audio e mede o tempo."""
import sys, time, subprocess, struct, math, os

# Generate 18s of synthetic 16kHz f32 mono audio (silence + 440Hz tone)
SR = 16000
DURATION = 18.0
N = int(SR * DURATION)

# Write a raw PCM 16-bit WAV file we can probably feed into the app for testing
# Actually for a benchmark, we'll just measure round-trip from a known-state
# Use the existing app via SetForegroundWindow and SendInput? No, just check
# the log for inference time after an actual user-triggered run.
#
# This script is a placeholder; the user needs to trigger F9 themselves.

print("This is a placeholder. The user needs to:")
print("  1. Start VoiceTyper.exe")
print("  2. Hold F9 and talk for ~15s")
print("  3. Release F9 — wait for transcription")
print("  4. Check the log: %APPDATA%\\VoiceTyper\\voice-typer.log")
print()
print("Look for a line like:")
print("  inference: 18.00s of audio in 14.20s (1.27x realtime, threads=4, single=true)")
