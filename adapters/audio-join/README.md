# audio-join

Joins two or more audio files into one, in the order chosen with the file-order widget, with optional silence (up to 3 seconds) between them. The result is saved in the chosen format (MP3, M4A, WAV, FLAC, Ogg, Opus, AAC or AIFF), at the chosen bitrate for lossy formats (ADR 0004: `outputs.format: chosen`).

Every file is resampled to the highest sample rate among them, at most 48 kHz (`AUDIO_RESAMPLED` when any differs), and mixed to stereo if any file is stereo. Only the first audio track of each file is used: cover art, other tracks and surround channels beyond stereo aren't kept (`STREAMS_DROPPED`). Tags aren't carried over, since they'd describe only one of the files.

Engines: FFmpeg.
