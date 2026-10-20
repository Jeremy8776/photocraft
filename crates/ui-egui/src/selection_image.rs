//! Bounded colour-managed capture and preview decoding for local selection models.
use super::*;
use photocraft_cms::{Builtin, Intent, Transform};

pub(super) fn preview_image(jpeg: &[u8]) -> Result<egui::ColorImage, String> {
    if jpeg.len() > 16 * 1024 * 1024 {
        return Err("SAM 3 preview is too large".into());
    }
    let image = photocraft_codecs::decode_as(Format::Jpeg, jpeg).map_err(|_| "SAM 3 preview could not be decoded".to_string())?;
    let size = [image.width() as usize, image.height() as usize];
    if size[0] == 0 || size[1] == 0 || size[0].saturating_mul(size[1]) > 16_777_216 || image.sample_type() != photocraft_codecs::SampleType::U8 {
        return Err("SAM 3 preview has unsupported dimensions".into());
    }
    let bytes = image.as_u8().ok_or_else(|| "SAM 3 preview has no pixels".to_string())?;
    match image.layout() {
        ChannelLayout::Rgb if bytes.len() == size[0] * size[1] * 3 => Ok(egui::ColorImage::from_rgb(size, bytes)),
        ChannelLayout::Gray if bytes.len() == size[0] * size[1] => {
            let rgb: Vec<_> = bytes.iter().flat_map(|v| [*v; 3]).collect();
            Ok(egui::ColorImage::from_rgb(size, &rgb))
        }
        _ => Err("SAM 3 preview has unsupported colours".into()),
    }
}

const MODEL_SIDE: u32 = 1536;

/// Banded composite reduction avoids holding a full 36 MP render on the UI thread.
pub(super) fn source_png(doc: &Document) -> Result<(Vec<u8>, [u32; 2]), String> {
    let (w, h) = (doc.size.width, doc.size.height);
    if w == 0 || h == 0 || u64::from(w) * u64::from(h) > 40_000_000 {
        return Err("This document is too large for Object Selection".into());
    }
    let longest = w.max(h);
    let (mw, mh) = if longest <= MODEL_SIDE {
        (w, h)
    } else {
        ((u64::from(w) * u64::from(MODEL_SIDE) / u64::from(longest)).max(1) as u32, (u64::from(h) * u64::from(MODEL_SIDE) / u64::from(longest)).max(1) as u32)
    };
    let mut buffer = photocraft_compose::render_reduced(doc, mw, mh).to_rgba8();
    let profile = photocraft_engine::color_cmds::composite_profile(doc);
    let transform = Transform::new(&profile, Builtin::Srgb.profile(), Intent::RelativeColorimetric, true)
        .map_err(|_| "Could not convert the document colours for Object Selection".to_string())?;
    for chunk in buffer.pixels.chunks_mut(1024 * 1024) {
        let source = chunk.to_vec();
        transform.convert_u8(&source, 4, chunk, 4, true);
    }
    let image = Image::from_u8(mw, mh, ChannelLayout::Rgba, buffer.pixels).map_err(|_| "Could not capture the document for Object Selection".to_string())?;
    let png =
        photocraft_codecs::encode(&image, Format::Png, &EncodeOptions::default()).map_err(|_| "Could not encode the Object Selection image".to_string())?;
    Ok((png, [mw, mh]))
}

pub(super) fn model_point(doc: [u32; 2], model: [u32; 2], point: [f64; 2]) -> Option<(f32, f32)> {
    let [w, h] = doc;
    let [mw, mh] = model;
    if w == 0
        || h == 0
        || mw == 0
        || mh == 0
        || !point.iter().all(|v| v.is_finite())
        || point[0] < 0.0
        || point[1] < 0.0
        || point[0] >= f64::from(w)
        || point[1] >= f64::from(h)
    {
        return None;
    }
    Some(((point[0] * f64::from(mw) / f64::from(w)) as f32, (point[1] * f64::from(mh) / f64::from(h)) as f32))
}
