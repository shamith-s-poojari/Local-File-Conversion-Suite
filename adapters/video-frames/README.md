# video-frames

Saves pictures of a video's frames as JPG or PNG (ADR 0004: `outputs.format: chosen`), either a number of frames spread evenly through the video (1 to 50) or one every few seconds (1 second to a minute), at the video's size or scaled down to 1920, 1280 or 640 pixels wide. Each picture is named after its time in the video, such as `clip-01m05s.jpg`, with tenths of a second (`clip-00m01.4s.jpg`) when frames are under a second apart. A camera's rotation flag is applied, so pictures come out upright.

Each frame is taken with a fast seek followed by an exact decode, so taking a few frames from a long video is quick. One video can give at most 1,000 pictures.

With the layout "contact sheet" each video gives one picture instead, `clip-sheet.jpg` (or `.png`): every frame in a grid, captioned with its time and titled with the video's name. A sheet holds 4 to 36 frames, which is checked before the job starts; each frame is decoded at thumbnail size (up to 480 px wide), so the sheet doesn't depend on the "Size" setting.

Engines: FFmpeg, and Pillow for contact sheets.
