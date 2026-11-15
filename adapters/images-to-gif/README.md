# images-to-gif

Turns two or more pictures into one animated GIF, in the order chosen with the file-order widget. Each picture shows for a chosen time (0.1 to 2 seconds), and the GIF can play once or forever. Frames are the chosen width (320 to 800 pixels) with the first picture's proportions; pictures of other shapes are fitted inside and centred on white. GIF holds 256 colours per frame (`COLORS_REDUCED`), and an animated input contributes its first frame (`ANIMATION_FLATTENED`).

Engines: Pillow, pillow-heif.
