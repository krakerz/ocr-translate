use std::cell::Cell;
use std::rc::Rc;
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

/// One watched region's rectangle plus its independent OCR/translate state
/// — each region is tracked separately (own last-seen text for dedup, own
/// status), so one region updating doesn't affect another's displayed
/// content. Indices into the shared `Vec<RegionState>` stay stable across
/// the session's lifetime since regions are only ever appended to, never
/// removed.
struct RegionState {
    rect: (u32, u32, u32, u32),
    source: String,
    translated: String,
    status: Option<Status>,
    last_text: Option<String>,
}

impl RegionState {
    fn new(rect: (u32, u32, u32, u32)) -> Self {
        Self {
            rect,
            source: String::new(),
            translated: String::new(),
            status: None,
            last_text: None,
        }
    }
}

type SharedRegions = Arc<Mutex<Vec<RegionState>>>;

/// Watches one or more fixed screen regions via a continuous capture
/// session (Linux: portal `ScreenCast` + PipeWire; Windows: `xcap`'s
/// DXGI-based `VideoRecorder` — see `capture::RegionSession`): OCRs each
/// region every `poll_interval_ms`, and re-translates whenever its
/// recognized text changes, independently of the others. Deliberately
/// never touches `history`, matching `live_translate::run` — this is for
/// quick, disposable lookups (subtitles, a status readout, a chat window),
/// not a record of what's been translated.
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
    // Shared (not moved into the watcher thread outright) via `Arc`, since
    // the main thread also needs a frame each time the user adds another
    // region later — `latest_frame` only needs `&self`, so both the
    // watcher thread and this one can hold their own clone freely.
    let session =
        Arc::new(RegionSession::start().context("failed to start the screen capture session")?);

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
        crate::capture::select_region_rect(&DynamicImage::ImageRgba8(first_frame), &cfg.popup, &[])?
    else {
        tracing::info!("region selection cancelled");
        return Ok(());
    };

    let regions: SharedRegions = Arc::new(Mutex::new(vec![RegionState::new(rect)]));
    spawn_watcher(cfg.clone(), session.clone(), regions.clone());

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

    // The window closes and reopens each time the user adds a region (see
    // `LiveRegionApp::update`'s "+ Add Region" button) rather than staying
    // open throughout — eframe/winit don't support two `run_native` windows
    // open at once in one process, so the region-selection overlay (its own
    // `select_region_rect` window) and this results window take turns
    // instead. The background watcher thread (and the capture session
    // itself) keeps running the whole time regardless, so no translation
    // progress is lost while a window is briefly closed between the two.
    let add_region_requested = Rc::new(Cell::new(false));
    loop {
        let app = LiveRegionApp {
            regions: regions.clone(),
            show_source: cfg.region_translate.show_source_by_default,
            font_size: cfg.translate.font_size,
            copied_flash: None,
            add_region_requested: add_region_requested.clone(),
        };

        eframe::run_native(
            "ocr-translate-live-region",
            native_options.clone(),
            Box::new(|cc| {
                crate::fonts::install_cjk_fallback(&cc.egui_ctx);
                Ok(Box::new(app))
            }),
        )
        .map_err(|e| anyhow::anyhow!("live region translate window failed: {e}"))?;

        if !add_region_requested.get() {
            break;
        }
        add_region_requested.set(false);

        let frame = match wait_for_frame(&session, Duration::from_secs(15)) {
            Ok(f) => f,
            Err(e) => {
                tracing::warn!("failed to grab a frame for the new region: {e:#}");
                continue;
            }
        };
        let existing: Vec<_> = regions.lock().unwrap().iter().map(|r| r.rect).collect();
        match crate::capture::select_region_rect(
            &DynamicImage::ImageRgba8(frame),
            &cfg.popup,
            &existing,
        ) {
            Ok(Some(new_rect)) => regions.lock().unwrap().push(RegionState::new(new_rect)),
            Ok(None) => tracing::info!("adding a region was cancelled"),
            Err(e) => tracing::warn!("failed to select a new region: {e:#}"),
        }
    }
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

