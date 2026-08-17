//! ImgView — a lightweight, Picasa-style image viewer.
//!
//! One big image up top with scroll-to-zoom / drag-to-pan, a thumbnail strip
//! along the bottom, arrow-key navigation, and 90° rotate (with save-to-disk).

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::fs::File;
use std::io::{BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};
use std::time::Duration;

use eframe::egui;
use egui::{Align, Color32, Key, Rect, Sense, Stroke, TextureHandle, TextureOptions, Vec2};
use image::codecs::gif::GifDecoder;
use image::codecs::jpeg::JpegEncoder;
use image::codecs::webp::WebPDecoder;
use image::{AnimationDecoder, DynamicImage};

const THUMB: u32 = 96;
/// Extensions we offer to open. Every one of these was probed against a real
/// sample and decodes; two used to be listed that never could:
///  * `avif` — the `image` crate's default features ship the AVIF *encoder*
///    only, so every `.avif` failed to open.
///  * `farbfeld` — the crate recognises farbfeld as `.ff`, and only as `.ff`,
///    so `.farbfeld` files were offered and `.ff` files were hidden.
const IMAGE_EXTS: &[&str] = &[
    "jpg", "jpeg", "png", "gif", "bmp", "webp", "tif", "tiff", "ppm", "pgm",
    "pbm", "pnm", "tga", "ico", "dds", "hdr", "exr", "qoi", "ff",
];

/// Hard ceiling on zoom for a normally-sized image. Small images may exceed it
/// — see [`zoom_bounds`].
const MAX_ZOOM: f32 = 40.0;
/// Bounds on the fit-to-window scale, so it is always finite and divisible by.
const MIN_SCALE: f32 = 1e-6;
const MAX_SCALE: f32 = 1e6;

/// Lower and upper bounds for the zoom clamp, given the fit-to-window scale.
///
/// The two bounds are computed on different bases — the floor is relative to
/// the image, the ceiling is absolute — so they must be reconciled explicitly:
/// `f32::clamp` panics if `min > max`, which used to abort the process when an
/// image fitted the window more than `MAX_ZOOM / 0.2` times over (about 5x3 px
/// in a default window, 19x10 fullscreen on 4K).
///
/// The ceiling also rises to `fit` for such images: a 1x1 needs several hundred
/// times magnification just to fill the window, so a flat 40x cap would leave it
/// unzoomable.
fn zoom_bounds(fit: f32) -> (f32, f32) {
    let hi = MAX_ZOOM.max(fit);
    let lo = (fit * 0.2).min(hi);
    (lo, hi)
}

fn is_image(path: &Path) -> bool {
    IMAGE_EXTS.contains(&ext_of(path).as_str())
}

/// Sorted list of image files in a directory (case-insensitive by name).
fn list_images(dir: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file() && is_image(p))
        .collect();
    out.sort_by_key(|p| p.file_name().map(|n| n.to_ascii_lowercase()));
    out
}

/// Convert a decoded image to an egui-ready CPU image.
fn to_color_image(img: &DynamicImage) -> egui::ColorImage {
    let rgba = img.to_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw())
}

/// One frame of an image: pixels plus how long to show it (zero for static).
struct AnimFrame {
    image: DynamicImage,
    delay: Duration,
}

/// What a decode produced: the frames, plus anything the user should be told
/// about how they were produced (currently only the animation memory cap).
struct Loaded {
    frames: Vec<AnimFrame>,
    note: Option<String>,
}

impl Loaded {
    fn frames(frames: Vec<AnimFrame>) -> Self {
        Self { frames, note: None }
    }
    fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }
}

/// Default ceiling on the RAM an animation may occupy while displayed, in MiB.
/// Override with `IMGVIEW_ANIM_BUDGET_MB` — which is also how the cap is tested
/// without generating a gigabyte of GIF.
const ANIM_RAM_BUDGET_MB: u64 = 1024;

fn anim_budget_bytes() -> u64 {
    std::env::var("IMGVIEW_ANIM_BUDGET_MB")
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .unwrap_or(ANIM_RAM_BUDGET_MB)
        .saturating_mul(1024 * 1024)
}

