# image-compress

Makes each image smaller than a chosen size (100 KB to 2 MB), keeping its format (ADR 0004). JPG, WebP and AVIF are re-encoded at falling quality (85 down to 35) until one fits; PNG is first re-compressed, then reduced to 256 colours (`COLORS_REDUCED`). If that isn't enough, the picture shrinks step by step (`IMAGE_DOWNSCALED`), but never below 64 pixels on its shorter side (`TARGET_NOT_REACHED`).

A file already under the target, or one the tool can't make smaller, is kept byte for byte (`KEPT_ORIGINAL`). Camera details are removed (`METADATA_DROPPED`); the colour profile is kept. Animated WebP keeps only its first frame (`ANIMATION_FLATTENED`).

Engines: Pillow.
