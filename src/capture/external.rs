use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use image::DynamicImage;

use crate::config::CaptureConfig;

type ClipboardImage = (usize, usize, Vec<u8>);

fn snapshot(clipboard: &mut arboard::Clipboard) -> Option<ClipboardImage> {
    let image = clipboard.get_image().ok()?;
    Some((image.width, image.height, image.bytes.into_owned()))
}

/// Runs the configured external screenshot command (expected to let the user
/// interactively select a region on the real desktop, e.g. KDE's `spectacle
/// -r -b -c`, and place a PNG image on the clipboard when done), then reads
/// that image back from the clipboard.
///
/// Records the clipboard's image (if any) before running the command so a
/// stale, unrelated image already sitting on the clipboard isn't mistaken
/// for a fresh capture — the same guard `vn_ocr.sh` used a hash-polling loop
/// for. Returns `None` if the tool exits without ever producing a new image
/// (e.g. the user cancelled the selection). Note this means capturing the
/// exact same pixels twice in a row reads as "cancelled" the second time,
/// same tradeoff `vn_ocr.sh` made — not a concern in practice for OCR use.
pub fn capture(cfg: &CaptureConfig) -> Result<Option<DynamicImage>> {
    let mut clipboard = arboard::Clipboard::new().context("failed to open the clipboard")?;
    let previous = snapshot(&mut clipboard);

    let status = std::process::Command::new("sh")
        .arg("-c")
        .arg(&cfg.external_command)
        .status()
        .with_context(|| format!("failed to run capture command: {}", cfg.external_command))?;
    if !status.success() {
        tracing::debug!("external capture command exited with {status}");
    }

    let deadline = Instant::now() + Duration::from_secs(cfg.external_timeout_secs);
    loop {
        if let Some(current) = snapshot(&mut clipboard) {
            if previous.as_ref() != Some(&current) {
                let (width, height, bytes) = current;
                let rgba = image::RgbaImage::from_raw(width as u32, height as u32, bytes)
                    .context("clipboard image had an unexpected size")?;
                return Ok(Some(DynamicImage::ImageRgba8(rgba)));
            }
        }
        if Instant::now() >= deadline {
            return Ok(None);
        }
        std::thread::sleep(Duration::from_millis(150));
    }
}