/// Collect an animation decoder's frames, clamping delays the way browsers do
/// and stopping if they would exceed the memory budget.
///
/// Frames are pulled **one at a time on purpose**: `collect_frames()` decodes the
/// whole animation before it returns anything, so a 4 GB GIF would already be in
/// RAM by the time there was a total to check. Iterating means the budget is a
/// real ceiling — we overshoot by at most the one frame that crosses it.
fn collect_anim(frames: image::Frames) -> Loaded {
    let budget = anim_budget_bytes();
    let mut out: Vec<AnimFrame> = Vec::new();
    let mut bytes: u64 = 0;

    for frame in frames {
        let Ok(frame) = frame else { break }; // keep whatever decoded cleanly
        let mut delay: Duration = frame.delay().into();
        if delay < Duration::from_millis(20) {
            delay = Duration::from_millis(100);
        }
        let image = DynamicImage::ImageRgba8(frame.into_buffer());
        bytes += u64::from(image.width()) * u64::from(image.height()) * 4;
        out.push(AnimFrame { image, delay });

        if bytes > budget {
            // Show the first frame as a still rather than animate a truncated
            // clip — a silently shortened animation is worse than none.
            let stopped = out.len();
            out.truncate(1);
            out[0].delay = Duration::ZERO;
            return Loaded {
                frames: out,
                note: Some(format!(
                    "animation over the {} MiB cap (stopped at {stopped} frame{}) — first frame only",
                    budget / (1024 * 1024),
                    if stopped == 1 { "" } else { "s" }
                )),
            };
        }
    }
    Loaded::frames(out)
}

/// Decode a file into frames. Static images yield a single frame; animated
/// GIF/WebP yield every frame with per-frame delays.
fn load_frames(path: &Path) -> Loaded {
    let ext = ext_of(path);

    if ext == "gif" {
        if let Ok(file) = File::open(path) {
            if let Ok(dec) = GifDecoder::new(BufReader::new(file)) {
                let v = collect_anim(dec.into_frames());
                if !v.is_empty() {
                    return v;
                }
            }
        }
    } else if ext == "webp" {
        if let Ok(file) = File::open(path) {
            if let Ok(dec) = WebPDecoder::new(BufReader::new(file)) {
                if dec.has_animation() {
                    let v = collect_anim(dec.into_frames());
                    if !v.is_empty() {
                        return v;
                    }
                }
            }
        }
    }

    // Static fallback: PNG/JPEG/BMP/TIFF/… and non-animated gif/webp.
    match image::open(path) {
        Ok(img) => Loaded::frames(vec![AnimFrame {
            image: img,
            delay: Duration::ZERO,
        }]),
        Err(_) => Loaded::frames(Vec::new()),
    }
}

/// A finished thumbnail decode, shipped from the loader thread to the UI.
/// `image` is `None` when the file could not be decoded — a failure has to be
/// reported explicitly, or "failed" is indistinguishable from "still running"
/// and the strip shows a spinner forever.
struct ThumbMsg {
    index: usize,
    image: Option<egui::ColorImage>,
}

/// What the strip knows about one thumbnail.
#[derive(Clone)]
enum ThumbState {
    Loading,
    Ready(TextureHandle),
    Failed,
}

struct ImgView {
    paths: Vec<PathBuf>,
    index: usize,

    // Current image as frames (one for static images, many for animations).
    // Frames stay decoded in RAM; only the *current* frame lives on the GPU,
    // so a huge multi-hundred-frame GIF doesn't blow out VRAM.
    frames: Vec<AnimFrame>,      // unrotated sources (rotation stays lossless)
    texture: Option<TextureHandle>, // the single frame currently on the GPU
    cur_frame: usize,
    frame_elapsed: f32, // seconds the current frame has been shown
    last_time: f64,     // wall-clock time (input.time) at the last update
    speed: f32,         // animation playback speed multiplier (1.0 = normal)
    angle: i32,         // 0 / 90 / 180 / 270, clockwise

    // View transform.
    scale: f32,     // screen pixels per image pixel
    offset: Vec2,   // image top-left relative to the panel's top-left
    fitting: bool,  // true => keep fitted to the window
    need_layout: bool,

    // Thumbnail strip.
    thumbs: Vec<ThumbState>,
    thumb_rx: Option<Receiver<ThumbMsg>>,
    scroll_to_selected: bool,

    status: String,
}

impl ImgView {
    fn new(cc: &eframe::CreationContext<'_>, initial: Option<String>) -> Self {
        // Nicer default look.
        cc.egui_ctx.set_visuals(egui::Visuals::dark());
        let mut app = Self {
            paths: Vec::new(),
            index: 0,
            frames: Vec::new(),
            texture: None,
            cur_frame: 0,
            frame_elapsed: 0.0,
            last_time: 0.0,
            speed: 1.0,
            angle: 0,
            scale: 1.0,
            offset: Vec2::ZERO,
            fitting: true,
            need_layout: true,
            thumbs: Vec::new(),
            thumb_rx: None,
            scroll_to_selected: false,
            status: String::new(),
        };
        if let Some(arg) = initial {
            app.open_path(&cc.egui_ctx, Path::new(&arg));
        }
        app
    }

    // ---- loading ---------------------------------------------------------
    fn open_path(&mut self, ctx: &egui::Context, path: &Path) {
        if path.is_dir() {
            self.open_folder(ctx, path, None);
        } else if path.is_file() {
            let dir = path.parent().unwrap_or(Path::new(".")).to_path_buf();
            self.open_folder(ctx, &dir, Some(path.to_path_buf()));
        }
    }

