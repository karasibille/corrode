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

- `drag`: a flash on a slow shutter. The picture stays sharp and a trail of light follows it, sideways (`motion`), from the centre (`zoom`) or round it (`arc`, with `center=x,y` to move the centre).
- `aberration`: the lens does not focus all colours at the same place, so red and blue edges grow towards the corners.
- `grain`: the speckle of fast film.
- `fade`: the look of an old print, with the blacks lifted, the colours washed and a warm tint.
- `vignette`: the corners get darker.
- `leak`: a coloured glow comes in from an edge, as if light had reached the film. By default it is a point on an edge chosen by the seed; `edge=left` (or `right`, `top`, `bottom`) lights a whole side. `spread=35` lets the hue wander round the colour and `wobble=70` makes the glow uneven in reach and strength, so that it is not the same everywhere.
- `streaks`: bands of light across the whole height, as film in a camera that was not shut tight: soft on one side, a hard edge on the other, red at the dim border and yellower towards the core. `count`, `width`, `color`, `spread` (how far the hue goes) and `wobble` (how much they wander from top to bottom) and `softness` (how soft the edge is where the light stops) shape them, and `position=0` puts the first one on the left edge (100 on the right); the seed draws where they are. `leak edge=right color=ffffff size=14` adds the blown white edge.
- `prism`: light split into the colours of a spectrum, as through a prism or a lens flare: a band whose hue changes across it, in straight bands (`kind=linear`, along `angle`) or in rings round a point (`kind=radial`, `center`). `position` and `size` place and widen it, `hue` and `range` choose the colours it goes through, `white` turns its core towards white and `wobble` makes it bend and fade unevenly.
- `bloom`: the brightest lights glow, with a halo around them.

**Distortion**: the picture is moved, not recoloured.

- `liquid`: waves run through the picture, as if seen through moving water.
- `spin`: the picture is smeared round a point, as if the camera had turned during the shot; with a few steps, it leaves separate ghosts.

**Colour**: the colours moved on purpose.

- `falsecolor`: the colours are swapped or turned round the colour wheel (red and blue swapped is the look of infrared film). It can make the colour stronger, and be limited to one range of colours so that the rest stays as it was.
- `remap`: several colours each sent their own way at once, such as the sky towards violet and the grass towards red, without one change feeding another.
- `onepoint` (one point colour): one colour is kept and everything else goes grey, as a yellow that stays vivid in a black and white picture.

## Ghosts: the frames of a burst in one picture

> **Experimental.** `corrode ghost` is still being tuned: its options, their defaults and the way the frames are mixed may change.

`corrode ghost` is not a recipe effect: it takes a few frames of the same scene taken one after another, three or four are enough, cuts each to a window round the subject and lays them over one another. The subject stays in the same place and each pose leaves a ghost round it, a dancer with many arms and legs.

```sh
corrode ghost burst1.RW2 burst2.RW2 burst3.RW2 burst4.RW2 -o ghosts.jpg --centre 50,44 --zoom 2.2
```

- `--blend lighten` (the default) keeps the brighter of two pixels, so the poses stay sharp and stack: it suits a bright subject on a dark stage. `screen` adds the lights, `average` gives every pose the same faint weight, and `over` mixes the frames like see-through layers, each ghost weighing the `--opacity` given when the main frame weighs 1 (1 gives all the frames the same weight, 0.5 shows the ghosts at half), so that no light is added and the poses show through one another.
- `--opacity 0.45` makes the ghosts transparent; the main frame is always whole, the middle one unless `--main N` says which (counting from 1).
- `--zoom 2.2` cuts the result to the frame divided by 2.2, round the subject.
- The subject is the point the camera focused on when every file has one (`--align af`), else the point `--centre x,y`, in percent of the frame, in every frame (`--align fixed`): it is for a camera that does not move.
- `--align track` finds the subject in the frames themselves, for a subject that crosses the frame while the camera stays still (at least three frames): what changes against the middle value of each pixel over all the frames is the subject, and its centre is where the window goes. It does not suit a subject that stays in place, a dancer turning on the spot for example: what does not move is part of that middle value, so the window would follow the limbs and not the body. Use `fixed` or `af` then.
- `--pin-background` keeps the background of the main frame: of the other frames, only what changes against the middle value of each pixel, the subject, is laid over it, so that a lamp or the floor is not repeated where the windows moved. It goes well with `--align track`, which moves the windows. It needs at least three frames.
- `--register` lines the background of the frames up on the main one first, measured on the lights, the lamp above all, because the camera moves a little between two shots and a lamp is then seen once for each frame. The shift found is printed for each frame. It is the option to use on a burst shot by hand; it works with the others.
- It uses the preview embedded in each file, smaller and better looking than a development of the RAW (the preview of a DNG made by `corrode deband` is 1024 pixels long, so give `--zoom` a value that keeps the result large enough); `--full` uses the full-size picture, and `--width` reduces the result.

