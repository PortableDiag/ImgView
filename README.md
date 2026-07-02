# ImgView (Rust)

A lightweight, Picasa-style image viewer: one big image with a thumbnail strip
along the bottom, scroll-to-zoom, drag-to-pan, arrow-key navigation, and 90°
rotate. Built with `egui`/`eframe` and the `image` crate for broad format
support (PNG, JPEG, GIF, WebP, BMP, TIFF, ICO, PNM, TGA, DDS, HDR, AVIF, …).

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
| F                   | Fit to window                    |
| 1                   | Actual size (100%)               |
| F11                 | Fullscreen                       |
| Esc                 | Exit fullscreen / quit           |

Drag & drop an image or folder onto the window to open it.

## Layout

```
src/main.rs   entire app: folder loading, background thumbnails,
              zoom/pan transform, rotate + save, toolbar, keyboard.
```
