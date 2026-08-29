use std::sync::{Arc, Mutex};

use anyhow::Result;
use image::DynamicImage;

use crate::config::CaptureWindowConfig;

/// Shows the captured screenshot in a borderless window and lets the user
/// scroll to zoom, right-drag to pan, and left-drag a rectangle to crop.
/// Returns `None` if the user cancels (Esc).
///
/// Rationale: neither X11 nor Wayland offer one portable API for a live
/// click-through region overlay on the real desktop, so instead we grab the
/// whole screen first and let the user crop the still image in-app — this
/// works identically on every compositor. Zoom makes it practical to select
/// small text precisely, which a 1:1 view can't on a high-resolution monitor.
pub fn select_crop(
    image: &DynamicImage,
    cfg: &CaptureWindowConfig,
) -> Result<Option<DynamicImage>> {
    let Some((x0, y0, w, h)) = select_region_rect(image, cfg)? else {
        return Ok(None);
    };
    Ok(Some(image.crop_imm(x0, y0, w, h)))
}

/// Same interactive zoom/pan/select UI as [`select_crop`], but returns the
/// chosen rectangle (in `image`'s pixel space) instead of the cropped image
/// itself — used by the live-region-translate feature, which needs to keep
/// re-cropping the same rectangle out of subsequent ScreenCast frames rather
/// than a single still image.
pub fn select_region_rect(
    image: &DynamicImage,
    cfg: &CaptureWindowConfig,
) -> Result<Option<(u32, u32, u32, u32)>> {
    let rgba = image.to_rgba8();
    let (width, height) = (rgba.width(), rgba.height());
    let color_image =
        egui::ColorImage::from_rgba_unmultiplied([width as usize, height as usize], rgba.as_raw());

    let result: Arc<Mutex<Option<((u32, u32), (u32, u32))>>> = Arc::new(Mutex::new(None));
    let base_scale = fit_scale(width, height, cfg.width, cfg.height);
    let app = SelectorApp {
        color_image: Some(color_image),
        texture: None,
        image_size: (width, height),
        base_scale,
        zoom: 1.0,
        pan: egui::Vec2::ZERO,
        drag_start: None,
        drag_current: None,
        result: result.clone(),
    };

    let (init_w, init_h) =
        crate::capture::clamp_to_screen(width as f32 * base_scale, height as f32 * base_scale);
    let mut viewport = egui::ViewportBuilder::default()
        .with_decorations(false)
        .with_inner_size([init_w, init_h])
        .with_icon(crate::icon::egui_icon(128))
        .with_title("ocr-translate: scroll to zoom, drag to select");
    if cfg.always_on_top {
        viewport = viewport.with_always_on_top();
    }
    let native_options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    eframe::run_native(
        "ocr-translate-selector",
        native_options,
        Box::new(|cc| {
            crate::fonts::install_cjk_fallback(&cc.egui_ctx);
            Ok(Box::new(app))
        }),
    )
    .map_err(|e| anyhow::anyhow!("crop selector window failed: {e}"))?;

    let Some(((x0, y0), (x1, y1))) = result.lock().unwrap().take() else {
        return Ok(None);
    };
    if x1 <= x0 || y1 <= y0 {
        return Ok(None);
    }
    Ok(Some((x0, y0, x1 - x0, y1 - y0)))
}

/// Large screenshots (multi-monitor, 4K) shouldn't force an oversized native
/// window; scale the *initial* display down to fit within `max_w`x`max_h`
/// (`popup.width`/`height`), while zoom/pan let the user get back to 1:1 (or
/// closer) for precise selection.
fn fit_scale(width: u32, height: u32, max_w: f32, max_h: f32) -> f32 {
    let (w, h) = (width as f32, height as f32);
    (max_w / w).min(max_h / h).min(1.0)
}

struct SelectorApp {
    color_image: Option<egui::ColorImage>,
    texture: Option<egui::TextureHandle>,
    image_size: (u32, u32),
    /// Scale that fits the whole image in the initial window size.
    base_scale: f32,
    /// User-adjustable multiplier on top of `base_scale`, via scroll.
    zoom: f32,
    /// Screen-space offset of the image's top-left corner from the panel's
    /// top-left corner, via right-drag.
    pan: egui::Vec2,
    drag_start: Option<egui::Pos2>,
    drag_current: Option<egui::Pos2>,
    result: Arc<Mutex<Option<((u32, u32), (u32, u32))>>>,
}

