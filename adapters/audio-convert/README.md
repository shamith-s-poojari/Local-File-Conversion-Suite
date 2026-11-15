# audio-convert

Converts between MP3, WAV, FLAC, OGG (Vorbis), Opus, M4A (AAC), AAC and AIFF with FFmpeg. One output per input, named after it, with its tags (title, artist, album) kept.

| Behaviour | Detail |
|---|---|
| Lossless targets | WAV, FLAC and AIFF keep every decoded sample; 24-bit sources stay 24-bit |
| Lossy targets | MP3, OGG, Opus, M4A and AAC at the chosen bitrate (192 kbps by default) |
| AAC ↔ M4A | Moved between containers without re-encoding when nothing else changes |
| Opus | Resampled to 48 kHz when the source rate isn't one Opus supports (`AUDIO_RESAMPLED`) |
| MP3 | Surround is mixed down to stereo (`AUDIO_DOWNMIXED`) |
| Cover art, extra tracks | Only the first audio track is kept (`STREAMS_DROPPED`) |
| Trim | Optional start and end times |
| Preview | A waveform picture |

Every input is opened with `-protocol_whitelist file`, and output is bit-exact so repeat runs match. Engine: FFmpeg (separate process, granted per adapter by the sandbox).
