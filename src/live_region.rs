use std::cell::Cell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use image::DynamicImage;

use crate::capture::{ExistingRegion, RegionSession};
use crate::config::AppConfig;
use crate::region_ipc::RegionCommand;
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

/// One watched region's rectangle, display name, and independent OCR/
/// translate state. Keyed by a stable id in [`RegionsData::regions`] (not a
/// plain `Vec` index), so deleting one region never shifts another's
/// identity — the watcher thread and any queued `region_ipc::RegionCommand`
/// referencing an id keep working across a delete.
struct RegionState {
    rect: (u32, u32, u32, u32),
    name: String,
    source: String,
    translated: String,
    status: Option<Status>,
    last_text: Option<String>,
}

impl RegionState {
    fn new(rect: (u32, u32, u32, u32), name: String) -> Self {
        Self {
            rect,
            name,
            source: String::new(),
            translated: String::new(),
            status: None,
            last_text: None,
        }
    }
}

/// The result of the "Quick Capture" button/`region-capture` CLI command —
/// a one-shot `capture` (see `main::run_capture_pipeline`) run without
/// leaving the Live Region Translate window, shown in its own toggleable
/// slot at the top of the list rather than mixed in with the persistently
/// watched regions.
struct QuickCaptureState {
    source: String,
    translated: String,
    provider: String,
    visible: bool,
}

/// All state shared between the results window and the background watcher
/// thread. `next_id` only ever increases — ids are never reused, even
/// after a region is deleted, so a stale `region-rename`/`region-delete`
/// command referencing an already-deleted id just harmlessly finds nothing.
struct RegionsData {
    next_id: usize,
    regions: BTreeMap<usize, RegionState>,
    quick_capture: Option<QuickCaptureState>,
}

impl RegionsData {
    fn insert(&mut self, rect: (u32, u32, u32, u32)) -> usize {
        let id = self.next_id;
        self.next_id += 1;
        self.regions
            .insert(id, RegionState::new(rect, format!("Region {id}")));
        id
    }
}

type SharedRegions = Arc<Mutex<RegionsData>>;

/// What the results window's "+ Add Region" / "Quick Capture" / "Show
/// Regions" actions (whether clicked in-window or queued externally via
/// `region_ipc`, see [`LiveRegionApp::update`]) ask `run()`'s main loop to
/// do once the window closes — each needs its own new window in turn
/// (`eframe`/`winit` don't support two `run_native` windows open at once in
/// one process), so the actual work happens here, not inside `update()`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum PendingAction {
    None,
    AddRegion,
    QuickCapture,
    ShowRegions,
}

