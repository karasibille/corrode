#!/bin/sh
# Remakes the pictures of this folder from photo.jpg and recipes.txt:
#   sh docs/examples/generate.sh
# Each line of recipes.txt is `name | recipe`; the picture is name.jpg,
# made from photo.jpg then reduced to 480 pixels wide and cut to a square
# near the top (ImageMagick), so that they line up and stay light in the
# repository. The animation glitch-mix.gif is made with glitchgif.
set -e
cd "$(dirname "$0")/../.."
cargo build --release -p corrode-core --example effects
grep -v '^#' docs/examples/recipes.txt | while IFS='|' read -r name recipe; do
    name=$(echo "$name" | tr -d ' ')
    # shellcheck disable=SC2086
    target/release/examples/effects docs/examples/photo.jpg "docs/examples/$name.jpg" seed=7 $recipe
    magick "docs/examples/$name.jpg" -resize 480x -gravity north -crop 480x480+0+60 +repage -strip -quality 70 "docs/examples/$name.jpg"
done
cargo build --release -p corrode-core --example glitchgif
target/release/examples/glitchgif docs/examples/photo.jpg docs/examples/glitch-mix.gif frames=14 fps=10 width=400 seed=11 \
    falsecolor order=rgb rotate=-70..70 saturation=130 \
    + slice slices=4..10 shift=10..70 split=true \
    + aberration amount=0.003..0.012 \
    + spin angle=0..2 center=50,45 \
    + scanlines period=3 strength=25
