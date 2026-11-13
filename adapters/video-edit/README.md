# video-edit

Rotates (90°, 180°, 270°), flips, speeds up or slows down (a quarter to four times) or mutes videos, each keeping its format (ADR 0004). A rotation flag set by the camera is applied first, so the result plays upright in every player. The sound follows the speed with its pitch kept (`atempo`).

At least one change is needed. Each video is re-encoded once at high quality (H.264 CRF 18 for MP4, MOV and MKV; VP9 for WebM; MPEG-4 Part 2 for AVI), which takes a while for long videos. Only the first video and audio track is kept (`STREAMS_DROPPED`); tags are kept.

Engines: FFmpeg.
