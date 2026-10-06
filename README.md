# corrode

Fast JPEG+RAW photo culling in the terminal, with RawTherapee integration and glitch effects. Written in Rust.

> Status: early development. The core library works and can be tried through its examples; there is no interface yet.

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

## Core library

| Module | What it does |
|---|---|
| `pairing` | Groups the JPEG and RAW files of a directory into shots, by base name |
| `pp3` | Reads and writes the marks of a RawTherapee sidecar, keeping every other byte |
| `rawtherapee` | Reads RawTherapee's settings, picks a shot's sidecar and creates it from the default profile |
| `picture` | Decodes a shot upright: the preview embedded in the JPEG or the RAW, or the full image |
| `jpeg` | Finds the EXIF data and the embedded preview in a JPEG without decoding it |
| `exif` | Date, exposure, aperture, ISO, focal length, camera and lens of a shot |

### Trying it

The examples work on a directory of photos. Copy a few shots to `sandbox/`, which git ignores, rather than working on the originals:

```sh
cargo run -p corrode-core --example scan -- sandbox --list                 # how files are paired
cargo run --release -p corrode-core --example info -- sandbox              # marks and shooting information
cargo run -p corrode-core --example marks -- set --rank 4 sandbox/P1011259.JPG
cargo run --release -p corrode-core --example picture -- sandbox/out sandbox/*.JPG
```

`marks set` writes the RawTherapee sidecars of the shots it is given. `picture` saves the decoded pictures in the output directory. Use `--release` for decoding: it is much slower in debug builds.

## Requirements

- Linux, a terminal supporting the Kitty or Sixel graphics protocol (e.g. kitty)
- [RawTherapee](https://rawtherapee.com/), configured to save `.pp3` files next to the input files

## License

Licensed under either of [Apache License 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT), at your option.