/// Watches one or more fixed screen regions via a continuous capture
/// session (Linux: portal `ScreenCast` + PipeWire; Windows: `xcap`'s
/// DXGI-based `VideoRecorder` — see `capture::RegionSession`): OCRs each
/// region every `poll_interval_ms`, and re-translates whenever its
/// recognized text changes, independently of the others. Deliberately
/// never touches `history` for the watched regions themselves, matching
/// `live_translate::run` — this is for quick, disposable lookups (subtitles,
/// a status readout, a chat window), not a record of what's been
/// translated. ("Quick Capture" is the one exception — see
/// [`QuickCaptureState`] — it's identical to a manual `capture`, history
/// included, just displayed inline instead of in a separate popup.)
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
    // the main thread also needs a frame each time the user adds a region
    // or asks to see the current ones — `latest_frame` only needs `&self`,
    // so both the watcher thread and this one can hold their own clone
    // freely.
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

    let Some(rect) = crate::capture::select_region_rect(
        &DynamicImage::ImageRgba8(first_frame),
        &cfg.popup,
        &[],
    )?
    else {
        tracing::info!("region selection cancelled");
        return Ok(());
    };

    let regions: SharedRegions = Arc::new(Mutex::new(RegionsData {
        next_id: 1,
        regions: BTreeMap::new(),
        quick_capture: None,
    }));
    regions.lock().unwrap().insert(rect);
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

    // The window closes and reopens each time an action needs a *different*
    // window (adding a region, Quick Capture's own crop selector, the
    // read-only regions preview) rather than staying open throughout —
    // `eframe`/`winit` don't support two `run_native` windows open at once
    // in one process, so this results window and whichever one-off window
    // an action needs take turns instead. The background watcher thread
    // (and the capture session itself) keeps running the whole time
    // regardless, so no translation progress is lost while a window is
    // briefly closed between the two.
    let pending_action = Rc::new(Cell::new(PendingAction::None));
    loop {
        let app = LiveRegionApp {
            regions: regions.clone(),
            show_source: cfg.region_translate.show_source_by_default,
            font_size: cfg.translate.font_size,
            copied_flash: None,
            pending_action: pending_action.clone(),
            renaming_id: None,
            rename_buffer: String::new(),
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

        match pending_action.get() {
            PendingAction::None => break,
            PendingAction::AddRegion => {
                pending_action.set(PendingAction::None);
                let frame = match wait_for_frame(&session, Duration::from_secs(15)) {
                    Ok(f) => f,
                    Err(e) => {
                        tracing::warn!("failed to grab a frame for the new region: {e:#}");
                        continue;
                    }
                };
                let existing = existing_regions(&regions);
                match crate::capture::select_region_rect(
                    &DynamicImage::ImageRgba8(frame),
                    &cfg.popup,
                    &existing,
                ) {
                    Ok(Some(new_rect)) => {
                        regions.lock().unwrap().insert(new_rect);
                    }
                    Ok(None) => tracing::info!("adding a region was cancelled"),
                    Err(e) => tracing::warn!("failed to select a new region: {e:#}"),
                }
            }
            PendingAction::QuickCapture => {
                pending_action.set(PendingAction::None);
                match crate::run_capture_pipeline(cfg) {
                    Ok(Some((text, translated, provider))) => {
                        regions.lock().unwrap().quick_capture = Some(QuickCaptureState {
                            source: text,
                            translated,
                            provider,
                            visible: true,
                        });
                    }
                    Ok(None) => tracing::info!("quick capture cancelled"),
                    Err(e) => tracing::warn!("quick capture failed: {e:#}"),
                }
            }
            PendingAction::ShowRegions => {
                pending_action.set(PendingAction::None);
                match wait_for_frame(&session, Duration::from_secs(15)) {
                    Ok(frame) => {
                        let existing = existing_regions(&regions);
                        if let Err(e) = crate::capture::show_regions(
                            &DynamicImage::ImageRgba8(frame),
                            &cfg.popup,
                            &existing,
                        ) {
                            tracing::warn!("failed to show current regions: {e:#}");
                        }
                    }
                    Err(e) => tracing::warn!("failed to grab a frame to show regions: {e:#}"),
                }
            }
        }
    }
    Ok(())
}

