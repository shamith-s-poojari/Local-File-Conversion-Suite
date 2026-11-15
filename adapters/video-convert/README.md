# video-convert

Converts video between MP4, MOV, MKV, WebM and AVI, makes animated GIFs from video (and video from GIFs), and saves a video's soundtrack as MP3, WAV, FLAC, OGG, Opus, M4A, AAC or AIFF. Uses FFmpeg.

| Target | Video | Audio |
|---|---|---|
| MP4, MOV, MKV | H.264 (CRF 18/23/28), or copied unchanged when "Same as the original" and the codec fits | AAC 160 kbps, or copied |
| WebM | VP9 (CRF 28/33/38) | Opus 128 kbps |
| AVI | MPEG-4 Part 2 (Xvid tag) | MP3 192 kbps |
| GIF | Palette per clip, Bayer dithering, chosen width and frame rate | none |

Resolution only ever shrinks, keeping the aspect ratio and even dimensions. Start and end times trim the clip (trimming always re-encodes, so cuts land on the exact frame). Subtitles, data tracks and extra audio tracks aren't kept (`STREAMS_DROPPED`). The preview is a frame a tenth of the way in.

Engine: FFmpeg (separate process, granted per adapter by the sandbox).
