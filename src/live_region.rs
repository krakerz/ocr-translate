use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use image::DynamicImage;

use crate::capture::RegionSession;
use crate::config::AppConfig;
use crate::translate::{self, TranslateRequest};

#[derive(Clone)]
enum Status {
    Watching,
    Translating,
    Done { provider: String },
    Error(String),
}

impl Status {
    fn label(&self) -> String {
        match self {
            Status::Watching => "Watching region...".to_string(),
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

/// Watches a fixed screen region via a continuous capture session (Linux:
/// portal `ScreenCast` + PipeWire; Windows: `xcap`'s DXGI-based
/// `VideoRecorder` — see `capture::RegionSession`): OCRs it every
/// `poll_interval_ms`, and re-translates whenever the recognized text
/// changes. Deliberately never touches `history`, matching
/// `live_translate::run` — this is for quick, disposable lookups (subtitles,
/// a status readout, a chat window), not a record of what's been translated.
///
/// Runs as its own process (spawned by the daemon, like `capture` and
/// `watch-clipboard`) for the same reason those do: this opens eframe/winit
/// windows (the region selector, then the live popup), which can't safely
/// share a process with the tray's GTK main loop.
pub fn run(cfg: &AppConfig) -> Result<()> {
    // Checked before negotiating any actual screen-capture session, so a
    // conflict is resolved (or cancelled) before spending any effort on
    // that. See session_lock.rs for why: Live Clipboard Translate doesn't
    // touch the screen at all, so it isn't blocked by a running capture,
    // but it does block a new Live Region Translate session from starting.
    let mut clipboard_lock = crate::session_lock::SessionLock::open("clipboard")?;
    let Some(_) = crate::session_lock::resolve_conflict(
        &mut clipboard_lock,
        "Live Region Translate",
        "Live Clipboard Translate",
    )?
    else {
        tracing::info!("cancelled: Live Clipboard Translate is active");
        return Ok(());
    };

    // Held for this whole session (until the window closes) — this is what
    // makes `capture`/`watch-clipboard`/a second `watch-region` all detect
    // this session as active.
    let mut region_lock = crate::session_lock::SessionLock::open("region")?;
    let Some(_region_guard) = crate::session_lock::resolve_conflict(
        &mut region_lock,
        "another Live Region Translate session",
        "Live Region Translate",
    )?
    else {
        tracing::info!("cancelled: Live Region Translate is already running");
        return Ok(());
    };

    tracing::info!(
        "starting a screen capture session (on Linux, your compositor may ask you to pick a screen/window to share)..."
    );
    let session = RegionSession::start().context("failed to start the screen capture session")?;

    // On Linux, the compositor's own "share screen with..." picker dialog is
    // still visible on the real desktop for a moment after negotiation
    // succeeds (its close animation, or just a brief redraw lag) — the very
    // first frames from the stream can still show it, which would otherwise
    // get baked into the region-selection screenshot (confirmed happening on
    // a KDE/KWin session with the default 0s delay). Windows' DXGI-based
    // session needs no such picker at all, so this delay is unnecessary
    // there — kept unconditional anyway for simplicity, since it's small,
    // user-configurable (`capture_delay_secs`), and harmless to wait out on
    // a platform that doesn't need it.
    let delay = Duration::from_secs(cfg.region_translate.capture_delay_secs);
    if !delay.is_zero() {
        std::thread::sleep(delay);
    }

    let first_frame = wait_for_frame(&session, Duration::from_secs(15))
        .context("no frame received from the screen capture session")?;

    let Some(rect) =
        crate::capture::select_region_rect(&DynamicImage::ImageRgba8(first_frame), &cfg.popup)?
    else {
        tracing::info!("region selection cancelled");
        return Ok(());
    };

    let state = Arc::new(Mutex::new(LiveState::default()));
    spawn_watcher(cfg.clone(), session, rect, state.clone());

    let app = LiveRegionApp {
        state,
        show_source: cfg.region_translate.show_source_by_default,
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
            .with_title("Live Region Translate"),
        ..Default::default()
    };

    eframe::run_native(
        "ocr-translate-live-region",
        native_options,
        Box::new(|cc| {
            crate::fonts::install_cjk_fallback(&cc.egui_ctx);
            Ok(Box::new(app))
        }),
    )
    .map_err(|e| anyhow::anyhow!("live region translate window failed: {e}"))?;
    Ok(())
}

/// Polls `session.latest_frame()` until one arrives or `timeout` elapses.
fn wait_for_frame(session: &RegionSession, timeout: Duration) -> Result<image::RgbaImage> {
    let start = Instant::now();
    loop {
        if let Some(frame) = session.latest_frame() {
            return Ok(frame);
        }
        if start.elapsed() > timeout {
            anyhow::bail!("timed out waiting for the first frame from the screen capture session");
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn spawn_watcher(
    cfg: AppConfig,
    session: RegionSession,
    rect: (u32, u32, u32, u32),
    state: Arc<Mutex<LiveState>>,
) {
    std::thread::spawn(move || {
        // `session` is held here for as long as this thread runs; dropping
        // it would end the capture session.
        let poll_interval = Duration::from_millis(cfg.region_translate.poll_interval_ms.max(100));
        let (x, y, w, h) = rect;
        let mut last_text: Option<String> = None;

        loop {
            std::thread::sleep(poll_interval);
            let Some(frame) = session.latest_frame() else {
                continue;
            };
            let cropped = crop_frame(&frame, x, y, w, h);
            let text = match crate::ocr::recognize(&cropped, &cfg.ocr) {
                Ok(t) => t,
                Err(e) => {
                    state.lock().unwrap().status =
                        Some(Status::Error(format!("OCR failed: {e:#}")));
                    continue;
                }
            };
            if text.is_empty() || last_text.as_deref() == Some(text.as_str()) {
                continue;
            }
            last_text = Some(text.clone());
            {
                let mut s = state.lock().unwrap();
                s.source = text.clone();
                s.status = Some(Status::Translating);
            }
            translate_and_store(&cfg, &text, &state);
        }
    });
}

/// Crops a raw captured frame to the selected region, clamping to the
/// frame's actual bounds — the frame's resolution should match the one the
/// region was selected on, but clamping avoids a panic if a later frame ever
/// comes back smaller.
fn crop_frame(frame: &image::RgbaImage, x: u32, y: u32, w: u32, h: u32) -> DynamicImage {
    let x = x.min(frame.width().saturating_sub(1));
    let y = y.min(frame.height().saturating_sub(1));
    let w = w.min(frame.width() - x);
    let h = h.min(frame.height() - y);
    DynamicImage::ImageRgba8(frame.clone()).crop_imm(x, y, w, h)
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

struct LiveRegionApp {
    state: Arc<Mutex<LiveState>>,
    show_source: bool,
    font_size: f32,
    copied_flash: Option<Instant>,
}

impl eframe::App for LiveRegionApp {
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
                .unwrap_or_else(|| Status::Watching.label());
            (s.source.clone(), s.translated.clone(), status)
        };

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.style_mut().override_font_id = Some(egui::FontId::proportional(self.font_size));

            ui.horizontal(|ui| {
                ui.heading("Live Region Translate");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.checkbox(&mut self.show_source, "Show source");
                });
            });
            ui.label(egui::RichText::new(status_label).weak());
            ui.separator();

            if self.show_source {
                ui.label(egui::RichText::new("Source").weak());
                egui::ScrollArea::vertical()
                    .id_source("region_source")
                    .max_height(ui.available_height() * 0.4)
                    .show(ui, |ui| {
                        ui.add(egui::Label::new(&source).wrap());
                    });
                ui.add_space(8.0);
            }

            ui.label(egui::RichText::new("Translated").weak());
            egui::ScrollArea::vertical()
                .id_source("region_translated")
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
