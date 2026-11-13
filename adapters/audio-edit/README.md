# audio-edit

Changes how audio files sound, each keeping its format (ADR 0004):

- **Even out the loudness** with EBU R128 loudness normalisation (FFmpeg's `loudnorm`, single pass, −16 LUFS, −1.5 dB true peak).
- **Volume** up or down by 3, 6 or 12 dB.
- **Fade in** and **fade out** over 1 to 10 seconds.
- **Speed** from half to double, keeping the pitch (`atempo`).

At least one change is needed. Files are re-encoded once with all the changes: lossy formats at the nearest standard bitrate to their own, lossless ones (WAV and AIFF as 16-bit) without loss beyond the change. The sample rate, channels and tags stay; cover art and extra tracks don't (`STREAMS_DROPPED`).

Engines: FFmpeg.
