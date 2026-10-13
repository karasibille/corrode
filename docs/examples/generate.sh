#!/bin/sh
# Remakes the pictures of this folder from photo.jpg and recipes.txt:
#   sh docs/examples/generate.sh
# Each line of recipes.txt is `name | recipe`; the picture is name.jpg,
# made from photo.jpg then reduced to 480 pixels wide (ImageMagick) to
# stay light in the repository.
set -e
cd "$(dirname "$0")/../.."
cargo build --release -p corrode-core --example effects
grep -v '^#' docs/examples/recipes.txt | while IFS='|' read -r name recipe; do
    name=$(echo "$name" | tr -d ' ')
    # shellcheck disable=SC2086
    target/release/examples/effects docs/examples/photo.jpg "docs/examples/$name.jpg" seed=7 $recipe
    magick "docs/examples/$name.jpg" -resize 480x -strip -quality 70 "docs/examples/$name.jpg"
done