impl SelectorApp {
    fn effective_scale(&self) -> f32 {
        self.base_scale * self.zoom
    }

    fn screen_to_image(&self, panel_origin: egui::Pos2, p: egui::Pos2) -> egui::Pos2 {
        let image_origin = panel_origin + self.pan;
        ((p - image_origin) / self.effective_scale()).to_pos2()
    }
}

impl eframe::App for SelectorApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.texture.is_none() {
            if let Some(img) = self.color_image.take() {
                self.texture =
                    Some(ctx.load_texture("screenshot", img, egui::TextureOptions::LINEAR));
            }
        }

        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            *self.result.lock().unwrap() = None;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }

        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(egui::Color32::from_gray(20)))
            .show(ctx, |ui| {
                let Some(texture) = &self.texture else { return };
                let panel_rect = ui.available_rect_before_wrap();
                let response = ui.allocate_rect(panel_rect, egui::Sense::click_and_drag());

                // Scroll to zoom, keeping the point under the cursor fixed.
                if response.hovered() {
                    let scroll_y = ctx.input(|i| i.smooth_scroll_delta.y);
                    if scroll_y.abs() > f32::EPSILON {
                        if let Some(pointer) = ctx.input(|i| i.pointer.hover_pos()) {
                            let old_scale = self.effective_scale();
                            let image_pt_under_cursor =
                                (pointer - (panel_rect.min + self.pan)) / old_scale;
                            self.zoom = (self.zoom * (1.0 + scroll_y * 0.001)).clamp(0.1, 12.0);
                            let new_scale = self.effective_scale();
                            self.pan = pointer - panel_rect.min - image_pt_under_cursor * new_scale;
                        }
                    }
                }

                let pointer = ctx.input(|i| i.pointer.clone());

                // Right-drag pans.
                if pointer.secondary_down() {
                    self.pan += pointer.delta();
                }

                // Left-drag selects the crop rectangle.
                if pointer.primary_pressed() {
                    self.drag_start = pointer.interact_pos();
                    self.drag_current = self.drag_start;
                } else if pointer.primary_down() {
                    if let Some(pos) = pointer.interact_pos() {
                        self.drag_current = Some(pos);
                    }
                } else if pointer.primary_released() {
                    if let (Some(start), Some(current)) = (self.drag_start, self.drag_current) {
                        let selection = egui::Rect::from_two_pos(start, current);
                        if selection.width() > 3.0 && selection.height() > 3.0 {
                            let (w, h) = self.image_size;
                            let p0 = self.screen_to_image(panel_rect.min, selection.min);
                            let p1 = self.screen_to_image(panel_rect.min, selection.max);
                            let x0 = p0.x.clamp(0.0, w as f32) as u32;
                            let y0 = p0.y.clamp(0.0, h as f32) as u32;
                            let x1 = p1.x.clamp(0.0, w as f32) as u32;
                            let y1 = p1.y.clamp(0.0, h as f32) as u32;
                            *self.result.lock().unwrap() = Some(((x0, y0), (x1, y1)));
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                    }
                    self.drag_start = None;
                    self.drag_current = None;
                }

                let scale = self.effective_scale();
                let image_origin = panel_rect.min + self.pan;
                let image_rect = egui::Rect::from_min_size(
                    image_origin,
                    egui::vec2(
                        self.image_size.0 as f32 * scale,
                        self.image_size.1 as f32 * scale,
                    ),
                );
                ui.painter().image(
                    texture.id(),
                    image_rect,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );

                if let (Some(start), Some(current)) = (self.drag_start, self.drag_current) {
                    let selection = egui::Rect::from_two_pos(start, current);
                    ui.painter().rect_stroke(
                        selection,
                        0.0,
                        egui::Stroke::new(2.0, egui::Color32::from_rgb(255, 80, 80)),
                    );
                    ui.painter().rect_filled(
                        selection,
                        0.0,
                        egui::Color32::from_rgba_unmultiplied(255, 80, 80, 40),
                    );
                }

                let hint_pos = panel_rect.min + egui::vec2(8.0, 8.0);
                ui.painter().text(
                    hint_pos,
                    egui::Align2::LEFT_TOP,
                    format!(
                        "scroll: zoom ({:.0}%) · right-drag: pan · left-drag: select · Esc: cancel",
                        self.zoom * 100.0
                    ),
                    egui::FontId::proportional(14.0),
                    egui::Color32::WHITE,
                );
            });

        ctx.request_repaint();
    }
}
