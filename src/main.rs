//! ImgView — a lightweight, Picasa-style image viewer.
//!
//! One big image up top with scroll-to-zoom / drag-to-pan, a thumbnail strip
//! along the bottom, arrow-key navigation, and 90° rotate (with save-to-disk).

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};
use std::time::Duration;

use eframe::egui;
use egui::{Align, Color32, Key, Rect, Sense, Stroke, TextureHandle, TextureOptions, Vec2};
use image::codecs::gif::GifDecoder;
use image::codecs::webp::WebPDecoder;
use image::{AnimationDecoder, DynamicImage};

const THUMB: u32 = 96;
const IMAGE_EXTS: &[&str] = &[
    "jpg", "jpeg", "png", "gif", "bmp", "webp", "tif", "tiff", "ppm", "pgm",
    "pbm", "pnm", "tga", "ico", "dds", "hdr", "exr", "qoi", "avif", "farbfeld",
];

fn is_image(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| IMAGE_EXTS.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
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

/// Collect an animation decoder's frames, clamping delays the way browsers do.
fn collect_anim(frames: image::Frames) -> Vec<AnimFrame> {
    match frames.collect_frames() {
        Ok(list) => list
            .into_iter()
            .map(|f| {
                let mut delay: Duration = f.delay().into();
                if delay < Duration::from_millis(20) {
                    delay = Duration::from_millis(100);
                }
                AnimFrame {
                    image: DynamicImage::ImageRgba8(f.into_buffer()),
                    delay,
                }
            })
            .collect(),
        Err(_) => Vec::new(),
    }
}

/// Decode a file into frames. Static images yield a single frame; animated
/// GIF/WebP yield every frame with per-frame delays.
fn load_frames(path: &Path) -> Vec<AnimFrame> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();

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
        Ok(img) => vec![AnimFrame {
            image: img,
            delay: Duration::ZERO,
        }],
        Err(_) => Vec::new(),
    }
}

/// A finished thumbnail decode, shipped from the loader thread to the UI.
struct ThumbMsg {
    index: usize,
    image: egui::ColorImage,
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
    thumbs: Vec<Option<TextureHandle>>,
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
        self.thumbs = vec![None; self.paths.len()];
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
                if let Ok(img) = image::open(path) {
                    let thumb = img.thumbnail(THUMB, THUMB);
                    let image = to_color_image(&thumb);
                    // If the receiver is gone (folder changed), stop early.
                    if tx.send(ThumbMsg { index, image }).is_err() {
                        break;
                    }
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
        let frames = load_frames(&path);
        if frames.is_empty() {
            self.status = format!("Failed to open {}", path.display());
            return;
        }
        self.frames = frames;
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
        self.status = format!(
            "{}  [{}/{}]{}",
            path.file_name().unwrap_or_default().to_string_lossy(),
            index + 1,
            self.paths.len(),
            anim
        );
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
        match rotated.save(&path) {
            Ok(()) => {
                // Bake rotation into our source and refresh this thumbnail.
                let thumb = to_color_image(&rotated.thumbnail(THUMB, THUMB));
                self.thumbs[self.index] =
                    Some(ctx.load_texture("thumb", thumb, TextureOptions::LINEAR));
                self.frames[0] = AnimFrame {
                    image: rotated,
                    delay: Duration::ZERO,
                };
                self.angle = 0;
                self.upload_current(ctx);
                self.status = format!(
                    "Saved rotation → {}",
                    path.file_name().unwrap_or_default().to_string_lossy()
                );
            }
            Err(e) => self.status = format!("Save failed: {e}"),
        }
    }

    // ---- view transform --------------------------------------------------
    fn layout(&mut self, rect: Rect, img: Vec2) {
        // Fit-to-window, centered.
        self.scale = (rect.width() / img.x).min(rect.height() / img.y).min(1e6);
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
                            self.thumbs[msg.index] = Some(ctx.load_texture(
                                "thumb",
                                msg.image,
                                TextureOptions::LINEAR,
                            ));
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
        ctx.input(|i| {
            let shift = i.modifiers.shift;
            let ctrl = i.modifiers.ctrl || i.modifiers.command;
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
                if ui.button("📂 Open Folder").clicked() {
                    if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                        self.open_folder(ctx, &dir, None);
                    }
                }
                ui.separator();
                if ui.button("◀ Prev").clicked() {
                    go_prev = true;
                }
                if ui.button("Next ▶").clicked() {
                    go_next = true;
                }
                ui.separator();
                if ui.button("⟲ Rotate L").clicked() {
                    rot_ccw = true;
                }
                if ui.button("⟳ Rotate R").clicked() {
                    rot_cw = true;
                }
                if ui.button("💾 Save").clicked() {
                    do_save = true;
                }
                // Playback-speed controls, only relevant for animations.
                if self.frames.len() > 1 {
                    ui.separator();
                    if ui.button("🐢 Slower").clicked() {
                        slower = true;
                    }
                    if ui.button("🐇 Faster").clicked() {
                        faster = true;
                    }
                    if ui.button("Reset").clicked() {
                        reset_speed = true;
                    }
                    ui.label(format!("{:.2}×", self.speed));
                }
                ui.separator();
                ui.label(&self.status);
            });
        });

        // ---- bottom thumbnail strip ----
        let mut clicked: Option<usize> = None;
        egui::TopBottomPanel::bottom("thumbs")
            .exact_height((THUMB + 24) as f32)
            .show(ctx, |ui| {
                egui::ScrollArea::horizontal()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            for i in 0..self.paths.len() {
                                let selected = i == self.index;
                                let stroke = if selected {
                                    Stroke::new(2.0, Color32::from_rgb(90, 160, 240))
                                } else {
                                    Stroke::NONE
                                };
                                let resp = egui::Frame::none()
                                    .stroke(stroke)
                                    .inner_margin(2.0)
                                    .show(ui, |ui| {
                                        if let Some(tex) = &self.thumbs[i] {
                                            let size = fit_within(tex.size_vec2(), THUMB as f32);
                                            ui.add(
                                                egui::Image::new((tex.id(), size))
                                                    .sense(Sense::click()),
                                            )
                                        } else {
                                            ui.add_sized(
                                                [THUMB as f32, THUMB as f32],
                                                egui::Spinner::new(),
                                            )
                                        }
                                    })
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
                        let new_scale = (self.scale * factor).clamp(
                            self.layout_fit_scale(rect, img) * 0.2,
                            40.0,
                        );
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
                    ui.centered_and_justified(|ui| {
                        ui.label("Open a folder or drop an image  (📂 or Ctrl+O)");
                    });
                }
            });

        // ---- drag & drop ----
        let dropped = ctx.input(|i| i.raw.dropped_files.clone());
        if let Some(file) = dropped.into_iter().find_map(|f| f.path) {
            self.open_path(ctx, &file);
        }

        // ---- apply actions ----
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
    fn layout_fit_scale(&self, rect: Rect, img: Vec2) -> f32 {
        (rect.width() / img.x).min(rect.height() / img.y)
    }
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
            let frames = load_frames(Path::new(p));
            println!("frames: {}", frames.len());
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
