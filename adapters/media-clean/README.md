# media-clean

Removes hidden details from audio and video files, one output per input, each in its own format (ADR 0004). Names are `{title}-clean.{ext}`.

FFmpeg copies every stream without re-encoding and leaves behind the global metadata, chapters and the tags of each stream: location, phone make and model, artist, dates, titles, comments. Sound and picture are exactly what they were. The option `removeCoverArt` (default off) also drops the cover picture embedded in a song or video.

`DETAILS_REMOVED` is warned when something was removed. Each output is read again, and the job fails with `E_INTERNAL` (`detailsRemain`) if a place, device, author or date is still found in it.

Engines: FFmpeg.
