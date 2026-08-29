use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;

use crate::config::AppConfig;
use crate::translate::{self, TranslateRequest};

#[derive(Clone)]
enum Status {
    WaitingForClipboard,
    Translating,
    Done { provider: String },
    Error(String),
}

impl Status {
    fn label(&self) -> String {
        match self {
            Status::WaitingForClipboard => "Waiting for clipboard text...".to_string(),
            Status::Translating => "Translating...".to_string(),
            Status::Done { provider } => format!("Up to date (via {provider})"),
            Status::Error(e) => format!("Error: {e}"),
        }
    }
}

#[derive(Default)]
struct LiveState {
    source: String,
    translated: String,
    status: Option<Status>,
}

/// Watches the clipboard for text changes and shows a live-updating
/// translation popup: copy something, see it translated within
/// `poll_interval_ms`; copy something else, it updates in place. Deliberately
/// never touches `history` — this is meant for quick, disposable lookups,
/// not a record of what you've translated.
///
/// Runs as its own process (spawned by the daemon, like `capture` and
/// `show-history`) for the same reason those do: this opens an eframe/winit
/// window, which can't safely share a process with the tray's GTK main loop.
pub fn run(cfg: &AppConfig) -> Result<()> {
    let state = Arc::new(Mutex::new(LiveState::default()));
    spawn_watcher(cfg.clone(), state.clone());

    let app = LiveTranslateApp {
        state,
        show_source: cfg.live_translate.show_source_by_default,
        font_size: cfg.translate.font_size,
        copied_flash: None,
    };

    let (width, height) =
        crate::capture::clamp_to_screen(cfg.translate.width, cfg.translate.height);
    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([width, height])
            .with_always_on_top()
            .with_icon(crate::icon::egui_icon(128))
            .with_title("Live Clipboard Translate"),
        ..Default::default()
    };

    eframe::run_native(
        "ocr-translate-live",
        native_options,
        Box::new(|cc| {
            crate::fonts::install_cjk_fallback(&cc.egui_ctx);
            Ok(Box::new(app))
        }),
    )
    .map_err(|e| anyhow::anyhow!("live translate window failed: {e}"))?;
    Ok(())
}

fn spawn_watcher(cfg: AppConfig, state: Arc<Mutex<LiveState>>) {
    std::thread::spawn(move || {
        let mut clipboard = match arboard::Clipboard::new() {
            Ok(c) => c,
            Err(e) => {
                state.lock().unwrap().status =
                    Some(Status::Error(format!("failed to open the clipboard: {e}")));
                return;
            }
        };

        let poll_interval = Duration::from_millis(cfg.live_translate.poll_interval_ms.max(50));
        let mut last_seen: Option<String> = None;

        loop {
            if let Ok(text) = clipboard.get_text() {
                let trimmed = text.trim();
                if !trimmed.is_empty() && last_seen.as_deref() != Some(text.as_str()) {
                    last_seen = Some(text.clone());
                    {
                        let mut s = state.lock().unwrap();
                        s.source = text.clone();
                        s.status = Some(Status::Translating);
                    }
                    translate_and_store(&cfg, &text, &state);
                }
            }
            std::thread::sleep(poll_interval);
        }
    });
}

fn translate_and_store(cfg: &AppConfig, text: &str, state: &Arc<Mutex<LiveState>>) {
    let result = translate::translate_with_fallback(
        cfg,
        TranslateRequest {
            text,
            source_lang: &cfg.general.source_lang,
            target_lang: &cfg.general.target_lang,
        },
    );
    let mut s = state.lock().unwrap();
    match result {
        Ok((provider, translated)) => {
            s.translated = translated;
            s.status = Some(Status::Done { provider });
        }
        Err(e) => {
            s.status = Some(Status::Error(format!("{e:#}")));
        }
    }
}

struct LiveTranslateApp {
    state: Arc<Mutex<LiveState>>,
    show_source: bool,
    font_size: f32,
    copied_flash: Option<Instant>,
}

impl eframe::App for LiveTranslateApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }

        let (source, translated, status_label) = {
            let s = self.state.lock().unwrap();
            let status = s
                .status
                .as_ref()
                .map(Status::label)
                .unwrap_or_else(|| Status::WaitingForClipboard.label());
            (s.source.clone(), s.translated.clone(), status)
        };

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.style_mut().override_font_id = Some(egui::FontId::proportional(self.font_size));

            ui.horizontal(|ui| {
                ui.heading("Live Clipboard Translate");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.checkbox(&mut self.show_source, "Show source");
                });
            });
            ui.label(egui::RichText::new(status_label).weak());
            ui.separator();

            if self.show_source {
                ui.label(egui::RichText::new("Source").weak());
                egui::ScrollArea::vertical()
                    .id_source("live_source")
                    .max_height(ui.available_height() * 0.4)
                    .show(ui, |ui| {
                        ui.add(egui::Label::new(&source).wrap());
                    });
                ui.add_space(8.0);
            }

            ui.label(egui::RichText::new("Translated").weak());
            egui::ScrollArea::vertical()
                .id_source("live_translated")
                .show(ui, |ui| {
                    ui.add(egui::Label::new(&translated).wrap());
                });

            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("Copy translation").clicked() {
                    ctx.copy_text(translated.clone());
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

        ctx.request_repaint_after(Duration::from_millis(200));
    }
}