    fn open_folder(&mut self, ctx: &egui::Context, dir: &Path, select: Option<PathBuf>) {
        self.paths = list_images(dir);
        self.thumbs = vec![ThumbState::Loading; self.paths.len()];
        self.spawn_thumb_loader();

        let start = select
            .and_then(|sel| {
                let sel = std::fs::canonicalize(&sel).unwrap_or(sel);
                self.paths.iter().position(|p| {
                    std::fs::canonicalize(p).map(|c| c == sel).unwrap_or(false)
                })
            })
            .unwrap_or(0);

        if self.paths.is_empty() {
            self.frames.clear();
            self.texture = None;
            self.status = format!("No images in {}", dir.display());
        } else {
            self.show_index(ctx, start);
        }
    }

    /// Decode thumbnails on a background thread; results arrive via `thumb_rx`.
    fn spawn_thumb_loader(&mut self) {
        let (tx, rx) = channel::<ThumbMsg>();
        self.thumb_rx = Some(rx);
        let paths = self.paths.clone();
        std::thread::spawn(move || {
            for (index, path) in paths.iter().enumerate() {
                let image = image::open(path)
                    .ok()
                    .map(|img| to_color_image(&img.thumbnail(THUMB, THUMB)));
                // If the receiver is gone (folder changed), stop early.
                if tx.send(ThumbMsg { index, image }).is_err() {
                    break;
                }
            }
        });
    }

    fn show_index(&mut self, ctx: &egui::Context, index: usize) {
        if self.paths.is_empty() || index >= self.paths.len() {
            return;
        }
        self.index = index;
        let path = self.paths[index].clone();
        let loaded = load_frames(&path);
        if loaded.is_empty() {
            // Drop the previous image rather than leaving it on screen: it no
            // longer matches `self.index`, and Ctrl+S would then write the old
            // picture over *this* file.
            self.frames.clear();
            self.texture = None;
            self.angle = 0;
            self.scroll_to_selected = true;
            self.status = format!("Failed to open {}", path.display());
            return;
        }
        self.frames = loaded.frames;
        self.cur_frame = 0;
        self.frame_elapsed = 0.0;
        self.angle = 0;
        self.upload_current(ctx);
        self.need_layout = true;
        self.fitting = true;
        self.scroll_to_selected = true;
        let anim = if self.frames.len() > 1 {
            format!("  ·  animated, {} frames", self.frames.len())
        } else {
            String::new()
        };
        // A capped animation has to say so — otherwise it just looks broken.
        let note = loaded
            .note
            .map(|n| format!("  ·  ⚠ {n}"))
            .unwrap_or_default();
        self.status = format!(
            "{}  [{}/{}]{}{}",
            path.file_name().unwrap_or_default().to_string_lossy(),
            index + 1,
            self.paths.len(),
            anim,
            note
        );
    }

