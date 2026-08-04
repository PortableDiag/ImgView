# Changelog

All notable changes to ImgView. Format loosely follows
[Keep a Changelog](https://keepachangelog.com/); versions are the app version in
`Cargo.toml`.

## [0.2.0] — unreleased

Not tagged: the scroll-zoom panic under **Known issues** should be fixed before
a release is cut.

### Added
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

### Known issues
- **Scroll-to-zoom panics on an extremely small image.** The zoom clamp in
  `src/main.rs` mixes an image-relative lower bound (`fit_scale * 0.2`) with an
  absolute upper bound (`40.0`); when an image fits the window more than ~200×
  over, the lower bound overtakes the upper one and `f32::clamp` asserts,
  aborting the process. Reproduced at 1×1 and 4×3 in a 1100×800 window; 6×4 and
  larger are unaffected. The threshold scales with the viewport, so a fullscreen
  4K window widens it to roughly 19×10 px.
- **AVIF is listed in `IMAGE_EXTS` but cannot be decoded.** Default `image`
  features include the `ravif` *encoder* and no decoder. Documented in the
  README; whether to enable `avif-native` (which links system `libdav1d` and
  ends the single-binary property) or drop the extension is undecided.
- **A file that fails to decode keeps a spinner in the thumbnail strip forever**,
  since a failed decode sends nothing and is indistinguishable from one still
  running.
- **Animated images hold every decoded frame in RAM** for as long as they are
  displayed, uncapped. Measured: a 48-frame 800×800 GIF costs ~117 MiB from a
  107 KB file.

## [0.1.0] — 2026-07-01

Initial three commits, never tagged or released.

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
