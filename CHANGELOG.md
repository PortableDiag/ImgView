# Changelog

All notable changes to ImgView. Format loosely follows
[Keep a Changelog](https://keepachangelog.com/); versions are the app version in
`Cargo.toml`.

## [0.2.0] — 2026-08-16

First release since `v0.1.0`. Three crashes and one silent data-loss bug are
fixed; every advertised format is now verified against a real sample.

### Fixed
- **Scroll-to-zoom no longer aborts the process on a small image.** The zoom
  clamp mixed an image-relative lower bound (`fit_scale * 0.2`) with an absolute
  upper bound (`40.0`); once an image fitted the window more than 200× over the
  bounds crossed and `f32::clamp` panicked, killing the app with exit 101.
  Reproduced at 1×1 and 4×3 in a 1100×800 window — and the threshold scaled with
  the viewport, reaching roughly 19×10 px fullscreen on 4K. Bounds are now
  reconciled in `zoom_bounds()`, and the ceiling rises to the fit scale so a tiny
  image can still be magnified enough to fill the window.
- **Opening an image larger than the GPU's maximum texture size no longer
  crashes.** `egui` panics outright above that limit, which can be as low as
  2048 px per side — i.e. any ordinary photo. Verified: a 4000×3000 PNG killed
  the previous build on open and now displays. The GPU gets a downscaled copy;
  `frames` keeps the original, so rotate-and-save still writes full resolution.
- **Ctrl+S could overwrite an unrelated file.** After a failed decode the old
  image stayed loaded while `index` had already moved on, so "save rotation"
  wrote the *previous* picture over the file that failed to open. Reproduced by
  driving the real window: a 23-byte file became a 6,661-byte PNG of the
  previous image. A failed decode now clears the view and reports itself.
- **A file that fails to decode no longer spins forever** in the thumbnail
  strip. Failures are reported explicitly and render as a ⚠ marker, and the
  central panel says which file failed instead of showing the cold-start
  "Open a folder or drop an image" invitation with a folder already open.
- **`.farbfeld` files were offered but could never open, and `.ff` files were
  hidden.** The `image` crate recognises farbfeld only as `.ff`. `IMAGE_EXTS`
  now lists `ff`.
- **`avif` removed from `IMAGE_EXTS`.** There is no AVIF decoder in this build,
  so listing the extension only produced files that were guaranteed to fail.
- **Ctrl+C no longer clobbers a text selection.** Copying part of the status
  path with the mouse and pressing Ctrl+C replaced it with the filename; the
  filename is now only copied when nothing else claimed the clipboard.
- **Saving a rotated JPEG re-encodes at quality 95** instead of the crate's
  default 75, and the status bar says the file was re-encoded.
- Zero-sized panels can no longer produce a non-finite fit scale.

### Added
- Unit tests (`cargo test`) covering the zoom-clamp regression, the extension
  list, JPEG detection, filename ellipsis and thumbnail fitting.
- **Filename captions under every thumbnail.** Names are middle-truncated so the
  extension stays visible (`middle_ellipsis`), with the full name on hover. The
  strip is 20 px taller to make room, and each thumbnail now sits in a
  fixed-width centred cell so the captions line up.
- **`Ctrl+O`** — open a folder from the keyboard.
- **`Ctrl+C` / "📋 Copy name"** — copy the current filename to the clipboard.
- **Tooltips on every toolbar button**, each naming its keyboard shortcut.
- **`Formats` and `Known limitations` sections in the README**, backed by a
  measured decode matrix rather than the crate's advertised feature list.
- This changelog.

### Changed
- The status text is now a selectable label, so part of a path can be
  drag-selected and copied.

### Changed (formats)
- DDS, Radiance HDR and OpenEXR moved from **untested** to **verified** — real
  samples were generated and probed rather than inferred from the crate's
  feature list. Every extension in `IMAGE_EXTS` is now individually confirmed to
  decode under the exact extension the viewer offers.

### Known issues
- **Animated images hold every decoded frame in RAM** for as long as they are
  displayed, uncapped. Measured: a 48-frame 800×800 GIF costs ~117 MiB from a
  107 KB file. Capping this needs a memory budget to be chosen.
- **Images above the GPU texture limit are shown downscaled.** Displaying them
  at true resolution would need tiled textures.
- **Thumbnails are decoded at full resolution** before scaling, so a folder of
  very large photos is slow to fill the strip.

## [0.1.0] — 2026-07-01

Initial three commits. Tagged `v0.1.0` and published as a GitHub release on
2026-07-02 with an `imgview-linux-x86_64` binary attached.

### Added
- Picasa-style viewer: one large image, background-loaded thumbnail strip,
  scroll-to-zoom at the cursor, drag-to-pan, arrow-key navigation, double-click
  to toggle fit ↔ 100%, fullscreen, drag & drop.
- 90° rotation, lossless in view, with `Ctrl+S` to write it back to disk
  (single-frame images only).
- **Animated GIF and WebP playback** at real per-frame delays, with delays under
  20 ms rewritten to 100 ms the way browsers do.
- **Playback-speed controls** (`+` / `-` / `0`) and a bounded-memory playback
  path: every frame stays decoded in RAM but only the *current* frame is
  uploaded to the GPU, so long animations no longer exhaust video memory.
- Animation timing driven by wall-clock `input.time` rather than egui's
  `stable_dt`, which is a smoothed ~16 ms prediction and made animations run
  several times too slow in an app that repaints only when a frame is due.
- `--probe FILE` diagnostic that prints decoded frame count, delays and
  dimensions without opening a window.
- `build.sh` / `run.sh` / `install.sh`, with `CARGO_TARGET_DIR` redirected to
  `$HOME` because the source lives on exfat (no symlinks or exec bits).
