# The crates and the core library

[Back to the README](../README.md)

## Workspace

| Crate | Role |
|---|---|
| `core/` (`corrode-core`) | JPEG↔RAW pairing, `.pp3` sidecars and presets, picture decoding, EXIF, similarity, light bands |
| `effects/` (`corrode-effects`) | The creative effects and their recipes; depends on `image` and `rayon` only, not on the RAW decoder |
| `ui/` (`corrode-ui`) | What the terminal interfaces share: background jobs, terminal graphics, lines that fit |
| `select/` (`corrode-select`) | The culling viewer in the terminal (`ratatui` + `ratatui-image`) |
| `preset/` (`corrode-preset`) | Applying a RawTherapee preset to a whole selection, group by group |
| `cli/` (`corrode`) | The `corrode` program: `select`, `group`, `preset`, `effect`, `effects`, `gif`, `bands` and `deband`, one command for the other crates |

## Core library

The creative effects are not here: they live in [`corrode-effects`](../effects/). The `corrode effect` and `corrode gif` commands load a shot with the decoder of `core` and apply the effects to it.

| Module | What it does |
|---|---|
| `pairing` | Groups the JPEG and RAW files of a directory into shots, by base name |
| `marks` | The rating, color label and rejection culling sets on a shot |
| `pp3` | Reads and writes a RawTherapee sidecar: the marks, any key, a whole section; every other byte is kept |
| `rawtherapee` | Reads RawTherapee's settings, picks a shot's sidecar, creates it from the default profile, opens RawTherapee |
| `exif` | Date to the millisecond, exposure, aperture, ISO, focal length, camera, lens and focus point, from the head of a file |
| `picture` | Decodes a shot upright: its thumbnail, the preview embedded in the JPEG or the RAW, or the full image |
| `formats` | What a TIFF structure, a JPEG and a RW2 file hold and where, without decoding |
| `cameras` | What is specific to a make: the Panasonic focus point and its orientation quirk |
| `cache` | Keeps the shooting information and thumbnails between sessions, one file per directory |
| `bursts` | Groups shots taken in quick succession |
| `selection` | Moves the kept shots, with their sidecars, to the shoot's `selection/` folder, then sorts them into subfolders by group |
| `similarity` | Tells how much shots look alike (colour, layout, light, time taken), groups them and names the colour of each group's light |
| `look` | Puts the look of a reference profile on other profiles, keeping the marks, framing and white balance of each shot |
| `preset` | Applies a preset to the groups of a selection, with an exposure and settings of their own for each |
| `sharpness` | Scores the sharpness of a picture around its focus point |
| `banding` | Detects the light bands LED lighting leaves with an electronic shutter |
| `debanding` | Removes those bands from the raw sensor data, per color, by their period |
| `dng` | Writes a raw image as a DNG with the original's metadata and preview |

## Trying it

The examples work on a directory of photos. Copy a few shots to `sandbox/`, which git ignores, rather than working on the originals:

```sh
cargo run -p corrode-core --example scan -- sandbox --list                 # how files are paired
cargo run --release -p corrode-core --example info -- sandbox              # marks and shooting information
cargo run -p corrode-core --example marks -- set --rank 4 sandbox/photo.JPG
cargo run --release -p corrode-core --example picture -- sandbox/out sandbox/*.JPG
cargo run --release -p corrode-core --example bursts -- sandbox --list
cargo run --release -p corrode-core --example sharpness -- sandbox
```

The tools made for a shoot are subcommands of `corrode`, run the same way on `sandbox/`: `corrode bands sandbox` tells which shots have light bands, `corrode deband sandbox/banded-shot.RW2` removes them into a DNG, and `corrode group sandbox --sheets sandbox/out` groups the shots that look alike.

`marks set` writes the RawTherapee sidecars of the shots it is given. `picture` saves the decoded pictures in the output directory. Use `--release` for decoding: it is much slower in debug builds.
