# corrode

Fast JPEG+RAW photo culling in the terminal, with RawTherapee integration and glitch effects. Written in Rust.

> Status: early development — nothing usable yet.

## Goals

- Browse the JPEGs of a shoot, with 100% zoom to check sharpness
- Rate, color-label and reject photos from the keyboard
- Store marks in RawTherapee `.pp3` sidecars (`Rank`, `ColorLabel`, `InTrash`), no proprietary database
- Open the matching RAW files in RawTherapee
- Later: group photos by color/brightness, apply presets in bulk, creative glitch effects

## Workspace

| Crate | Role |
|---|---|
| `core/` (`corrode-core`) | JPEG↔RAW pairing, `.pp3` read/write, decoding, cache, EXIF, effects |
| `tui/` (`corrode-tui`) | Terminal interface (`ratatui` + `ratatui-image`), builds the `corrode` binary |

## Requirements

- Linux, a terminal supporting the Kitty or Sixel graphics protocol (e.g. kitty)
- [RawTherapee](https://rawtherapee.com/), configured to save `.pp3` files next to the input files

## License

Licensed under either of [Apache License 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT), at your option.
