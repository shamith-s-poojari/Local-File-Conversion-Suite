# media-clean

Removes hidden details from audio and video files, one output per input, each in its own format (ADR 0004). Names are `{title}-clean.{ext}`.

FFmpeg copies every stream without re-encoding and leaves behind the global metadata, chapters and the tags of each stream: location, phone make and model, artist, dates, titles, comments. Sound and picture are exactly what they were. Data streams (GPS tracks, sensor readings, timecode) are not copied, always. Attachment streams (a font in a Matroska file) keep their `filename` and `mimetype` tags so players can still find them; their other tags are cleared. The option `removeCoverArt` (default off) also drops the cover picture embedded in a song or video.

`DETAILS_REMOVED` is warned when something was removed. Each output is read again, and the job fails with `E_INTERNAL` (`detailsRemain`) if a place, device, author or date, or a data stream, is still found in it.

Engines: FFmpeg.
