//! Text-selection orchestration for the prepared model. Masks are combined before the
//! single undoable engine command so all selection modes act on the same union.

use photocraft_codecs::{ChannelLayout, DecodeOptions, EncodeOptions, Format, Image, Limits, SampleType};

const MAX_PHRASES: usize = 32;
const MAX_PHRASE_BYTES: usize = 200;

pub(super) fn ground_phrases(text: &str, mut ground: impl FnMut(&str) -> Result<Vec<u8>, String>) -> Result<(String, Vec<u8>), String> {
    if text.len() > MAX_PHRASES * (MAX_PHRASE_BYTES + 1) {
        return Err("The text selection prompt is too long".into());
    }
    let mut phrases = Vec::new();
    for phrase in text.split(',').map(str::trim).filter(|phrase| !phrase.is_empty()) {
        if phrase.len() > MAX_PHRASE_BYTES {
            return Err("Each selection phrase must be at most 200 bytes".into());
        }
        if !phrases.contains(&phrase) {
            phrases.push(phrase);
        }
        if phrases.len() > MAX_PHRASES {
            return Err("Use at most 32 comma-separated selection phrases".into());
        }
    }
    if phrases.is_empty() {
        return Err("Enter at least one selection phrase".into());
    }
    let mut matched = Vec::new();
    let mut combined: Option<(u32, u32, Vec<u8>)> = None;
    for phrase in &phrases {
        let bytes = match ground(phrase) {
            Ok(bytes) => bytes,
            Err(error) if error.starts_with("Nothing matched") => continue,
            Err(error) => return Err(error),
        };
        let image = decode_mask(&bytes)?;
        matched.push(*phrase);
        let pixels = image.as_u8().ok_or_else(|| "SAM 3 mask has no pixels".to_string())?;
        if let Some((width, height, mask)) = &mut combined {
            if (*width, *height) != (image.width(), image.height()) || mask.len() != pixels.len() {
                return Err("SAM 3 phrase masks have different dimensions".into());
            }
            for (output, input) in mask.iter_mut().zip(pixels) {
                *output = (*output).max(*input);
            }
        } else {
            combined = Some((image.width(), image.height(), pixels.to_vec()));
        }
    }
    let (width, height, pixels) = combined.ok_or_else(|| format!("Nothing matched “{}”", phrases.join(", ")))?;
    let image = Image::from_u8(width, height, ChannelLayout::Gray, pixels).map_err(|error| error.to_string())?;
    let png = photocraft_codecs::encode(&image, Format::Png, &EncodeOptions::default()).map_err(|error| error.to_string())?;
    Ok((matched.join(", "), png))
}

fn decode_mask(bytes: &[u8]) -> Result<Image, String> {
    if bytes.len() > 24 * 1024 * 1024 {
        return Err("SAM 3 mask is too large".into());
    }
    let options =
        DecodeOptions { limits: Limits { max_width: 8192, max_height: 8192, max_pixels: 16_777_216, max_alloc: 32 * 1024 * 1024 }, keep_orientation: true };
    let image = photocraft_codecs::decode_as_with(Format::Png, bytes, &options).map_err(|error| error.to_string())?;
    // The adapter's mask contract is grayscale U8, independent of the document's depth.
    if image.layout() != ChannelLayout::Gray || image.sample_type() != SampleType::U8 {
        return Err("SAM 3 mask must be 8-bit grayscale".into());
    }
    Ok(image)
}

#[cfg(test)]
#[path = "sam3_multi_select_tests.rs"]
mod tests;
