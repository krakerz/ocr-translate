use std::path::PathBuf;
use std::sync::OnceLock;

/// Adds a CJK-capable fallback font to `ctx`'s font families, so Japanese/
/// Chinese/Korean text (very much the point of this app) doesn't render as
/// tofu boxes — egui's bundled default fonts only cover Latin script.
///
/// Rather than embedding a CJK font (a full-coverage one like Noto Sans CJK
/// is ~20MB, since it needs thousands of Han glyphs), this looks up whatever
/// CJK-capable font is already installed via fontconfig (`fc-list :lang=ja`)
/// and loads that file at runtime. Anyone doing Japanese OCR already has a
/// real CJK font installed system-wide — this just tells egui about it. If
/// none is found (checked via `fc-list`, not `fc-match`, since the latter
/// always returns *some* font even with no CJK support at all), this is a
/// no-op and text falls back to egui's default appearance (boxes).
///
/// Call once per process, before showing any window that might display
/// non-Latin text — every `eframe::run_native` closure in this crate does.
pub fn install_cjk_fallback(ctx: &egui::Context) {
    static FONT_BYTES: OnceLock<Option<Vec<u8>>> = OnceLock::new();
    let Some(bytes) = FONT_BYTES.get_or_init(load_cjk_font_bytes) else {
        tracing::debug!(
            "no system CJK font found (checked `fc-list :lang=ja`); \
             Japanese/Chinese/Korean text may render as boxes"
        );
        return;
    };

    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "cjk-fallback".to_owned(),
        egui::FontData::from_owned(bytes.clone()),
    );
    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        fonts
            .families
            .entry(family)
            .or_default()
            .push("cjk-fallback".to_owned());
    }
    ctx.set_fonts(fonts);
}

fn load_cjk_font_bytes() -> Option<Vec<u8>> {
    let path = find_cjk_font_path()?;
    match std::fs::read(&path) {
        Ok(bytes) => {
            tracing::debug!("using {} as the CJK fallback font", path.display());
            Some(bytes)
        }
        Err(e) => {
            tracing::warn!(
                "found a CJK font at {} but failed to read it: {e}",
                path.display()
            );
            None
        }
    }
}

/// Asks fontconfig for every font that actually declares Japanese glyph
/// coverage (`fc-list :lang=ja`, not `fc-match`, which would happily return
/// an unrelated font even with zero CJK support) and picks the most
/// generically-suitable one: a "Sans" family, preferring a "Regular" weight,
/// falling back to whatever was found first.
fn find_cjk_font_path() -> Option<PathBuf> {
    let output = std::process::Command::new("fc-list")
        .args([":lang=ja", "file"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let candidates: Vec<&str> = stdout
        .lines()
        .filter_map(|line| {
            let path = line.trim().trim_end_matches(':').trim();
            (!path.is_empty()).then_some(path)
        })
        .collect();

    let pick = candidates
        .iter()
        .find(|p| p.contains("Sans") && p.contains("Regular"))
        .or_else(|| candidates.iter().find(|p| p.contains("Sans")))
        .or_else(|| candidates.first())
        .copied()?;
    Some(PathBuf::from(pick))
}
