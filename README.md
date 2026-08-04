# ImgView (Rust)

A lightweight, Picasa-style image viewer: one big image with a thumbnail strip
along the bottom, scroll-to-zoom, drag-to-pan, arrow-key navigation, and 90°
rotate. Built with `egui`/`eframe` and the `image` crate for broad format
support — PNG, JPEG, GIF, WebP, BMP, TIFF, ICO, PNM, TGA and QOI are all
verified to decode; see [Formats](#formats) for the full picture.

Animated **GIF** and **WebP** play back at their real frame delays; all other
formats show as stills. (Rotating an animation is view-only — save-to-disk is
limited to single-frame images.)

Compiles to a **single native binary** — copy it to any Linux box (with the
usual GL/X11/Wayland libraries present) and run it. No runtime, no interpreter.

## Build & run

```bash
./run.sh                 # dev run (opens a folder chooser if no arg)
./run.sh ~/Pictures      # open a folder
./run.sh photo.jpg       # open a file (loads its whole folder)

./build.sh               # -> ./imgview   (optimized single binary)
```

> Build output is kept in `$HOME/.cache/imgview-rs-target` on purpose:
> `/media/veracrypt1` is exfat, which lacks the symlinks/exec bits cargo wants.

## Install as a system image viewer ("Open With" everywhere)

```bash
./install.sh             # app menu + "Open With" for images (builds if needed)
./install.sh --default   # also make it the default image viewer
./install.sh --uninstall
```

## Controls

| Key / action        | Function                         |
|---------------------|----------------------------------|
| Right / Space       | Next image                       |
| Left                | Previous image                   |
| Mouse wheel         | Zoom in/out (at cursor)          |
| Drag                | Pan                              |
| Double-click        | Toggle fit ↔ 100%                |
| R  /  ]             | Rotate right 90°                 |
| Shift+R  /  [       | Rotate left 90°                  |
| Ctrl+S              | Save rotation back to the file   |
| Ctrl+O              | Open a folder                    |
| Ctrl+C              | Copy the current filename        |
| F                   | Fit to window                    |
| 1                   | Actual size (100%)               |
| + / -               | GIF playback faster / slower     |
| 0                   | Reset playback speed to normal   |
| F11                 | Fullscreen                       |
| Esc                 | Exit fullscreen / quit           |

Every toolbar button carries a tooltip naming its shortcut, and the status text
is selectable so you can drag-select part of a path.

Animations keep only the current frame on the GPU, so even large multi-hundred-
frame GIFs play without exhausting video memory.

Each thumbnail is captioned with its filename, middle-truncated so the extension
stays visible; hover for the full name.

Drag & drop an image or folder onto the window to open it.

## Formats

Decoding is whatever the `image` crate provides with default features. The table
below was measured against a real build by round-tripping a sample through
`image::open` — the same call the viewer makes — rather than assumed from the
crate's feature list.

| Format | Extensions | Status |
|---|---|---|
| PNG, JPEG, GIF, BMP, WebP, TIFF, PNM, TGA, ICO, QOI | `png` `jpg` `jpeg` `gif` `bmp` `webp` `tif` `tiff` `ppm` `pgm` `pbm` `pnm` `tga` `ico` `qoi` | ✅ verified |
| DDS, HDR, OpenEXR, Farbfeld | `dds` `hdr` `exr` `farbfeld` | ❔ untested — no sample could be generated |
| AVIF | `avif` | ❌ **does not decode** |

**AVIF does not work in this build.** The `image` crate ships its AVIF *encoder*
(`ravif`) under default features but no decoder, so every `.avif` fails to open.
Decoding needs the `avif-native` feature, which links system `libdav1d` and would
end this project's "copy the single binary anywhere" property — so it is
deliberately not enabled. `.avif` files still appear in the strip and will report
`Failed to open`.

You can check any file against your own build without launching the GUI:

```bash
./imgview --probe FILE     # frames: 0  means this build cannot decode it
```

## Known limitations

- **Scroll-to-zoom aborts on an extremely small image.** When an image fits the
  window more than ~200× over (roughly 5×3 px or smaller at a 1100×800 window;
  larger when fullscreen), the zoom clamp's bounds cross and the process panics.
  Avoid scroll-zooming single-pixel images until this is fixed.
- **A file that fails to decode shows a spinner in the thumbnail strip forever**,
  because a failed decode is indistinguishable from one still in progress.
- **Animated images hold every decoded frame in RAM** while displayed. Only VRAM
  is bounded (one frame). A 48-frame 800×800 GIF costs ~117 MiB of RAM; there is
  no cap, so a very long high-resolution animation can use a lot of memory.
- **Saving a rotation is limited to single-frame images.** Rotating an animation
  is view-only.

## Layout

```
src/main.rs   entire app: folder loading, background thumbnails,
              zoom/pan transform, rotate + save, toolbar, keyboard.
```
