use std::cell::RefCell;
use std::rc::Rc;
use std::time::Instant;

use anyhow::Result;

use crate::config::PopupConfig;

/// The user's answer to a [`show_conflict`] prompt.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ConflictChoice {
    /// Do nothing — closing the window any other way (Escape, the X
    /// button) counts as this too.
    Cancel,
    /// Terminate the other session (see `session_lock::terminate_process`)
    /// and proceed with whatever the caller was trying to do.
    StopAndContinue,
}

/// A small blocking dialog for the mutual-exclusion rules between Live
/// Region Translate, Live Clipboard Translate, and one-shot `capture` (see
/// `session_lock.rs`) — blocks the calling process until the user answers,
/// same as every other popup in this module. `can_force_stop` controls
/// whether the "Stop it and continue" button shows at all — the caller
/// only knows the other session's PID (needed to actually stop it) via a
/// racy, best-effort read (`SessionLock::holder_pid`), so a prompt with no
/// usable PID just offers Cancel.
pub fn show_conflict(title: &str, message: &str, can_force_stop: bool) -> Result<ConflictChoice> {
    let choice = Rc::new(RefCell::new(ConflictChoice::Cancel));
    let app = ConflictApp {
        title: title.to_string(),
        message: message.to_string(),
        can_force_stop,
        choice: choice.clone(),
    };
    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([460.0, 220.0])
            .with_always_on_top()
            .with_icon(crate::icon::egui_icon(128))
            .with_title(title),
        ..Default::default()
    };
    eframe::run_native(
        "ocr-translate-conflict",
        native_options,
        Box::new(|cc| {
            crate::fonts::install_cjk_fallback(&cc.egui_ctx);
            Ok(Box::new(app))
        }),
    )
    .map_err(|e| anyhow::anyhow!("conflict prompt window failed: {e}"))?;
    let result = *choice.borrow();
    Ok(result)
}

pub fn show_result(
    original: &str,
    translated: &str,
    provider: &str,
    cfg: &PopupConfig,
) -> Result<()> {
    let app = PopupApp {
        original: original.to_string(),
        translated: translated.to_string(),
        provider: provider.to_string(),
        font_size: cfg.font_size,
        auto_close_secs: cfg.auto_close_secs,
        opened_at: Instant::now(),
        copied_flash: None,
    };

    let (width, height) = crate::capture::clamp_to_screen(cfg.width, cfg.height);
    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([width, height])
            .with_always_on_top()
            .with_icon(crate::icon::egui_icon(128))
            .with_title("Translation"),
        ..Default::default()
    };

    eframe::run_native(
        "ocr-translate-popup",
        native_options,
        Box::new(|cc| {
            crate::fonts::install_cjk_fallback(&cc.egui_ctx);
            Ok(Box::new(app))
        }),
    )
    .map_err(|e| anyhow::anyhow!("popup window failed: {e}"))?;
    Ok(())
}

pub fn show_error(message: &str) -> Result<()> {
    let app = ErrorApp {
        message: message.to_string(),
    };
    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([460.0, 220.0])
            .with_always_on_top()
            .with_icon(crate::icon::egui_icon(128))
            .with_title("ocr-translate: error"),
        ..Default::default()
    };
    eframe::run_native(
        "ocr-translate-error",
        native_options,
        Box::new(|cc| {
            crate::fonts::install_cjk_fallback(&cc.egui_ctx);
            Ok(Box::new(app))
        }),
    )
    .map_err(|e| anyhow::anyhow!("error window failed: {e}"))?;
    Ok(())
}

struct PopupApp {
    original: String,
    translated: String,
    provider: String,
    font_size: f32,
    auto_close_secs: u64,
    opened_at: Instant,
    copied_flash: Option<Instant>,
}

impl eframe::App for PopupApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.auto_close_secs > 0 && self.opened_at.elapsed().as_secs() >= self.auto_close_secs {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.style_mut().override_font_id = Some(egui::FontId::proportional(self.font_size));

            ui.heading("Translation");
            ui.label(egui::RichText::new(format!("via {}", self.provider)).weak());
            ui.separator();

            ui.label(egui::RichText::new("Original").weak());
            egui::ScrollArea::vertical()
                .id_source("original")
                .max_height(ui.available_height() * 0.4)
                .show(ui, |ui| {
                    ui.add(egui::Label::new(&self.original).wrap());
                });

            ui.add_space(8.0);
            ui.label(egui::RichText::new("Translated").weak());
            egui::ScrollArea::vertical()
                .id_source("translated")
                .show(ui, |ui| {
                    ui.add(egui::Label::new(&self.translated).wrap());
                });

            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("Copy translation").clicked() {
                    ctx.copy_text(self.translated.clone());
                    self.copied_flash = Some(Instant::now());
                }
                if ui.button("Close").clicked() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
                if let Some(t) = self.copied_flash {
                    if t.elapsed().as_secs_f32() < 1.5 {
                        ui.label("Copied!");
                    } else {
                        self.copied_flash = None;
                    }
                }
            });
        });

        ctx.request_repaint_after(std::time::Duration::from_millis(200));
    }
}

struct ErrorApp {
    message: String,
}

impl eframe::App for ErrorApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.colored_label(egui::Color32::from_rgb(220, 80, 80), "ocr-translate failed");
            ui.separator();
            ui.add(egui::Label::new(&self.message).wrap());
            if ui.button("Close").clicked() {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        });
    }
}

struct ConflictApp {
    title: String,
    message: String,
    can_force_stop: bool,
    choice: Rc<RefCell<ConflictChoice>>,
}

impl eframe::App for ConflictApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading(&self.title);
            ui.separator();
            ui.add(egui::Label::new(&self.message).wrap());
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if ui.button("Cancel").clicked() {
                    *self.choice.borrow_mut() = ConflictChoice::Cancel;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
                if self.can_force_stop && ui.button("Stop it and continue").clicked() {
                    *self.choice.borrow_mut() = ConflictChoice::StopAndContinue;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
        });
    }
}