    /// The bare filename of the currently shown image (empty if none).
    fn cur_name(&self) -> String {
        self.paths
            .get(self.index)
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    fn next(&mut self, ctx: &egui::Context) {
        if !self.paths.is_empty() {
            let i = (self.index + 1) % self.paths.len();
            self.show_index(ctx, i);
        }
    }

    fn prev(&mut self, ctx: &egui::Context) {
        if !self.paths.is_empty() {
            let i = (self.index + self.paths.len() - 1) % self.paths.len();
            self.show_index(ctx, i);
        }
    }

    // ---- rotation --------------------------------------------------------
    fn rotate_image(img: &DynamicImage, angle: i32) -> DynamicImage {
        match angle.rem_euclid(360) {
            90 => img.rotate90(),
            180 => img.rotate180(),
            270 => img.rotate270(),
            _ => img.clone(),
        }
    }

    /// Upload only the current frame (rotated) to the single GPU texture,
    /// reusing the existing texture slot when possible. Keeps VRAM at one
    /// frame regardless of how many frames a GIF has.
    fn upload_current(&mut self, ctx: &egui::Context) {
        if self.frames.is_empty() {
            return;
        }
        let idx = self.cur_frame.min(self.frames.len() - 1);
        let img = Self::rotate_image(&self.frames[idx].image, self.angle);
        // A texture larger than the GL limit is a hard panic inside egui, and
        // that limit can be as low as 2048 — i.e. any ordinary photo. Send the
        // GPU a downscaled copy; `frames` keeps the original, so rotate-and-save
        // still writes full resolution.
        let max = ctx.input(|i| i.max_texture_side).max(1) as u32;
        let img = if img.width() > max || img.height() > max {
            img.resize(max, max, image::imageops::FilterType::Triangle)
        } else {
            img
        };
        let color = to_color_image(&img);
        if let Some(tex) = self.texture.as_mut() {
            tex.set(color, TextureOptions::LINEAR);
        } else {
            self.texture = Some(ctx.load_texture("current", color, TextureOptions::LINEAR));
        }
    }

    fn rotate(&mut self, ctx: &egui::Context, delta: i32) {
        if !self.frames.is_empty() {
            self.angle = (self.angle + delta).rem_euclid(360);
            self.fitting = true;
            self.need_layout = true;
            self.upload_current(ctx);
        }
    }

    fn save_rotation(&mut self, ctx: &egui::Context) {
        if self.angle % 360 == 0 {
            self.status = "Nothing to save (image not rotated)".into();
            return;
        }
        if self.frames.len() != 1 {
            self.status = "Save rotation isn't supported for animated images".into();
            return;
        }
        let rotated = Self::rotate_image(&self.frames[0].image, self.angle);
        let path = self.paths[self.index].clone();
        match save_image(&rotated, &path) {
            Ok(()) => {
                // Bake rotation into our source and refresh this thumbnail.
                let thumb = to_color_image(&rotated.thumbnail(THUMB, THUMB));
                self.thumbs[self.index] =
                    ThumbState::Ready(ctx.load_texture("thumb", thumb, TextureOptions::LINEAR));
                self.frames[0] = AnimFrame {
                    image: rotated,
                    delay: Duration::ZERO,
                };
                self.angle = 0;
                self.upload_current(ctx);
                let note = if is_jpeg(&path) {
                    "  (JPEG re-encoded, q95)"
                } else {
                    ""
                };
                self.status = format!(
                    "Saved rotation → {}{note}",
                    path.file_name().unwrap_or_default().to_string_lossy()
                );
            }
            Err(e) => self.status = format!("Save failed: {e}"),
        }
    }

    // ---- view transform --------------------------------------------------
    fn layout(&mut self, rect: Rect, img: Vec2) {
        // Fit-to-window, centered.
        self.scale = self.layout_fit_scale(rect, img);
        self.offset = (rect.size() - img * self.scale) * 0.5;
    }

    fn center_at(&mut self, rect: Rect, img: Vec2, scale: f32) {
        self.scale = scale;
        self.offset = (rect.size() - img * scale) * 0.5;
    }
}

impl eframe::App for ImgView {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Drain a few finished thumbnails per frame into GPU textures.
        if let Some(rx) = &self.thumb_rx {
            let mut pending = false;
            for _ in 0..8 {
                match rx.try_recv() {
                    Ok(msg) => {
                        if msg.index < self.thumbs.len() {
                            self.thumbs[msg.index] = match msg.image {
                                Some(image) => ThumbState::Ready(ctx.load_texture(
                                    "thumb",
                                    image,
                                    TextureOptions::LINEAR,
                                )),
                                None => ThumbState::Failed,
                            };
                        }
                        pending = true;
                    }
                    Err(_) => break,
                }
            }
            if pending {
                ctx.request_repaint(); // keep pulling until the queue drains
            }
        }

        // ---- animation playback ----
        // Use real wall-clock time (input.time), NOT stable_dt: stable_dt is a
        // smoothed *predicted* frame interval (~16ms), but we only repaint once
        // per frame-delay, so it would make animations run several times slow.
        let now = ctx.input(|i| i.time);
        let dt = ((now - self.last_time) as f32).clamp(0.0, 0.25);
        self.last_time = now;
        if self.frames.len() > 1 {
            let start = self.cur_frame;
            self.frame_elapsed += dt * self.speed.max(0.05);
            let mut delay = self.frames[self.cur_frame].delay.as_secs_f32().max(0.02);
            while self.frame_elapsed >= delay {
                self.frame_elapsed -= delay;
                self.cur_frame = (self.cur_frame + 1) % self.frames.len();
                delay = self.frames[self.cur_frame].delay.as_secs_f32().max(0.02);
            }
            if self.cur_frame != start {
                self.upload_current(ctx); // swap the one GPU texture to this frame
            }
            // Wake up when the current frame should flip (scaled by speed).
            let remaining = (delay - self.frame_elapsed).max(0.0) / self.speed.max(0.05);
            ctx.request_repaint_after(Duration::from_secs_f32(remaining));
        }

