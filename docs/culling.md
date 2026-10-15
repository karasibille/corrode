# Culling with corrode select

[Back to the README](../README.md)

```sh
corrode select path/to/shoot
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
| G | Sort the shots of `selection/` into folders of shots that look alike, named after the colour of the light and the first shot (`bleu-IMG_0042`); asks first |
| d / D | Remove the light bands of the RAW into `<name>-deband.dng` next to it, shown once written (D: even when none were found) |
| z, Enter | 100% zoom; arrows then move around, Ctrl+arrows change shot at the same spot to compare. A shot without JPEG is developed from its RAW, with the exposure of the camera's own preview, colours kept inside the screen's gamut and highlights rolled off, so it looks close to the camera's JPEG but is not RawTherapee's rendering |
| + / - | Zoom in (200, 400, 800%) / out |
| o / O | Open the shot / the directory in RawTherapee |
| ? | Full help |
| Esc, q | Leave the zoom / quit |

There is nothing to save: marks are written as they are set. The line above the keys counts kept (✓), rejected (✗) and unsorted (?) shots, and says "all sorted" once nothing is left; quitting prints the same summary. Files are only read, except the sidecars that marks are written to. Directories on a spinning disk are read in the background: bursts take shape around the current shot within seconds to half a minute the first time. What the files told is kept in `~/.cache/corrode` (about 5 KB per shot), so that a directory seen before opens at once; an entry is dropped when its file changes.
