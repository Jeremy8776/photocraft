//! Import an exact local SAM 3 mask as an undoable selection, through the command seam.
//! No network access here: the UI supplies a bounded PNG and the document/revision it prepared.
use crate::commands::CommandSpec;
use crate::{EngineError, Result, Session};
use base64::Engine;
use photocraft_algo::selection::{Region, SelectionMode, combine_region};
use photocraft_codecs::{ChannelLayout, Format, SampleType};
use photocraft_geom::Rect;
use serde_json::{Value, json};

const CMD: &str = "select.sam3Mask";

fn bad(msg: &str) -> EngineError {
    EngineError::BadParams { cmd: CMD.into(), msg: msg.into() }
}

fn enabled(s: &Session) -> std::result::Result<(), String> {
    s.active().map(|_| ()).ok_or_else(|| "no document open".into())
}

fn mode(p: &Value) -> Result<SelectionMode> {
    match p.get("mode").and_then(Value::as_str).unwrap_or("replace") {
        "replace" => Ok(SelectionMode::Replace),
        "add" => Ok(SelectionMode::Add),
        "subtract" => Ok(SelectionMode::Subtract),
        "intersect" => Ok(SelectionMode::Intersect),
        _ => Err(bad("mode must be replace, add, subtract or intersect")),
    }
}

fn resample(src: &[u8], sw: usize, sh: usize, dw: usize, dh: usize) -> Vec<u8> {
    let mut out = vec![0; dw.saturating_mul(dh)];
    for (y, row) in out.chunks_mut(dw).enumerate() {
        let sy = ((y as f32 + 0.5) * sh as f32 / dh as f32 - 0.5).clamp(0.0, (sh - 1) as f32);
        let y0 = sy.floor() as usize;
        let y1 = (y0 + 1).min(sh - 1);
        let fy = sy - y0 as f32;
        for (x, pixel) in row.iter_mut().enumerate() {
            let sx = ((x as f32 + 0.5) * sw as f32 / dw as f32 - 0.5).clamp(0.0, (sw - 1) as f32);
            let x0 = sx.floor() as usize;
            let x1 = (x0 + 1).min(sw - 1);
            let fx = sx - x0 as f32;
            let sample = |xx, yy| f32::from(src.get(yy * sw + xx).copied().unwrap_or(0));
            let a = sample(x0, y0) * (1.0 - fx) + sample(x1, y0) * fx;
            let b = sample(x0, y1) * (1.0 - fx) + sample(x1, y1) * fx;
            *pixel = (a * (1.0 - fy) + b * fy).round().clamp(0.0, 255.0) as u8;
        }
    }
    out
}

fn run(s: &mut Session, p: &Value) -> Result<Value> {
    let st = s.active().ok_or(EngineError::NoDocument)?;
    let id = p.get("docId").and_then(Value::as_u64).ok_or_else(|| bad("docId is required"))?;
    let revision = p.get("revision").and_then(Value::as_u64).ok_or_else(|| bad("revision is required"))?;
    if st.doc.id.0 != id || st.revision != revision {
        return Err(bad("the document changed while SAM 3 was selecting; try again"));
    }
    let mode = mode(p)?;
    let encoded = p
        .get("maskPng")
        .and_then(Value::as_str)
        .filter(|text| text.len() <= 32 * 1024 * 1024)
        .ok_or_else(|| bad("maskPng is required and must be under 32 MiB"))?;
    let data = base64::engine::general_purpose::STANDARD.decode(encoded).map_err(|_| bad("maskPng is not base64"))?;
    if data.len() > 24 * 1024 * 1024 {
        return Err(bad("SAM 3 mask is too large"));
    }
    let image = photocraft_codecs::decode_as(Format::Png, &data).map_err(|_| bad("SAM 3 mask is not a valid PNG"))?;
    if image.layout() != ChannelLayout::Gray || image.sample_type() != SampleType::U8 {
        return Err(bad("SAM 3 mask must be 8-bit grayscale"));
    }
    let (sw, sh) = (image.width() as usize, image.height() as usize);
    let b = st.doc.bounds();
    let (dw, dh) = (b.width() as usize, b.height() as usize);
    if sw == 0 || sh == 0 || dw == 0 || dh == 0 || sw > 8192 || sh > 8192 || sw.saturating_mul(sh) > 16_777_216 || dw.saturating_mul(dh) > 40_000_000 {
        return Err(bad("SAM 3 mask or document dimensions are unsupported"));
    }
    let source = image.as_u8().ok_or_else(|| bad("SAM 3 mask has no pixels"))?;
    if source.len() != sw * sh {
        return Err(bad("SAM 3 mask pixels do not match its dimensions"));
    }
    if source.iter().all(|v| *v == 0) {
        return Ok(json!({"changed":false,"selected":st.doc.selection.is_some()}));
    }
    let mask = resample(source, sw, sh, dw, dh);
    let region = Region { bbox: Rect::new(0, 0, dw as i32, dh as i32), mask };
    let selected = s.edit("Object Selection", |doc, _| {
        doc.selection = combine_region(doc.selection.as_ref(), Some(&region), mode);
        Ok(doc.selection.is_some())
    })?;
    Ok(json!({"changed":true,"selected":selected}))
}

pub fn specs() -> Vec<CommandSpec> {
    vec![CommandSpec {
        id: CMD,
        label: "Object Selection Mask",
        menu: &[],
        shortcut: None,
        params: r##"{"maskPng":"base64 grayscale PNG","docId":u64,"revision":u64,"mode":"replace|add|subtract|intersect"}"##,
        enabled,
        run,
        journal: false,
    }]
}
