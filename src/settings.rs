use anyhow::Result;

use crate::config::{
    AppConfig, CaptureBackend, ProviderConfig, ProviderKind, ProviderMode,
};

/// Opens the Settings window (`ocr-translate configure`, tray "Settings..."):
/// an editable form over every `AppConfig` field, so a user never has to
/// hand-edit `config.yaml`/`config.conf` for day-to-day tweaks (providers,
/// languages, window sizes, ...). Like every other window-showing subcommand
/// in this project (`capture`, `watch-clipboard`, ...), this runs in its own
/// process rather than the long-running tray daemon — see the process-model
/// note in `daemon.rs::run` for why. `config_path` is the `--config` the
/// daemon/tray itself was started with, if any, so Save writes back to the
/// same file the user actually pointed at rather than always the default
/// per-user location.
pub fn run(cfg: &AppConfig, config_path: Option<&std::path::Path>) -> Result<()> {
    let app = SettingsApp::new(cfg, config_path.map(|p| p.to_path_buf()));

    let (width, height) = crate::capture::clamp_to_screen(720.0, 780.0);
    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([width, height])
            .with_always_on_top()
            .with_icon(crate::icon::egui_icon(128))
            .with_title("ocr-translate: Settings"),
        ..Default::default()
    };
    eframe::run_native(
        "ocr-translate-settings",
        native_options,
        Box::new(|cc| {
            crate::fonts::install_cjk_fallback(&cc.egui_ctx);
            Ok(Box::new(app))
        }),
    )
    .map_err(|e| anyhow::anyhow!("settings window failed: {e}"))?;
    Ok(())
}

/// A single provider entry as edited in the form. Kept as a `Vec` (not the
/// `HashMap` `AppConfig` itself uses) so entries have a stable on-screen
/// order and the provider's key/name is itself an editable text field (a
/// `HashMap`'s keys can't be edited in place) — rebuilt into a `HashMap` only
/// when saving.
struct ProviderRow {
    name: String,
    cfg: ProviderConfig,
}

/// A save/discard confirmation flash shown next to the Save button, same
/// pattern as `popup::PopupApp`'s "Copied!" flash.
enum SaveStatus {
    Saved(std::time::Instant),
    Error(String),
}

struct SettingsApp {
    config_path: Option<std::path::PathBuf>,

    // general
    target_lang: String,
    source_lang: String,
    log_level: String,

    // providers
    active_provider: String,
    fallback_providers: String,
    providers: Vec<ProviderRow>,

    // ocr
    ocr_languages: String,
    ocr_tessdata_dir: String,
    ocr_psm_enabled: bool,
    ocr_psm: i32,
    ocr_preprocess: bool,

    // capture
    capture_backend: CaptureBackend,
    capture_external_command: String,
    capture_external_timeout_secs: u64,

    // prompt
    prompt_system: String,
    prompt_template: String,

    // windows
    popup_width: f32,
    popup_height: f32,
    popup_always_on_top: bool,
    translate_width: f32,
    translate_height: f32,
    translate_font_size: f32,
    translate_always_on_top: bool,
    translate_auto_close_secs: u64,
    history_popup_width: f32,
    history_popup_height: f32,
    history_popup_font_size: f32,
    history_popup_always_on_top: bool,
    history_popup_auto_close_secs: u64,

    // history
    history_enabled: bool,
    history_max_entries: usize,
    history_tray_menu_entries: usize,

    // live translate / live region
    live_translate_show_source: bool,
    live_translate_poll_interval_ms: u64,
    region_translate_show_source: bool,
    region_translate_poll_interval_ms: u64,
    region_translate_capture_delay_secs: u64,

    status: Option<SaveStatus>,
}