fn existing_regions(regions: &SharedRegions) -> Vec<ExistingRegion> {
    regions
        .lock()
        .unwrap()
        .regions
        .iter()
        .map(|(_, r)| ExistingRegion {
            rect: r.rect,
            name: r.name.clone(),
        })
        .collect()
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
        // needs a frame each time a region is added or shown) is held here
        // for as long as this thread runs; dropping every clone of it
        // would end the capture session, but this thread's own clone alone
        // keeps it alive even after `run()`'s main loop finishes using its
        // copy for a given action.
        let poll_interval = Duration::from_millis(cfg.region_translate.poll_interval_ms.max(100));

        loop {
            std::thread::sleep(poll_interval);
            let Some(frame) = session.latest_frame() else {
                continue;
            };

            // Snapshot the current (id, rect) pairs rather than holding the
            // lock for the whole loop body below (OCR + translation are
            // slow; holding the lock that long would stall the UI thread's
            // own reads for rendering). A region added or deleted mid-poll
            // just gets picked up (or correctly skipped) on the next tick.
            let ids_and_rects: Vec<(usize, (u32, u32, u32, u32))> = regions
                .lock()
                .unwrap()
                .regions
                .iter()
                .map(|(&id, r)| (id, r.rect))
                .collect();

            for (id, (x, y, w, h)) in ids_and_rects {
                let cropped = crop_frame(&frame, x, y, w, h);
                let text = match crate::ocr::recognize(&cropped, &cfg.ocr) {
                    Ok(t) => t,
                    Err(e) => {
                        if let Some(r) = regions.lock().unwrap().regions.get_mut(&id) {
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
                    let Some(r) = guard.regions.get_mut(&id) else {
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
                    translate_and_store(&cfg, &text, &regions, id);
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

fn translate_and_store(cfg: &AppConfig, text: &str, regions: &SharedRegions, id: usize) {
    let result = translate::translate_with_fallback(
        cfg,
        TranslateRequest {
            text,
            source_lang: &cfg.general.source_lang,
            target_lang: &cfg.general.target_lang,
        },
    );
    let mut guard = regions.lock().unwrap();
    let Some(r) = guard.regions.get_mut(&id) else {
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
    /// `None` id (`usize::MAX`, never a real region id) marks the Quick
    /// Capture slot's own copy button.
    copied_flash: Option<(usize, Instant)>,
    /// Set by the "+ Add Region"/"Quick Capture"/"Show Regions" buttons —
    /// or by a queued external `region_ipc::RegionCommand` of the matching
    /// kind, polled every frame below — and read by `run()`'s main loop
    /// after this viewport closes to decide what to do next (or whether to
    /// actually quit).
    pending_action: Rc<Cell<PendingAction>>,
    /// Region id currently being renamed in-place (its row shows a text
    /// edit + Save/Cancel instead of its normal display), and the text
    /// being edited.
    renaming_id: Option<usize>,
    rename_buffer: String,
}

/// Sentinel used with `copied_flash` for the Quick Capture slot's copy
/// button, which isn't a real region id.
const QUICK_CAPTURE_FLASH_ID: usize = usize::MAX;

impl eframe::App for LiveRegionApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }

        // Apply any commands queued externally (`region-capture`,
        // `region-show`, `region-delete`, `region-rename` — see
        // `region_ipc.rs`) since the last frame. Delete/Rename are pure
        // data mutations applied immediately, no window close needed;
        // QuickCapture/ShowRegions need their own window in turn, so they
        // just set `pending_action` and close this one, same as the
        // matching in-window buttons below do.
        for command in crate::region_ipc::drain() {
            match command {
                RegionCommand::Delete { id } => {
                    self.regions.lock().unwrap().regions.remove(&id);
                }
                RegionCommand::Rename { id, name } => {
                    if let Some(r) = self.regions.lock().unwrap().regions.get_mut(&id) {
                        r.name = name;
                    }
                }
                RegionCommand::QuickCapture => {
                    self.pending_action.set(PendingAction::QuickCapture);
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
                RegionCommand::ShowRegions => {
                    self.pending_action.set(PendingAction::ShowRegions);
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
        }
        if self.pending_action.get() != PendingAction::None {
            return;
        }

        if let Some((_, t)) = self.copied_flash {
            if t.elapsed().as_secs_f32() >= 1.5 {
                self.copied_flash = None;
            }
        }

        struct RegionSnapshot {
            id: usize,
            name: String,
            status_label: String,
            source: String,
            translated: String,
        }
        let (quick_capture, snapshot): (
            Option<(String, String, String, bool)>,
            Vec<RegionSnapshot>,
        ) = {
            let guard = self.regions.lock().unwrap();
            let quick = guard.quick_capture.as_ref().map(|q| {
                (
                    q.source.clone(),
                    q.translated.clone(),
                    q.provider.clone(),
                    q.visible,
                )
            });
            let regions = guard
                .regions
                .iter()
                .map(|(&id, r)| RegionSnapshot {
                    id,
                    name: r.name.clone(),
                    status_label: r
                        .status
                        .as_ref()
                        .map(Status::label)
                        .unwrap_or_else(|| Status::Watching.label()),
                    source: r.source.clone(),
                    translated: r.translated.clone(),
                })
                .collect();
            (quick, regions)
        };

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.style_mut().override_font_id = Some(egui::FontId::proportional(self.font_size));

            ui.horizontal(|ui| {
                ui.heading("Live Region Translate");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.checkbox(&mut self.show_source, "Show source");
                    if ui.button("+ Add Region").clicked() {
                        self.pending_action.set(PendingAction::AddRegion);
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                    if ui.button("Quick Capture").clicked() {
                        self.pending_action.set(PendingAction::QuickCapture);
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                    if ui.button("Show Regions").clicked() {
                        self.pending_action.set(PendingAction::ShowRegions);
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                });
            });
            ui.separator();

            egui::ScrollArea::vertical()
                .id_source("regions_scroll")
                .show(ui, |ui| {
                    // Quick Capture always occupies the first row, when
                    // present — toggleable rather than always shown, since
                    // a one-off lookup you're done with shouldn't
                    // permanently take up space above the regions you're
                    // actually watching continuously.
                    if let Some((source, translated, provider, mut visible)) = quick_capture {
                        ui.group(|ui| {
                            ui.set_width(ui.available_width());
                            ui.horizontal(|ui| {
                                ui.strong("Quick Capture");
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        if ui.small_button("Copy").clicked() {
                                            ctx.copy_text(translated.clone());
                                            self.copied_flash =
                                                Some((QUICK_CAPTURE_FLASH_ID, Instant::now()));
                                        }
                                        if matches!(self.copied_flash, Some((id, _)) if id == QUICK_CAPTURE_FLASH_ID)
                                        {
                                            ui.label("Copied!");
                                        }
                                        if ui.checkbox(&mut visible, "Show").changed() {
                                            if let Some(q) =
                                                &mut self.regions.lock().unwrap().quick_capture
                                            {
                                                q.visible = visible;
                                            }
                                        }
                                    },
                                );
                            });
                            if visible {
                                ui.label(egui::RichText::new(format!("via {provider}")).weak());
                                if self.show_source {
                                    ui.label(egui::RichText::new("Source").weak());
                                    ui.add(egui::Label::new(&source).wrap());
                                    ui.add_space(4.0);
                                }
                                ui.label(egui::RichText::new("Translated").weak());
                                ui.add(egui::Label::new(&translated).wrap());
                            }
                        });
                        ui.add_space(6.0);
                    }

                    for region in &snapshot {
                        ui.group(|ui| {
                            ui.set_width(ui.available_width());
                            ui.horizontal(|ui| {
                                if self.renaming_id == Some(region.id) {
                                    ui.text_edit_singleline(&mut self.rename_buffer);
                                    if ui.small_button("Save").clicked() {
                                        if let Some(r) =
                                            self.regions.lock().unwrap().regions.get_mut(&region.id)
                                        {
                                            r.name = self.rename_buffer.clone();
                                        }
                                        self.renaming_id = None;
                                    }
                                    if ui.small_button("Cancel").clicked() {
                                        self.renaming_id = None;
                                    }
                                } else {
                                    ui.strong(&region.name);
                                }
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        if ui.small_button("Delete").clicked() {
                                            self.regions.lock().unwrap().regions.remove(&region.id);
                                        }
                                        if self.renaming_id != Some(region.id)
                                            && ui.small_button("Rename").clicked()
                                        {
                                            self.renaming_id = Some(region.id);
                                            self.rename_buffer = region.name.clone();
                                        }
                                        if ui.small_button("Copy").clicked() {
                                            ctx.copy_text(region.translated.clone());
                                            self.copied_flash = Some((region.id, Instant::now()));
                                        }
                                        if matches!(self.copied_flash, Some((id, _)) if id == region.id)
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
