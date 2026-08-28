use std::sync::OnceLock;

use anyhow::{Context, Result};
use image::DynamicImage;

static LOGO: OnceLock<DynamicImage> = OnceLock::new();

fn logo() -> &'static DynamicImage {
    LOGO.get_or_init(|| {
        image::load_from_memory(include_bytes!("logo.jpeg"))
            .expect("embedded logo.jpeg failed to decode")
    })
}

/// Resizes the embedded app logo to `size x size` RGBA pixels.
fn rgba(size: u32) -> (Vec<u8>, u32, u32) {
    let resized = logo().resize_exact(size, size, image::imageops::FilterType::Lanczos3);
    (resized.to_rgba8().into_raw(), size, size)
}

/// Window icon (title bar / task switcher) for the popup, error, and crop
/// selector windows.
pub fn egui_icon(size: u32) -> egui::IconData {
    let (rgba, width, height) = rgba(size);
    egui::IconData {
        rgba,
        width,
        height,
    }
}

/// Tray icon built from the same logo.
pub fn tray_icon(size: u32) -> Result<tray_icon::Icon> {
    let (rgba, width, height) = rgba(size);
    tray_icon::Icon::from_rgba(rgba, width, height)
        .context("failed to build tray icon from logo.jpeg")
}