impl SettingsApp {
    fn new(cfg: &AppConfig, config_path: Option<std::path::PathBuf>) -> Self {
        let mut providers: Vec<ProviderRow> = cfg
            .providers
            .iter()
            .map(|(name, p)| ProviderRow {
                name: name.clone(),
                cfg: p.clone(),
            })
            .collect();
        providers.sort_by(|a, b| a.name.cmp(&b.name));

        Self {
            config_path,
            target_lang: cfg.general.target_lang.clone(),
            source_lang: cfg.general.source_lang.clone(),
            log_level: cfg.general.log_level.clone(),

            active_provider: cfg.active_provider.clone(),
            fallback_providers: cfg.fallback_providers.join(", "),
            providers,

            ocr_languages: cfg.ocr.languages.clone(),
            ocr_tessdata_dir: cfg.ocr.tessdata_dir.clone().unwrap_or_default(),
            ocr_psm_enabled: cfg.ocr.psm.is_some(),
            ocr_psm: cfg.ocr.psm.unwrap_or(6),
            ocr_preprocess: cfg.ocr.preprocess,

            capture_backend: cfg.capture.backend,
            capture_external_command: cfg.capture.external_command.clone(),
            capture_external_timeout_secs: cfg.capture.external_timeout_secs,

            prompt_system: cfg.prompt.system.clone(),
            prompt_template: cfg.prompt.template.clone(),

            popup_width: cfg.popup.width,
            popup_height: cfg.popup.height,
            popup_always_on_top: cfg.popup.always_on_top,
            translate_width: cfg.translate.width,
            translate_height: cfg.translate.height,
            translate_font_size: cfg.translate.font_size,
            translate_always_on_top: cfg.translate.always_on_top,
            translate_auto_close_secs: cfg.translate.auto_close_secs,
            history_popup_width: cfg.history_popup.width,
            history_popup_height: cfg.history_popup.height,
            history_popup_font_size: cfg.history_popup.font_size,
            history_popup_always_on_top: cfg.history_popup.always_on_top,
            history_popup_auto_close_secs: cfg.history_popup.auto_close_secs,

            history_enabled: cfg.history.enabled,
            history_max_entries: cfg.history.max_entries,
            history_tray_menu_entries: cfg.history.tray_menu_entries,

            live_translate_show_source: cfg.live_translate.show_source_by_default,
            live_translate_poll_interval_ms: cfg.live_translate.poll_interval_ms,
            region_translate_show_source: cfg.region_translate.show_source_by_default,
            region_translate_poll_interval_ms: cfg.region_translate.poll_interval_ms,
            region_translate_capture_delay_secs: cfg.region_translate.capture_delay_secs,

            status: None,
        }
    }

    /// Rebuilds a full `AppConfig` from the form fields and writes it to
    /// disk. Duplicate provider names collapse to whichever entry appears
    /// last in the list (matches `HashMap::insert`'s own overwrite
    /// semantics, so this isn't a new rule the user has to learn).
    fn save(&mut self) {
        let mut providers = std::collections::HashMap::new();
        for row in &self.providers {
            let key = row.name.trim();
            if key.is_empty() {
                continue;
            }
            providers.insert(key.to_string(), row.cfg.clone());
        }

        let cfg = AppConfig {
            general: crate::config::GeneralConfig {
                target_lang: self.target_lang.trim().to_string(),
                source_lang: self.source_lang.trim().to_string(),
                log_level: self.log_level.trim().to_string(),
            },
            ocr: crate::config::OcrConfig {
                languages: self.ocr_languages.trim().to_string(),
                tessdata_dir: non_empty(&self.ocr_tessdata_dir),
                psm: self.ocr_psm_enabled.then_some(self.ocr_psm),
                preprocess: self.ocr_preprocess,
            },
            capture: crate::config::CaptureConfig {
                backend: self.capture_backend,
                external_command: self.capture_external_command.clone(),
                external_timeout_secs: self.capture_external_timeout_secs,
            },
            popup: crate::config::CaptureWindowConfig {
                width: self.popup_width,
                height: self.popup_height,
                always_on_top: self.popup_always_on_top,
            },
            translate: crate::config::PopupConfig {
                width: self.translate_width,
                height: self.translate_height,
                font_size: self.translate_font_size,
                always_on_top: self.translate_always_on_top,
                auto_close_secs: self.translate_auto_close_secs,
            },
            history_popup: crate::config::PopupConfig {
                width: self.history_popup_width,
                height: self.history_popup_height,
                font_size: self.history_popup_font_size,
                always_on_top: self.history_popup_always_on_top,
                auto_close_secs: self.history_popup_auto_close_secs,
            },
            history: crate::config::HistoryConfig {
                enabled: self.history_enabled,
                max_entries: self.history_max_entries,
                tray_menu_entries: self.history_tray_menu_entries,
            },
            live_translate: crate::config::LiveTranslateConfig {
                show_source_by_default: self.live_translate_show_source,
                poll_interval_ms: self.live_translate_poll_interval_ms,
            },
            region_translate: crate::config::RegionTranslateConfig {
                show_source_by_default: self.region_translate_show_source,
                poll_interval_ms: self.region_translate_poll_interval_ms,
                capture_delay_secs: self.region_translate_capture_delay_secs,
            },
            prompt: crate::config::PromptConfig {
                system: self.prompt_system.clone(),
                template: self.prompt_template.clone(),
            },
            active_provider: self.active_provider.trim().to_string(),
            fallback_providers: self
                .fallback_providers
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect(),
            providers,
        };

        let result = crate::config::save_target_path(self.config_path.as_deref())
            .and_then(|path| crate::config::save(&cfg, &path).map(|_| path));
        self.status = Some(match result {
            Ok(_) => SaveStatus::Saved(std::time::Instant::now()),
            Err(e) => SaveStatus::Error(format!("{e:#}")),
        });
    }
}