fn spawn_watcher(cfg: AppConfig, session: Arc<RegionSession>, regions: SharedRegions) {
    std::thread::spawn(move || {
        // `session` (shared via `Arc` with the main thread, which also
        // needs a frame each time a region is added) is held here for as
        // long as this thread runs; dropping every clone of it would end
        // the capture session, but this thread's own clone alone keeps it
        // alive even after `run()`'s main loop finishes using its copy.
        let poll_interval = Duration::from_millis(cfg.region_translate.poll_interval_ms.max(100));

        loop {
            std::thread::sleep(poll_interval);
            let Some(frame) = session.latest_frame() else {
                continue;
            };

            // Snapshot the current rectangles rather than holding the lock
            // for the whole loop body below (OCR + translation are slow;
            // holding the lock that long would stall the UI thread's own
            // reads for rendering). New regions added mid-poll just get
            // picked up on the next tick.
            let rects: Vec<(u32, u32, u32, u32)> =
                regions.lock().unwrap().iter().map(|r| r.rect).collect();

            for (index, &(x, y, w, h)) in rects.iter().enumerate() {
                let cropped = crop_frame(&frame, x, y, w, h);
                let text = match crate::ocr::recognize(&cropped, &cfg.ocr) {
                    Ok(t) => t,
                    Err(e) => {
                        if let Some(r) = regions.lock().unwrap().get_mut(index) {
                            r.status = Some(Status::Error(format!("OCR failed: {e:#}")));
                        }
                        continue;
                    }
                };
                if text.is_empty() {
                    continue;
                }
                let changed = {
                    let mut guard = regions.lock().unwrap();
                    let Some(r) = guard.get_mut(index) else {
                        continue;
                    };
                    if r.last_text.as_deref() == Some(text.as_str()) {
                        false
                    } else {
                        r.last_text = Some(text.clone());
                        r.source = text.clone();
                        r.status = Some(Status::Translating);
                        true
                    }
                };
                if changed {
                    translate_and_store(&cfg, &text, &regions, index);
                }
            }
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

fn translate_and_store(cfg: &AppConfig, text: &str, regions: &SharedRegions, index: usize) {
    let result = translate::translate_with_fallback(
        cfg,
        TranslateRequest {
            text,
            source_lang: &cfg.general.source_lang,
            target_lang: &cfg.general.target_lang,
        },
    );
    let mut guard = regions.lock().unwrap();
    let Some(r) = guard.get_mut(index) else {
        return;
    };
    match result {
        Ok((provider, translated)) => {
            r.translated = translated;
            r.status = Some(Status::Done { provider });
        }
        Err(e) => {
            r.status = Some(Status::Error(format!("{e:#}")));
        }
    }
}

struct LiveRegionApp {
    regions: SharedRegions,
    show_source: bool,
    font_size: f32,
    /// Which region's "Copy" button was last clicked, and when — for the
    /// brief "Copied!" confirmation next to that specific region's button.
    copied_flash: Option<(usize, Instant)>,
    /// Set by the "+ Add Region" button, read by `run()` after this
    /// viewport closes to decide whether to reopen it (with the new region
    /// added) or actually quit.
    add_region_requested: Rc<Cell<bool>>,
}

impl eframe::App for LiveRegionApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }

        if let Some((_, t)) = self.copied_flash {
            if t.elapsed().as_secs_f32() >= 1.5 {
                self.copied_flash = None;
            }
        }

        struct RegionSnapshot {
            status_label: String,
            source: String,
            translated: String,
        }
        let snapshot: Vec<RegionSnapshot> = {
            let guard = self.regions.lock().unwrap();
            guard
                .iter()
                .map(|r| RegionSnapshot {
                    status_label: r
                        .status
                        .as_ref()
                        .map(Status::label)
                        .unwrap_or_else(|| Status::Watching.label()),
                    source: r.source.clone(),
                    translated: r.translated.clone(),
                })
                .collect()
        };

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.style_mut().override_font_id = Some(egui::FontId::proportional(self.font_size));

            ui.horizontal(|ui| {
                ui.heading("Live Region Translate");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.checkbox(&mut self.show_source, "Show source");
                    if ui.button("+ Add Region").clicked() {
                        self.add_region_requested.set(true);
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                });
            });
            ui.separator();

            egui::ScrollArea::vertical()
                .id_source("regions_scroll")
                .show(ui, |ui| {
                    for (index, region) in snapshot.iter().enumerate() {
                        ui.group(|ui| {
                            ui.set_width(ui.available_width());
                            ui.horizontal(|ui| {
                                ui.strong(format!("Region {}", index + 1));
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        if ui.small_button("Copy").clicked() {
                                            ctx.copy_text(region.translated.clone());
                                            self.copied_flash = Some((index, Instant::now()));
                                        }
                                        if matches!(self.copied_flash, Some((i, _)) if i == index)
                                        {
                                            ui.label("Copied!");
                                        }
                                    },
                                );
                            });
                            ui.label(egui::RichText::new(&region.status_label).weak());

                            if self.show_source {
                                ui.label(egui::RichText::new("Source").weak());
                                ui.add(egui::Label::new(&region.source).wrap());
                                ui.add_space(4.0);
                            }
                            ui.label(egui::RichText::new("Translated").weak());
                            ui.add(egui::Label::new(&region.translated).wrap());
                        });
                        ui.add_space(6.0);
                    }
                });

            ui.add_space(4.0);
            if ui.button("Close").clicked() {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        });

        ctx.request_repaint_after(Duration::from_millis(200));
    }
}
