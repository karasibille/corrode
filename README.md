# corrode

Fast JPEG+RAW photo culling in the terminal, with RawTherapee integration and glitch effects. Written in Rust.

> Status: early development. Culling works in the terminal; effects and bulk presets are still to come.

## Goals

- Browse a shoot, with or without JPEGs, with 100% zoom to check sharpness
- Rate, color-label and reject photos from the keyboard
- Store marks in RawTherapee `.pp3` sidecars (`Rank`, `ColorLabel`, `InTrash`), no proprietary database
- Open the matching RAW files in RawTherapee
- Later: group photos by color/brightness, apply presets in bulk, creative glitch effects

## Workspace

| Crate | Role |
|---|---|
| `core/` (`corrode-core`) | JPEG↔RAW pairing, `.pp3` marks, picture decoding, EXIF; later effects |
| `tui/` (`corrode-tui`) | Terminal interface (`ratatui` + `ratatui-image`), builds the `corrode` binary |

## Culling

```sh
cargo run --release -p corrode-tui -- path/to/shoot
```

corrode shows the shots of a directory one at a time, with the shot's marks and settings on top and a strip of the burst it belongs to below: shots taken less than 300 ms apart. Under each thumbnail are its marks, and ◆ points out the sharpest frame of the burst, measured around the camera's focus point. Marks are written to the RawTherapee sidecars as soon as they are set.

| Key | Action |
|---|---|
| ← → (h l, space, page keys) | Previous / next shot |
| ↑ ↓ ([ ]) | Previous / next burst |
| s | Sharpest frame of the burst |
| k | Keep the current shot, reject the rest of the burst, go to the next burst |
| X | Reject the whole burst, go to the next burst |
| 1–5, 0 (or & é " ' ( à) | Rating, cleared by 0 |
| r y g b p | Red, yellow, green, blue, purple label (again to clear) |
| x, Delete | Reject / restore |
| f | Filter: all, unsorted (left to cull), kept, rejected |
| m | Move the kept shots, whole (JPEG, RAW, sidecars), to a `selection/` folder next to them, to open in RawTherapee; asks first |
| d / D | Remove the light bands of the RAW into `<name>-deband.dng` next to it, shown once written (D: even when none were found) |
| z, Enter | 100% zoom; arrows then move around, Ctrl+arrows change shot at the same spot to compare |
| + / - | Zoom in (200, 400, 800%) / out |
| o / O | Open the shot / the directory in RawTherapee |
| ? | Full help |
| Esc, q | Leave the zoom / quit |

There is nothing to save: marks are written as they are set. The line above the keys counts kept (✓), rejected (✗) and unsorted (?) shots, and says "all sorted" once nothing is left; quitting prints the same summary. Files are only read, except the sidecars that marks are written to. Directories on a spinning disk are read in the background: bursts take shape around the current shot within seconds to half a minute the first time. What the files told is kept in `~/.cache/corrode` (about 5 KB per shot), so that a directory seen before opens at once; an entry is dropped when its file changes.

## Core library

| Module | What it does |
|---|---|
| `pairing` | Groups the JPEG and RAW files of a directory into shots, by base name |
| `marks` | The rating, color label and rejection culling sets on a shot |
| `pp3` | Reads and writes the marks of a RawTherapee sidecar, keeping every other byte |
| `rawtherapee` | Reads RawTherapee's settings, picks a shot's sidecar, creates it from the default profile, opens RawTherapee |
| `exif` | Date to the millisecond, exposure, aperture, ISO, focal length, camera, lens and focus point, from the head of a file |
| `picture` | Decodes a shot upright: its thumbnail, the preview embedded in the JPEG or the RAW, or the full image |
| `formats` | What a TIFF structure, a JPEG and a RW2 file hold and where, without decoding |
| `cameras` | What is specific to a make: the Panasonic focus point and its orientation quirk |
| `cache` | Keeps the shooting information and thumbnails between sessions, one file per directory |
| `bursts` | Groups shots taken in quick succession |
| `selection` | Moves the kept shots, with their sidecars, to the shoot's `selection/` folder |
| `sharpness` | Scores the sharpness of a picture around its focus point |
| `banding` | Detects the light bands LED lighting leaves with an electronic shutter |
| `debanding` | Removes those bands from the raw sensor data, per color, by their period |
| `dng` | Writes a raw image as a DNG with the original's metadata and preview |

### Trying it

The examples work on a directory of photos. Copy a few shots to `sandbox/`, which git ignores, rather than working on the originals:

```sh
cargo run -p corrode-core --example scan -- sandbox --list                 # how files are paired
cargo run --release -p corrode-core --example info -- sandbox              # marks and shooting information
cargo run -p corrode-core --example marks -- set --rank 4 sandbox/P1011259.JPG
cargo run --release -p corrode-core --example picture -- sandbox/out sandbox/*.JPG
cargo run --release -p corrode-core --example bursts -- sandbox --list
cargo run --release -p corrode-core --example sharpness -- sandbox
cargo run --release -p corrode-core --example deband -- sandbox/banding/109/_1094086.RW2
```

`marks set` writes the RawTherapee sidecars of the shots it is given. `picture` saves the decoded pictures in the output directory. Use `--release` for decoding: it is much slower in debug builds.

## Requirements

- Linux, a terminal supporting the Kitty or Sixel graphics protocol (e.g. kitty)
- [RawTherapee](https://rawtherapee.com/), configured to save `.pp3` files next to the input files

## License

Licensed under either of [Apache License 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT), at your option.