/// A single-line text edit that fills whatever width is available to it
/// (`ui.text_edit_singleline` uses egui's fixed default width instead, which
/// left a growing gap on the right as the window was resized wider — every
/// field in this form should track the window's actual width).
fn stretchy_text_edit(ui: &mut egui::Ui, text: &mut String) -> egui::Response {
    ui.add(egui::TextEdit::singleline(text).desired_width(f32::INFINITY))
}

/// Rows to give a multiline prompt box so it grows with its own content
/// instead of a fixed 3 rows clipping a longer prompt (or wasting space on a
/// short one) — counts actual newlines in the text, not wrapped display
/// lines (egui has no cheap way to query those before layout), clamped to a
/// sane range so one huge prompt can't blow up the whole window.
fn auto_rows(text: &str) -> usize {
    (text.lines().count() + 1).clamp(3, 12)
}

fn non_empty(s: &str) -> Option<String> {
    let trimmed = s.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

impl eframe::App for SettingsApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }

        egui::TopBottomPanel::bottom("settings_bottom").show(ctx, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui.button("Save").clicked() {
                    self.save();
                }
                if ui.button("Close").clicked() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
                match &self.status {
                    Some(SaveStatus::Saved(t)) if t.elapsed().as_secs_f32() < 2.5 => {
                        ui.colored_label(egui::Color32::from_rgb(90, 170, 90), "Saved.");
                    }
                    Some(SaveStatus::Error(e)) => {
                        ui.colored_label(egui::Color32::from_rgb(220, 80, 80), e);
                    }
                    _ => {}
                }
            });
            ui.add_space(6.0);
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("Settings");
            ui.label(
                egui::RichText::new(
                    "Changes take effect on the next capture/tray refresh — no restart needed.",
                )
                .weak(),
            );
            ui.separator();

            egui::ScrollArea::vertical().show(ui, |ui| {
                egui::CollapsingHeader::new("General")
                    .default_open(true)
                    .show(ui, |ui| {
                        egui::Grid::new("general_grid")
                            .num_columns(2)
                            .show(ui, |ui| {
                                ui.label("Target language");
                                stretchy_text_edit(ui, &mut self.target_lang);
                                ui.end_row();

                                ui.label("Source language");
                                stretchy_text_edit(ui, &mut self.source_lang);
                                ui.end_row();

                                ui.label("Log level");
                                egui::ComboBox::from_id_source("log_level")
                                    .selected_text(&self.log_level)
                                    .show_ui(ui, |ui| {
                                        for lvl in ["trace", "debug", "info", "warn", "error"] {
                                            ui.selectable_value(
                                                &mut self.log_level,
                                                lvl.to_string(),
                                                lvl,
                                            );
                                        }
                                    });
                                ui.end_row();
                            });
                    });

                egui::CollapsingHeader::new("Providers")
                    .default_open(true)
                    .show(ui, |ui| {
                        egui::Grid::new("providers_top_grid")
                            .num_columns(2)
                            .show(ui, |ui| {
                                ui.label("Active provider");
                                stretchy_text_edit(ui, &mut self.active_provider);
                                ui.end_row();

                                ui.label("Fallback providers");
                                stretchy_text_edit(ui, &mut self.fallback_providers);
                                ui.end_row();
                            });
                        ui.label(
                            egui::RichText::new(
                                "Comma-separated provider names, tried in order if the active one fails.",
                            )
                            .weak()
                            .small(),
                        );
                        ui.add_space(8.0);

                        let mut remove_index = None;
                        for (i, row) in self.providers.iter_mut().enumerate() {
                            ui.push_id(i, |ui| {
                                egui::Frame::group(ui.style()).show(ui, |ui| {
                                    egui::Grid::new("provider_grid")
                                        .num_columns(2)
                                        .show(ui, |ui| {
                                            ui.label("Name");
                                            ui.horizontal(|ui| {
                                                // `with_layout(right_to_left)` was tried here
                                                // first so "Remove" would claim its space before
                                                // the name field — but nested layouts inside an
                                                // `egui::Grid` cell confuse its column-width
                                                // tracking (confirmed by testing: the field grew
                                                // to the *entire* row and pushed "Remove"
                                                // offscreen). Reserving a concrete width for the
                                                // button up front and sizing the field to what's
                                                // left, both in the default left-to-right layout,
                                                // avoids that.
                                                const REMOVE_BTN_WIDTH: f32 = 64.0;
                                                let field_width = (ui.available_width()
                                                    - REMOVE_BTN_WIDTH
                                                    - ui.spacing().item_spacing.x)
                                                    .max(40.0);
                                                ui.add_sized(
                                                    [field_width, ui.spacing().interact_size.y],
                                                    egui::TextEdit::singleline(&mut row.name),
                                                );
                                                if ui.button("Remove").clicked() {
                                                    remove_index = Some(i);
                                                }
                                            });
                                            ui.end_row();

                                            ui.label("Kind");
                                            egui::ComboBox::from_id_source("kind")
                                                .selected_text(provider_kind_label(row.cfg.kind))
                                                .show_ui(ui, |ui| {
                                                    for kind in [
                                                        ProviderKind::OpenAiCompatible,
                                                        ProviderKind::GoogleTranslate,
                                                        ProviderKind::BingTranslate,
                                                        ProviderKind::DeepLTranslate,
                                                    ] {
                                                        ui.selectable_value(
                                                            &mut row.cfg.kind,
                                                            kind,
                                                            provider_kind_label(kind),
                                                        );
                                                    }
                                                });
                                            ui.end_row();

                                            ui.label("Mode");
                                            egui::ComboBox::from_id_source("mode")
                                                .selected_text(match row.cfg.mode {
                                                    ProviderMode::Public => "public",
                                                    ProviderMode::Private => "private",
                                                })
                                                .show_ui(ui, |ui| {
                                                    ui.selectable_value(
                                                        &mut row.cfg.mode,
                                                        ProviderMode::Public,
                                                        "public",
                                                    );
                                                    ui.selectable_value(
                                                        &mut row.cfg.mode,
                                                        ProviderMode::Private,
                                                        "private",
                                                    );
                                                });
                                            ui.end_row();

                                            ui.label("Base URL");
                                            let mut base_url =
                                                row.cfg.base_url.clone().unwrap_or_default();
                                            if stretchy_text_edit(ui, &mut base_url).changed() {
                                                row.cfg.base_url = non_empty(&base_url);
                                            }
                                            ui.end_row();

                                            ui.label("Model");
                                            let mut model =
                                                row.cfg.model.clone().unwrap_or_default();
                                            if stretchy_text_edit(ui, &mut model).changed() {
                                                row.cfg.model = non_empty(&model);
                                            }
                                            ui.end_row();

                                            ui.label("API key");
                                            let mut api_key =
                                                row.cfg.api_key.clone().unwrap_or_default();
                                            if ui
                                                .add(
                                                    egui::TextEdit::singleline(&mut api_key)
                                                        .password(true)
                                                        .desired_width(f32::INFINITY),
                                                )
                                                .changed()
                                            {
                                                row.cfg.api_key = non_empty(&api_key);
                                            }
                                            ui.end_row();

                                            ui.label("API key env var");
                                            let mut api_key_env =
                                                row.cfg.api_key_env.clone().unwrap_or_default();
                                            if stretchy_text_edit(ui, &mut api_key_env).changed() {
                                                row.cfg.api_key_env = non_empty(&api_key_env);
                                            }
                                            ui.end_row();

                                            ui.label("Region (Bing/Azure)");
                                            let mut region =
                                                row.cfg.region.clone().unwrap_or_default();
                                            if stretchy_text_edit(ui, &mut region).changed() {
                                                row.cfg.region = non_empty(&region);
                                            }
                                            ui.end_row();

                                            ui.label("Timeout (seconds)");
                                            ui.add(egui::DragValue::new(
                                                &mut row.cfg.timeout_secs,
                                            ));
                                            ui.end_row();
                                        });
                                });
                            });
                            ui.add_space(4.0);
                        }
                        if let Some(i) = remove_index {
                            self.providers.remove(i);
                        }

                        if ui.button("+ Add provider").clicked() {
                            self.providers.push(ProviderRow {
                                name: format!("provider{}", self.providers.len() + 1),
                                cfg: ProviderConfig::default(),
                            });
                        }
                    });

                egui::CollapsingHeader::new("OCR")
                    .default_open(false)
                    .show(ui, |ui| {
                        egui::Grid::new("ocr_grid").num_columns(2).show(ui, |ui| {
                            ui.label("Languages");
                            stretchy_text_edit(ui, &mut self.ocr_languages);
                            ui.end_row();

                            ui.label("Tessdata directory");
                            stretchy_text_edit(ui, &mut self.ocr_tessdata_dir);
                            ui.end_row();

                            ui.label("Page segmentation mode");
                            ui.horizontal(|ui| {
                                ui.checkbox(&mut self.ocr_psm_enabled, "");
                                ui.add_enabled(
                                    self.ocr_psm_enabled,
                                    egui::DragValue::new(&mut self.ocr_psm).range(0..=13),
                                );
                            });
                            ui.end_row();

                            ui.label("Preprocess (grayscale + contrast)");
                            ui.checkbox(&mut self.ocr_preprocess, "");
                            ui.end_row();
                        });
                    });

                egui::CollapsingHeader::new("Capture")
                    .default_open(false)
                    .show(ui, |ui| {
                        egui::Grid::new("capture_grid")
                            .num_columns(2)
                            .show(ui, |ui| {
                                ui.label("Backend");
                                egui::ComboBox::from_id_source("capture_backend")
                                    .selected_text(match self.capture_backend {
                                        CaptureBackend::BuiltIn => "built_in",
                                        CaptureBackend::External => "external",
                                    })
                                    .show_ui(ui, |ui| {
                                        ui.selectable_value(
                                            &mut self.capture_backend,
                                            CaptureBackend::BuiltIn,
                                            "built_in",
                                        );
                                        ui.selectable_value(
                                            &mut self.capture_backend,
                                            CaptureBackend::External,
                                            "external",
                                        );
                                    });
                                ui.end_row();

                                ui.label("External command");
                                stretchy_text_edit(ui, &mut self.capture_external_command);
                                ui.end_row();

                                ui.label("External timeout (seconds)");
                                ui.add(egui::DragValue::new(
                                    &mut self.capture_external_timeout_secs,
                                ));
                                ui.end_row();
                            });
                    });

                egui::CollapsingHeader::new("Prompt")
                    .default_open(false)
                    .show(ui, |ui| {
                        ui.label("System prompt");
                        let system_rows = auto_rows(&self.prompt_system);
                        ui.add(
                            egui::TextEdit::multiline(&mut self.prompt_system)
                                .desired_rows(system_rows)
                                .desired_width(f32::INFINITY),
                        );
                        ui.add_space(6.0);
                        ui.label("Template (placeholders: {source_lang} {target_lang} {text})");
                        let template_rows = auto_rows(&self.prompt_template);
                        ui.add(
                            egui::TextEdit::multiline(&mut self.prompt_template)
                                .desired_rows(template_rows)
                                .desired_width(f32::INFINITY),
                        );
                    });

                egui::CollapsingHeader::new("Windows")
                    .default_open(false)
                    .show(ui, |ui| {
                        ui.label(egui::RichText::new("Crop-selector window").strong());
                        egui::Grid::new("popup_grid").num_columns(2).show(ui, |ui| {
                            ui.label("Width");
                            ui.add(egui::DragValue::new(&mut self.popup_width));
                            ui.end_row();
                            ui.label("Height");
                            ui.add(egui::DragValue::new(&mut self.popup_height));
                            ui.end_row();
                            ui.label("Always on top");
                            ui.checkbox(&mut self.popup_always_on_top, "");
                            ui.end_row();
                        });

                        ui.add_space(8.0);
                        ui.label(egui::RichText::new("Translation result window").strong());
                        egui::Grid::new("translate_grid")
                            .num_columns(2)
                            .show(ui, |ui| {
                                ui.label("Width");
                                ui.add(egui::DragValue::new(&mut self.translate_width));
                                ui.end_row();
                                ui.label("Height");
                                ui.add(egui::DragValue::new(&mut self.translate_height));
                                ui.end_row();
                                ui.label("Font size");
                                ui.add(egui::DragValue::new(&mut self.translate_font_size));
                                ui.end_row();
                                ui.label("Always on top");
                                ui.checkbox(&mut self.translate_always_on_top, "");
                                ui.end_row();
                                ui.label("Auto-close after (seconds, 0 = never)");
                                ui.add(egui::DragValue::new(&mut self.translate_auto_close_secs));
                                ui.end_row();
                            });

                        ui.add_space(8.0);
                        ui.label(egui::RichText::new("History popup window").strong());
                        egui::Grid::new("history_popup_grid")
                            .num_columns(2)
                            .show(ui, |ui| {
                                ui.label("Width");
                                ui.add(egui::DragValue::new(&mut self.history_popup_width));
                                ui.end_row();
                                ui.label("Height");
                                ui.add(egui::DragValue::new(&mut self.history_popup_height));
                                ui.end_row();
                                ui.label("Font size");
                                ui.add(egui::DragValue::new(&mut self.history_popup_font_size));
                                ui.end_row();
                                ui.label("Always on top");
                                ui.checkbox(&mut self.history_popup_always_on_top, "");
                                ui.end_row();
                                ui.label("Auto-close after (seconds, 0 = never)");
                                ui.add(egui::DragValue::new(
                                    &mut self.history_popup_auto_close_secs,
                                ));
                                ui.end_row();
                            });
                    });

                egui::CollapsingHeader::new("History")
                    .default_open(false)
                    .show(ui, |ui| {
                        egui::Grid::new("history_grid")
                            .num_columns(2)
                            .show(ui, |ui| {
                                ui.label("Record history");
                                ui.checkbox(&mut self.history_enabled, "");
                                ui.end_row();
                                ui.label("Max entries kept");
                                ui.add(egui::DragValue::new(&mut self.history_max_entries));
                                ui.end_row();
                                ui.label("Entries shown in tray menu");
                                ui.add(egui::DragValue::new(
                                    &mut self.history_tray_menu_entries,
                                ));
                                ui.end_row();
                            });
                    });

                egui::CollapsingHeader::new("Live Clipboard Translate")
                    .default_open(false)
                    .show(ui, |ui| {
                        egui::Grid::new("live_translate_grid")
                            .num_columns(2)
                            .show(ui, |ui| {
                                ui.label("Show source by default");
                                ui.checkbox(&mut self.live_translate_show_source, "");
                                ui.end_row();
                                ui.label("Poll interval (ms)");
                                ui.add(egui::DragValue::new(
                                    &mut self.live_translate_poll_interval_ms,
                                ));
                                ui.end_row();
                            });
                    });

                egui::CollapsingHeader::new("Live Region Translate")
                    .default_open(false)
                    .show(ui, |ui| {
                        egui::Grid::new("region_translate_grid")
                            .num_columns(2)
                            .show(ui, |ui| {
                                ui.label("Show source by default");
                                ui.checkbox(&mut self.region_translate_show_source, "");
                                ui.end_row();
                                ui.label("Poll interval (ms)");
                                ui.add(egui::DragValue::new(
                                    &mut self.region_translate_poll_interval_ms,
                                ));
                                ui.end_row();
                                ui.label("Capture delay (seconds)");
                                ui.add(egui::DragValue::new(
                                    &mut self.region_translate_capture_delay_secs,
                                ));
                                ui.end_row();
                            });
                    });

                ui.add_space(20.0);
            });
        });
    }
}

fn provider_kind_label(kind: ProviderKind) -> &'static str {
    match kind {
        ProviderKind::OpenAiCompatible => "openai_compatible",
        ProviderKind::GoogleTranslate => "google_translate",
        ProviderKind::BingTranslate => "bing_translate",
        ProviderKind::DeepLTranslate => "deepl_translate",
    }
}
