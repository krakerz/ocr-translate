use anyhow::{Context, Result};
use image::DynamicImage;
use leptess::{LepTess, Variable};

use crate::config::OcrConfig;

/// Runs Tesseract OCR over a captured region and returns the recognized text, trimmed.
pub fn recognize(image: &DynamicImage, cfg: &OcrConfig) -> Result<String> {
    let processed = if cfg.preprocess {
        preprocess(image)
    } else {
        image.clone()
    };

    let mut lt = LepTess::new(cfg.tessdata_dir.as_deref(), &cfg.languages).context(
        "failed to initialize Tesseract (is `tessdata` for the configured language installed?)",
    )?;

    if let Some(psm) = cfg.psm {
        lt.set_variable(Variable::TesseditPagesegMode, &psm.to_string())
            .context("failed to set Tesseract page segmentation mode")?;
    }

    let rgb = processed.to_rgb8();
    lt.set_image_from_mem(&encode_png(&rgb)?)
        .context("failed to hand the captured image to Tesseract")?;

    let text = lt.get_utf8_text().context("Tesseract OCR failed")?;
    Ok(text.trim().to_string())
}

fn encode_png(rgb: &image::RgbImage) -> Result<Vec<u8>> {
    let mut buf = Vec::new();
    let mut cursor = std::io::Cursor::new(&mut buf);
    rgb.write_to(&mut cursor, image::ImageFormat::Png)?;
    Ok(buf)
}

/// Grayscale + contrast stretch, which measurably helps Tesseract on UI
/// screenshots (anti-aliased small text, subtitle overlays, etc.) versus raw RGB.
fn preprocess(image: &DynamicImage) -> DynamicImage {
    let gray = image.grayscale();
    DynamicImage::ImageLuma8(imageproc_contrast(&gray.to_luma8()))
}

fn imageproc_contrast(img: &image::GrayImage) -> image::GrayImage {
    let (min, max) = img
        .pixels()
        .fold((255u8, 0u8), |(mn, mx), p| (mn.min(p.0[0]), mx.max(p.0[0])));
    if max <= min {
        return img.clone();
    }
    let range = (max - min) as f32;
    image::ImageBuffer::from_fn(img.width(), img.height(), |x, y| {
        let v = img.get_pixel(x, y).0[0];
        let stretched = ((v.saturating_sub(min)) as f32 / range * 255.0).round() as u8;
        image::Luma([stretched])
    })
}