        // ---- keyboard ----
        let (mut go_next, mut go_prev) = (false, false);
        let (mut rot_cw, mut rot_ccw, mut do_save) = (false, false, false);
        let (mut do_fit, mut do_actual, mut do_full, mut do_esc) =
            (false, false, false, false);
        let (mut faster, mut slower, mut reset_speed) = (false, false, false);
        let (mut do_open, mut do_copy) = (false, false);
        ctx.input(|i| {
            let shift = i.modifiers.shift;
            let ctrl = i.modifiers.ctrl || i.modifiers.command;
            if ctrl && i.key_pressed(Key::O) {
                do_open = true;
            }
            if ctrl && i.key_pressed(Key::C) {
                do_copy = true;
            }
            if i.key_pressed(Key::ArrowRight) || i.key_pressed(Key::Space) {
                go_next = true;
            }
            if i.key_pressed(Key::ArrowLeft) {
                go_prev = true;
            }
            if (i.key_pressed(Key::R) && !shift) || i.key_pressed(Key::CloseBracket) {
                rot_cw = true;
            }
            if (i.key_pressed(Key::R) && shift) || i.key_pressed(Key::OpenBracket) {
                rot_ccw = true;
            }
            if ctrl && i.key_pressed(Key::S) {
                do_save = true;
            }
            if i.key_pressed(Key::F) {
                do_fit = true;
            }
            if i.key_pressed(Key::Num1) {
                do_actual = true;
            }
            if i.key_pressed(Key::F11) {
                do_full = true;
            }
            if i.key_pressed(Key::Escape) {
                do_esc = true;
            }
            if i.key_pressed(Key::Plus) || i.key_pressed(Key::Equals) {
                faster = true;
            }
            if i.key_pressed(Key::Minus) {
                slower = true;
            }
            if i.key_pressed(Key::Num0) {
                reset_speed = true;
            }
        });

