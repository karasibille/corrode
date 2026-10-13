# Presets with corrode-preset

[Back to the README](../README.md)

Sort the selection into groups first, with `G` in [`corrode-select`](culling.md), or without the interface:

```sh
cargo run --release -p corrode-core --example similar -- selection/ --sheets /tmp/sheets   # list the groups, contact sheets to check them
cargo run --release -p corrode-core --example similar -- selection/ --move                 # then sort the shots into folders
```

Then retouch one shot in RawTherapee: its profile is the preset, the `.pp3` sidecar next to it (`shot.RW2.pp3`) or a profile saved from RawTherapee. Apply it to every group:

```sh
cargo run --release -p corrode-preset -- selection/ --preset my-preset.pp3 --ref selection/reference-shot.RW2            # what would change
cargo run --release -p corrode-preset -- selection/ --preset my-preset.pp3 --ref selection/reference-shot.RW2 --save-adjustments adjustments.txt
cargo run --release -p corrode-preset -- selection/ --preset my-preset.pp3 --ref selection/reference-shot.RW2 --adjustments adjustments.txt --apply
```

Each folder is a group, and each shot left at the top level is a group of its own. Each group gets the preset with its own exposure compensation, worked out from how light or dark the group is next to the shot the preset was made on (`--ref`). Any other setting can be changed per group in a small file (`[group]`, then `Section.Key=value`). The marks, crop, rotation and white balance of each shot are kept. Nothing is written without `--apply`, and each sidecar is copied to a `.bak` first.
