# video-join

Joins two or more clips into one MP4, MOV, MKV or WebM, in the order chosen with the file-order widget (ADR 0004: `outputs.format: chosen`).

- **Matching clips** (same video codec, size, pixel format and frame rate, and the same sound codec, sample rate and channels), in a container that can hold them, are joined with FFmpeg's concat demuxer without re-encoding: fast, and nothing is lost.
- **Other clips** are fitted into the first clip's frame (letterboxed or pillarboxed where the shapes differ), brought to its frame rate (at most 60), given silence where they have no sound, and encoded once at high quality (H.264 CRF 18 and AAC; VP9 and Opus for WebM). This takes longer and loses a little quality (`REENCODED`).

Only the first video and audio track of each clip is used; subtitles, data tracks and other audio tracks aren't kept (`STREAMS_DROPPED`). Tags aren't carried over.

Engines: FFmpeg.