        // ---- top toolbar ----
        egui::TopBottomPanel::top("toolbar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                if ui
                    .button("📂 Open Folder")
                    .on_hover_text("Open a folder  (Ctrl+O)")
                    .clicked()
                {
                    do_open = true;
                }
                ui.separator();
                if ui.button("◀ Prev").on_hover_text("Previous  (←)").clicked() {
                    go_prev = true;
                }
                if ui
                    .button("Next ▶")
                    .on_hover_text("Next  (→ or Space)")
                    .clicked()
                {
                    go_next = true;
                }
                ui.separator();
                if ui
                    .button("⟲ Rotate L")
                    .on_hover_text("Rotate left  (Shift+R or [)")
                    .clicked()
                {
                    rot_ccw = true;
                }
                if ui
                    .button("⟳ Rotate R")
                    .on_hover_text("Rotate right  (R or ])")
                    .clicked()
                {
                    rot_cw = true;
                }
                if ui
                    .button("💾 Save")
                    .on_hover_text("Save rotation to disk  (Ctrl+S)")
                    .clicked()
                {
                    do_save = true;
                }
                // Playback-speed controls, only relevant for animations.
                if self.frames.len() > 1 {
                    ui.separator();
                    if ui.button("🐢 Slower").on_hover_text("Slower  (-)").clicked() {
                        slower = true;
                    }
                    if ui.button("🐇 Faster").on_hover_text("Faster  (+)").clicked() {
                        faster = true;
                    }
                    if ui.button("Reset").on_hover_text("Reset speed  (0)").clicked() {
                        reset_speed = true;
                    }
                    ui.label(format!("{:.2}×", self.speed));
                }
                ui.separator();
                // Copy the current filename to the clipboard.
                if !self.paths.is_empty()
                    && ui
                        .button("📋 Copy name")
                        .on_hover_text("Copy the filename to the clipboard  (Ctrl+C)")
                        .clicked()
                {
                    do_copy = true;
                }
                // Selectable so the user can drag-select and copy any part of it.
                ui.add(egui::Label::new(&self.status).selectable(true));
            });
        });

        // ---- bottom thumbnail strip ----
        let mut clicked: Option<usize> = None;
        egui::TopBottomPanel::bottom("thumbs")
            .exact_height((THUMB + 44) as f32)
            .show(ctx, |ui| {
                egui::ScrollArea::horizontal()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.horizontal_top(|ui| {
                            for i in 0..self.paths.len() {
                                let selected = i == self.index;
                                // Each thumbnail lives in a fixed-width, centered
                                // cell so the filename caption lines up under it.
                                let cell = Vec2::new(THUMB as f32 + 8.0, (THUMB + 40) as f32);
                                let resp = ui
                                    .allocate_ui_with_layout(
                                        cell,
                                        egui::Layout::top_down(Align::Center),
                                        |ui| {
                                            let stroke = if selected {
                                                Stroke::new(2.0, Color32::from_rgb(90, 160, 240))
                                            } else {
                                                Stroke::NONE
                                            };
                                            let resp = egui::Frame::none()
                                                .stroke(stroke)
                                                .inner_margin(2.0)
                                                .show(ui, |ui| match &self.thumbs[i] {
                                                    ThumbState::Ready(tex) => {
                                                        let size = fit_within(
                                                            tex.size_vec2(),
                                                            THUMB as f32,
                                                        );
                                                        ui.add(
                                                            egui::Image::new((tex.id(), size))
                                                                .sense(Sense::click()),
                                                        )
                                                    }
                                                    ThumbState::Loading => ui.add_sized(
                                                        [THUMB as f32, THUMB as f32],
                                                        egui::Spinner::new(),
                                                    ),
                                                    // Not a spinner: this file was
                                                    // tried and cannot be decoded.
                                                    ThumbState::Failed => ui
                                                        .add_sized(
                                                            [THUMB as f32, THUMB as f32],
                                                            egui::Label::new(
                                                                egui::RichText::new("⚠")
                                                                    .size(28.0)
                                                                    .color(Color32::DARK_GRAY),
                                                            )
                                                            .sense(Sense::click()),
                                                        )
                                                        .on_hover_text("Cannot decode this file"),
                                                })
                                                .inner;
                                            // Filename caption, middle-truncated to fit.
                                            let name = self.paths[i]
                                                .file_name()
                                                .unwrap_or_default()
                                                .to_string_lossy();
                                            let color = if selected {
                                                Color32::from_rgb(120, 180, 250)
                                            } else {
                                                Color32::GRAY
                                            };
                                            ui.add(
                                                egui::Label::new(
                                                    egui::RichText::new(middle_ellipsis(&name, 16))
                                                        .size(11.0)
                                                        .color(color),
                                                )
                                                .truncate(),
                                            )
                                            .on_hover_text(name.as_ref());
                                            resp
                                        },
                                    )
                                    .inner;
                                if resp.clicked() {
                                    clicked = Some(i);
                                }
                                if selected && self.scroll_to_selected {
                                    resp.scroll_to_me(Some(Align::Center));
                                }
                            }
                        });
                    });
            });
        self.scroll_to_selected = false;

        // ---- central image area ----
        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(Color32::BLACK))
            .show(ctx, |ui| {
                let rect = ui.max_rect();
                if let Some(tex) = self.texture.clone() {
                    let img = tex.size_vec2();
                    if self.need_layout || self.fitting {
                        self.layout(rect, img);
                        self.need_layout = false;
                    }

                    let resp = ui.interact(rect, ui.id().with("canvas"), Sense::click_and_drag());

                    // Zoom at cursor.
                    let scroll = ui.input(|i| i.smooth_scroll_delta.y);
                    if resp.hovered() && scroll != 0.0 {
                        let factor = (scroll / 200.0).exp2();
                        let (lo, hi) = zoom_bounds(self.layout_fit_scale(rect, img));
                        let new_scale = (self.scale * factor).clamp(lo, hi);
                        if let Some(pos) = ui.input(|i| i.pointer.hover_pos()) {
                            let cursor = pos - rect.min.to_vec2();
                            let before = (cursor.to_vec2() - self.offset) / self.scale;
                            self.offset = cursor.to_vec2() - before * new_scale;
                        }
                        self.scale = new_scale;
                        self.fitting = false;
                    }

                    // Pan.
                    if resp.dragged() {
                        self.offset += resp.drag_delta();
                        self.fitting = false;
                    }

                    // Double-click toggles fit <-> 100%.
                    if resp.double_clicked() {
                        if self.fitting {
                            self.center_at(rect, img, 1.0);
                            self.fitting = false;
                        } else {
                            self.fitting = true;
                            self.need_layout = true;
                        }
                    }

                    // Paint (clipped to the panel).
                    let min = rect.min + self.offset;
                    let draw = Rect::from_min_size(min, img * self.scale);
                    let painter = ui.painter_at(rect);
                    painter.image(
                        tex.id(),
                        draw,
                        Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                        Color32::WHITE,
                    );
                } else {
                    // A folder *is* open and one file simply would not decode:
                    // say so, instead of the cold-start invitation.
                    let msg = if self.paths.is_empty() {
                        "Open a folder or drop an image  (📂 or Ctrl+O)".to_owned()
                    } else {
                        format!("⚠  {}", self.status)
                    };
                    ui.centered_and_justified(|ui| {
                        ui.label(msg);
                    });
                }
            });

        // ---- drag & drop ----
        let dropped = ctx.input(|i| i.raw.dropped_files.clone());
        if let Some(file) = dropped.into_iter().find_map(|f| f.path) {
            self.open_path(ctx, &file);
        }

        // ---- apply actions ----
        if do_open {
            if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                self.open_folder(ctx, &dir, None);
            }
        }
        if do_copy && !self.paths.is_empty() {
            let name = self.cur_name();
            // Only if nothing else claimed the clipboard this frame: Ctrl+C with
            // part of the status label selected must copy the selection, not
            // silently replace it with the filename.
            ctx.output_mut(|o| {
                if o.copied_text.is_empty() {
                    o.copied_text = name;
                }
            });
        }
        if go_next {
            self.next(ctx);
        }
        if go_prev {
            self.prev(ctx);
        }
        if rot_cw {
            self.rotate(ctx, 90);
        }
        if rot_ccw {
            self.rotate(ctx, -90);
        }
        if do_save {
            self.save_rotation(ctx);
        }
        if faster {
            self.speed = (self.speed * 1.5).min(16.0);
        }
        if slower {
            self.speed = (self.speed / 1.5).max(0.1);
        }
        if reset_speed {
            self.speed = 1.0;
        }
        if do_fit {
            self.fitting = true;
            self.need_layout = true;
        }
        if do_actual {
            self.fitting = false;
            self.need_layout = false;
            if let Some(tex) = self.texture.clone() {
                let img = tex.size_vec2();
                let rect = ctx.available_rect();
                self.center_at(rect, img, 1.0);
            }
        }
        if do_full {
            let is_full = ctx.input(|i| i.viewport().fullscreen.unwrap_or(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(!is_full));
        }
        if do_esc {
            let is_full = ctx.input(|i| i.viewport().fullscreen.unwrap_or(false));
            if is_full {
                ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
            } else {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
        if let Some(i) = clicked {
            self.show_index(ctx, i);
        }
    }
}

impl ImgView {
    /// Scale at which `img` fits inside `rect`. Always finite and strictly
    /// positive: callers divide by it and use it as a clamp bound, and a
    /// degenerate (zero-sized) panel would otherwise yield 0, inf or NaN.
    fn layout_fit_scale(&self, rect: Rect, img: Vec2) -> f32 {
        let fit = (rect.width() / img.x).min(rect.height() / img.y);
        if fit.is_finite() {
            fit.clamp(MIN_SCALE, MAX_SCALE)
        } else {
            1.0
        }
    }
}

/// Shorten a string to at most `max` characters, dropping from the middle and
/// inserting an ellipsis so both the start and the file extension stay visible.
fn middle_ellipsis(s: &str, max: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max {
        return s.to_string();
    }
    if max <= 1 {
        return "…".to_string();
    }
    let keep = max - 1; // one slot for the ellipsis
    let front = keep.div_ceil(2);
    let back = keep - front;
    let mut out: String = chars[..front].iter().collect();
    out.push('…');
    out.extend(&chars[chars.len() - back..]);
    out
}

/// Lower-cased extension of `path`, or an empty string.
fn ext_of(path: &Path) -> String {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default()
}

fn is_jpeg(path: &Path) -> bool {
    matches!(ext_of(path).as_str(), "jpg" | "jpeg")
}

/// Write `img` back over `path`, keeping the file's existing format.
///
/// JPEG is special-cased. A rotation cannot be applied losslessly here, so the
/// file must be re-encoded — and `DynamicImage::save` would do that at the
/// crate's default quality (75), visibly degrading a photo a little more on
/// every single save. 95 keeps the damage down to something a photo survives.
fn save_image(img: &DynamicImage, path: &Path) -> image::ImageResult<()> {
    if !is_jpeg(path) {
        return img.save(path);
    }
    let mut out = BufWriter::new(File::create(path)?);
    // The JPEG encoder rejects alpha, so flatten to RGB first.
    JpegEncoder::new_with_quality(&mut out, 95)
        .encode_image(&DynamicImage::ImageRgb8(img.to_rgb8()))?;
    out.flush()?;
    Ok(())
}

/// Scale a size down so its largest side is at most `max` (never up).
fn fit_within(size: Vec2, max: f32) -> Vec2 {
    let f = (max / size.x).min(max / size.y).min(1.0);
    size * f
}

/// A tiny generated app icon: blue sky, sun, green mountains.
fn app_icon() -> egui::IconData {
    let s = 64usize;
    let mut rgba = vec![0u8; s * s * 4];
    let put = |buf: &mut [u8], x: usize, y: usize, c: [u8; 4]| {
        let i = (y * s + x) * 4;
        buf[i..i + 4].copy_from_slice(&c);
    };
    for y in 0..s {
        for x in 0..s {
            // sky
            let mut c = [74, 144, 217, 255];
            // sun
            let (dx, dy) = (x as f32 - 16.0, y as f32 - 16.0);
            if dx * dx + dy * dy < 8.0 * 8.0 {
                c = [255, 211, 78, 255];
            }
            // mountains (two triangles rising from the bottom)
            let fy = s as f32 - 1.0 - y as f32;
            let m1 = (x as f32 - 6.0).abs() * 0.9;
            let m2 = (x as f32 - 40.0).abs() * 1.1;
            if fy < (28.0 - m1) || fy < (34.0 - m2) {
                c = [47, 125, 79, 255];
            }
            put(&mut rgba, x, y, c);
        }
    }
    egui::IconData { rgba, width: s as u32, height: s as u32 }
}

fn main() -> eframe::Result<()> {
    let args: Vec<String> = std::env::args().collect();

    // Hidden diagnostic: `imgview --probe FILE` prints decoded frame info.
    if args.get(1).map(|s| s == "--probe").unwrap_or(false) {
        if let Some(p) = args.get(2) {
            let loaded = load_frames(Path::new(p));
            let frames = loaded.frames;
            println!("frames: {}", frames.len());
            if let Some(note) = loaded.note {
                println!("note: {note}");
            }
            for (i, f) in frames.iter().enumerate() {
                let img = &f.image;
                println!(
                    "  {i}: {}ms  {}x{}",
                    f.delay.as_millis(),
                    image::GenericImageView::width(img),
                    image::GenericImageView::height(img)
                );
            }
        }
        return Ok(());
    }

    let arg = args.get(1).cloned();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1100.0, 800.0])
            .with_title("ImgView")
            .with_icon(app_icon()),
        ..Default::default()
    };
    eframe::run_native(
        "ImgView",
        options,
        Box::new(|cc| Ok(Box::new(ImgView::new(cc, arg)))),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The regression this whole clamp exists for: `f32::clamp` panics when
    /// `min > max`, which aborted the process on a scroll over a tiny image.
    #[test]
    fn zoom_bounds_never_cross() {
        // 638.0 is the fit scale of a 1x1 image in a default window; 212.7 is
        // 4x3; the rest bracket the range either side of the old threshold.
        for fit in [1e-6, 0.01, 1.0, 31.9, 42.5, 199.0, 200.0, 212.7, 638.0, 1e6] {
            let (lo, hi) = zoom_bounds(fit);
            assert!(lo <= hi, "bounds crossed at fit={fit}: {lo} > {hi}");
            // Exercising the real call proves the panic cannot come back.
            let _ = (fit * 2.0).clamp(lo, hi);
        }
    }

    #[test]
    fn zoom_ceiling_lets_a_tiny_image_fill_the_window() {
        // A 1x1 needs ~638x just to fit, so a flat 40x cap would pin it below
        // fit-to-window and make zooming a no-op.
        let (_, hi) = zoom_bounds(638.0);
        assert!(hi >= 638.0);
        // Normal images keep the plain 40x ceiling.
        assert_eq!(zoom_bounds(1.5), (0.3, MAX_ZOOM));
    }

    #[test]
    fn only_decodable_extensions_are_offered() {
        // No decoder in this build: listing it only fills the strip with files
        // that can never open.
        assert!(!is_image(Path::new("photo.avif")));
        // The crate knows farbfeld as `.ff` and nothing else, so `.farbfeld`
        // could never open and `.ff` was being hidden.
        assert!(is_image(Path::new("photo.ff")));
        assert!(!is_image(Path::new("photo.farbfeld")));
        assert!(is_image(Path::new("photo.JPG")));
        assert!(is_image(Path::new("photo.png")));
        assert!(!is_image(Path::new("notes.txt")));
        assert!(!is_image(Path::new("noextension")));
    }

    #[test]
    fn jpeg_detection_is_case_insensitive() {
        assert!(is_jpeg(Path::new("a.jpg")));
        assert!(is_jpeg(Path::new("a.JPEG")));
        assert!(!is_jpeg(Path::new("a.png")));
    }

    #[test]
    fn middle_ellipsis_keeps_the_extension() {
        assert_eq!(middle_ellipsis("short.png", 16), "short.png");
        let out = middle_ellipsis("a-very-long-file-name-indeed.jpeg", 16);
        assert_eq!(out.chars().count(), 16);
        assert!(out.starts_with('a') && out.ends_with("jpeg"));
        assert_eq!(middle_ellipsis("abc", 1), "…");
        // Multi-byte names must not be split mid-character.
        assert_eq!(middle_ellipsis("ααααααααββββββββ.png", 8).chars().count(), 8);
    }

    #[test]
    fn anim_budget_defaults_to_one_gib() {
        // The operator picked 1 GB on 2026-08-16. The env override is what makes
        // the cap testable without generating a gigabyte of GIF, so it must not
        // change the default when unset or unparseable.
        assert_eq!(ANIM_RAM_BUDGET_MB, 1024);
        std::env::remove_var("IMGVIEW_ANIM_BUDGET_MB");
        assert_eq!(anim_budget_bytes(), 1024 * 1024 * 1024);
        std::env::set_var("IMGVIEW_ANIM_BUDGET_MB", "nonsense");
        assert_eq!(anim_budget_bytes(), 1024 * 1024 * 1024);
        std::env::set_var("IMGVIEW_ANIM_BUDGET_MB", "32");
        assert_eq!(anim_budget_bytes(), 32 * 1024 * 1024);
        std::env::remove_var("IMGVIEW_ANIM_BUDGET_MB");
    }

    #[test]
    fn fit_within_never_upscales() {
        assert_eq!(fit_within(Vec2::new(50.0, 20.0), 96.0), Vec2::new(50.0, 20.0));
        assert_eq!(fit_within(Vec2::new(192.0, 96.0), 96.0), Vec2::new(96.0, 48.0));
    }
}
