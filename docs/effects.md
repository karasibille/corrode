# Effects

[Back to the README](../README.md)

The effects turn a picture into something else on purpose: the wear of a compression, the faults of a print or a screen, the colours of another film. A recipe is a few words, `seed=7 sort low=40 + slice split=true`, and the same recipe on the same picture always gives the same result. Every picture of the [gallery](../README.md#gallery) comes from [`docs/examples/photo.jpg`](examples/photo.jpg), with `seed=7` and the recipe given in its fold-out:

```sh
corrode effect docs/examples/photo.jpg result.jpg seed=7 falsecolor order=rgb rotate=120
```

`corrode effects` lists the effects and `corrode effects <effect>` tells the parameters of one, with their defaults. Sizes are in pixels for a picture 1000 pixels wide and grow with the picture, so a recipe looks the same on a preview and on the full photo. Colours are written `ff4fa3`, by name (`red`, `rouge`, `cyan`…) or as a hue (`210deg`). `sh docs/examples/generate.sh` makes these pictures again from [`recipes.txt`](examples/recipes.txt).

## What each effect does

**Faults of digital files**: what happens to a picture when its data goes wrong.

- `loss` (generation loss): the picture is saved as a JPEG over and over, moved a little each time, as a photo copied and shared a hundred times. Colours bleed and blocks appear.
- `bend` (databending): a few bytes of the compressed file are changed on purpose, then the file is read again. Whole bands of the picture take the wrong colour.
- `sort` (pixel sorting): runs of pixels are put in order from dark to bright, which melts the picture into streaks.
- `slice` (slice shift): horizontal slices are pushed sideways, as on a TV that loses its signal, with their colours coming apart or inverted if asked.
- `stretch` (pixel stretch): a pixel is smeared along its row, or its column, into a long streak.
- `split` (channel split): the red, green and blue of the picture are moved apart, leaving coloured edges, like a badly printed picture.

**Print and screens**: a picture turned into ink or light.

- `dither`: the picture is reduced to black and white dots, as in a fax or a newspaper, either scattered (`diffusion`) or in a regular pattern (`bayer`).
- `duotone`: the picture is printed with two inks, one for the shadows and one for the lights.
- `tritone`: the same with three inks, for the shadows, the mid tones and the lights.
- `scanlines`: dark horizontal lines over the picture, as on an old tube screen.

**Film and light**: what a camera, a lens or a roll of film does by accident.

- `drag`: a flash on a slow shutter. The picture stays sharp and a trail of light follows it, sideways (`motion`) or from the centre (`zoom`).
- `aberration`: the lens does not focus all colours at the same place, so red and blue edges grow towards the corners.
- `grain`: the speckle of fast film.
- `fade`: the look of an old print, with the blacks lifted, the colours washed and a warm tint.
- `vignette`: the corners get darker.
- `leak`: a coloured glow comes in from an edge, as if light had reached the film.
- `bloom`: the brightest lights glow, with a halo around them.

**Distortion**: the picture is moved, not recoloured.

- `liquid`: waves run through the picture, as if seen through moving water.
- `spin`: the picture is smeared round a point, as if the camera had turned during the shot; with a few steps, it leaves separate ghosts.

**Colour**: the colours moved on purpose.

- `falsecolor`: the colours are swapped or turned round the colour wheel (red and blue swapped is the look of infrared film). It can make the colour stronger, and be limited to one range of colours so that the rest stays as it was.
- `remap`: several colours each sent their own way at once, such as the sky towards violet and the grass towards red, without one change feeding another.
- `onepoint` (one point colour): one colour is kept and everything else goes grey, as a yellow that stays vivid in a black and white picture.
