//! Generative Fill: fill the selection with something generated, or remove what is inside it.
//!
//! `edit.generativeFill` crops the document to the selection plus some context, sends that to the
//! connected [`GenerationBackend`], and puts each result on a new layer whose transparency is the
//! selection. The original pixels are never touched, the whole thing is one undo step, and it runs
//! on a worker thread with progress and cancel like any other long command.
//!
//! Which service answers is not the engine's business: a host connects a backend with
//! [`AiState::set_backend`]. With none connected the command is disabled and says so. A backend
//! that sends data off the machine ([`GenerationBackend::is_remote`]) is refused until the host
//! calls [`AiState::grant_remote`] after the user agreed to what is sent. That is a Rust call, not a
//! command, so automation (CLI, scripts, MCP) cannot upload document pixels by itself.
//!
//! Colour: the document composite is converted to sRGB for the service and the result back to the
//! document's profile. RGB documents at 8 or 16 bits are supported; other modes and 32-bit
//! documents return a clear "not supported yet" error rather than guessing.

use std::sync::Arc;

use photocraft_ai::{
    AiError, Control, ErrorKind, GenerationBackend, IRect, Image, Mask, MockBackend, MockBehavior, PrepLimits, Task, build_request, compose_patch, plan_region,
};
use photocraft_cms::{Builtin, Intent, Transform};
use photocraft_color::{ColorMode, SampleType};
use photocraft_doc::{Document, Layer};
use photocraft_geom::Rect;
use photocraft_raster::{Surface, from_rgba_into};
use serde_json::{Value, json};

use crate::commands::CommandSpec;
use crate::jobs::JobCtx;
use crate::{EngineError, Result, Session};

const CMD: &str = "edit.generativeFill";

/// Where the session keeps the connected backend.
#[derive(Default, Clone)]
pub struct AiState {
    backend: Option<Arc<dyn GenerationBackend>>,
    /// The user agreed to send pixels to the connected remote service. Only the host can set this
    /// (there is no command for it), so the CLI, scripts and MCP can never grant it themselves.
    remote_granted: bool,
}

impl AiState {
    /// Connects a backend (or disconnects with `None`). Any earlier agreement to send data
    /// elsewhere ends here: a new destination needs its own.
    pub fn set_backend(&mut self, backend: Option<Arc<dyn GenerationBackend>>) {
        self.backend = backend;
        self.remote_granted = false;
    }

    /// Records that the user agreed, in the host's own UI, to what is sent to the connected remote
    /// service. Deliberately not reachable through a command.
    pub fn grant_remote(&mut self) {
        self.remote_granted = true;
    }

    pub fn remote_granted(&self) -> bool {
        self.remote_granted
    }

    pub fn backend(&self) -> Option<&Arc<dyn GenerationBackend>> {
        self.backend.as_ref()
    }

    /// The connected service's name for the UI.
    pub fn label(&self) -> Option<String> {
        self.backend.as_ref().map(|b| b.label())
    }
}

/// Wraps the job context as the port's [`Control`], mapping the model's progress into the middle
/// of the job (preparing before, applying after).
struct JobControl<'a>(&'a JobCtx);

impl Control for JobControl<'_> {
    fn cancelled(&self) -> bool {
        self.0.cancelled()
    }

    fn progress(&self, fraction: f32, message: &str) {
        self.0.progress(0.15 + 0.7 * fraction.clamp(0.0, 1.0), message);
    }
}

fn port_error(e: AiError) -> EngineError {
    match e.kind {
        ErrorKind::Cancelled => EngineError::Cancelled,
        ErrorKind::ConsentRequired => EngineError::Other(format!("{e}")),
        _ => EngineError::Other(e.message),
    }
}

fn cms_error(e: impl std::fmt::Display) -> EngineError {
    EngineError::Other(format!("colour conversion failed: {e}"))
}

fn supported(doc: &Document) -> std::result::Result<(), String> {
    if doc.mode != ColorMode::Rgb {
        return Err("Generative Fill works on RGB documents for now (Image > Mode > RGB Color)".into());
    }
    if doc.depth == SampleType::F32 {
        return Err("Generative Fill works on 8 and 16 bit documents for now (Image > Mode > 16 Bits/Channel)".into());
    }
    Ok(())
}

fn has_selection(doc: &Document) -> bool {
    doc.selection.as_ref().is_some_and(|s| !s.content_bounds().is_empty())
}

fn enabled(s: &Session) -> std::result::Result<(), String> {
    let d = s.active().ok_or("no document open")?;
    if s.ai.backend.is_none() {
        return Err("no AI model is connected (Edit > Preferences > Integrations)".into());
    }
    supported(&d.doc)?;
    if !has_selection(&d.doc) {
        return Err("make a selection first".into());
    }
    Ok(())
}

/// Converts RGBA8 pixels between two profiles in place, in blocks so the scratch copy stays small.
fn convert_u8(t: &Transform, px: &mut [u8]) {
    const BLOCK: usize = 1 << 20;
    let mut src = Vec::new();
    for chunk in px.chunks_mut(BLOCK * 4) {
        src.clear();
        src.extend_from_slice(chunk);
        t.convert_u8(&src, 4, chunk, 4, true);
    }
}

/// The document composite over `rect` as an sRGB image.
fn composite_srgb(doc: &Document, rect: Rect) -> Result<Image> {
    let mut img = photocraft_compose::render(doc, rect).to_rgba8();
    let src = crate::color_cmds::composite_profile(doc);
    let t = Transform::new(&src, Builtin::Srgb.profile(), Intent::RelativeColorimetric, true).map_err(cms_error)?;
    convert_u8(&t, &mut img.pixels);
    Image::new(img.width, img.height, img.pixels).map_err(port_error)
}

/// The selection's coverage over `rect`.
fn selection_mask(sel: &Surface, rect: Rect) -> Result<Mask> {
    let n = sel.channels().max(1);
    let data = sel.read_region(rect);
    let cover = data.chunks(n).map(|px| (px.first().copied().unwrap_or(0.0).clamp(0.0, 1.0) * 255.0 + 0.5) as u8).collect();
    Mask::new(rect.width(), rect.height(), cover).map_err(port_error)
}

/// A new layer holding `patch` (sRGB, straight alpha) at `at`, in the document's profile and format.
fn patch_layer(doc: &Document, patch: &Image, at: Rect, name: &str) -> Result<Layer> {
    let mut px = patch.rgba().to_vec();
    let dst = crate::color_cmds::composite_profile(doc);
    let t = Transform::new(Builtin::Srgb.profile(), &dst, Intent::RelativeColorimetric, true).map_err(cms_error)?;
    convert_u8(&t, &mut px);
    let format = doc.pixel_format();
    let mut layer = Layer::raster(name, format);
    let surface = layer.surface_mut().ok_or_else(|| EngineError::Other("could not create the layer".into()))?;
    let channels = surface.channels().max(1);
    let mut data = vec![0.0f32; px.len() / 4 * channels];
    for (src, out) in px.as_chunks::<4>().0.iter().zip(data.chunks_mut(channels)) {
        let rgba = [f32::from(src[0]) / 255.0, f32::from(src[1]) / 255.0, f32::from(src[2]) / 255.0, f32::from(src[3]) / 255.0];
        from_rgba_into(&format, rgba, out);
    }
    surface.try_write_region(at, &data)?;
    Ok(layer)
}

struct Params {
    task: Task,
    prompt: Option<String>,
    variations: u32,
}

fn parse(p: &Value) -> Result<Params> {
    let bad = |msg: &str| EngineError::BadParams { cmd: CMD.into(), msg: msg.into() };
    let task = match p.get("task").and_then(Value::as_str).unwrap_or("fill") {
        "fill" => Task::Fill,
        "remove" => Task::Remove,
        _ => return Err(bad("`task` is `fill` or `remove`")),
    };
    let prompt = match p.get("prompt") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => Some(s.clone()),
        Some(_) => return Err(bad("`prompt` is a string")),
    };
    let variations = match p.get("variations") {
        None | Some(Value::Null) => 1,
        Some(v) => v
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .filter(|n| (1..=photocraft_ai::backend::MAX_OUTPUTS).contains(n))
            .ok_or_else(|| bad("`variations` is 1 to 4"))?,
    };
    Ok(Params { task, prompt, variations })
}

fn run(s: &mut Session, p: &Value) -> Result<Value> {
    let params = parse(p)?;
    let backend = s.ai.backend.clone().ok_or_else(|| EngineError::Other("no AI model is connected (Edit > Preferences > Integrations)".into()))?;
    if !backend.supports(params.task) {
        return Err(EngineError::Other(format!("{} cannot do this", backend.label())));
    }
    if backend.is_remote() && !s.ai.remote_granted {
        return Err(EngineError::Other(format!("{} is a remote service: nothing is sent until you agree to what leaves this computer", backend.label())));
    }
    let doc = &s.active().ok_or(EngineError::NoDocument)?.doc;
    supported(doc).map_err(EngineError::Other)?;
    if !has_selection(doc) {
        return Err(EngineError::Other("make a selection first".into()));
    }
    crate::jobs::edit_job(
        s,
        "Generative Fill",
        move |doc, active, ctx| {
            ctx.progress(0.02, "Preparing");
            let sel = doc.selection.as_ref().ok_or_else(|| EngineError::Other("make a selection first".into()))?;
            // Exact bounds of what is selected (the surface only knows its tiles).
            let doc_rect = doc.bounds();
            let coarse = sel.content_bounds().intersect(&doc_rect);
            let exact = selection_mask(sel, coarse)?.bounds().ok_or_else(|| EngineError::Other("make a selection first".into()))?;
            let selection = IRect::new(coarse.x0 + exact.x0, coarse.y0 + exact.y0, coarse.x0 + exact.x1, coarse.y0 + exact.y1);
            let plan = plan_region(doc_rect.width(), doc_rect.height(), selection, &PrepLimits::default()).map_err(port_error)?;
            let crop = Rect::new(plan.crop.x0, plan.crop.y0, plan.crop.x1, plan.crop.y1);
            let (image, coverage) = (composite_srgb(doc, crop)?, selection_mask(sel, crop)?);
            ctx.check()?;
            let request = build_request(params.task, params.prompt, params.variations, &image, &coverage, &plan).map_err(port_error)?;
            ctx.progress(0.15, "Generating");
            let result = backend.generate(&request, &JobControl(ctx)).map_err(port_error)?;
            result.check(&request).map_err(port_error)?;
            ctx.check()?;
            ctx.progress(0.9, "Adding layers");
            let mut layers = Vec::new();
            for (i, img) in result.images.iter().enumerate() {
                let patch = compose_patch(img, &plan, &coverage).map_err(port_error)?;
                let name = if result.images.len() == 1 { "Generative Fill".to_owned() } else { format!("Generative Fill {}", i + 1) };
                let mut layer = patch_layer(doc, &patch, crop, &name)?;
                // Several variations stack up; only the first shows, the others wait to be picked.
                layer.visible = i == 0;
                layers.push(layer);
            }
            let count = layers.len();
            let mut first = None;
            let mut above = *active;
            for layer in layers {
                let id = doc.insert_above(above, layer);
                first.get_or_insert(id);
                above = Some(id);
            }
            *active = first;
            Ok((first, count, [crop.x0, crop.y0, crop.width() as i32, crop.height() as i32]))
        },
        |(first, count, bounds)| json!({ "layer": first.map(|l| l.0), "variations": count, "bounds": bounds }),
    )
}

fn connect_mock(s: &mut Session, p: &Value) -> Result<Value> {
    let bad = |msg: &str| EngineError::BadParams { cmd: "ai.connectMock".into(), msg: msg.into() };
    let colour = match p.get("color") {
        None | Some(Value::Null) => [200, 60, 60],
        Some(v) => {
            let a = v.as_array().filter(|a| a.len() == 3).ok_or_else(|| bad("`color` is [r, g, b], 0 to 255"))?;
            let mut c = [0u8; 3];
            for (o, x) in c.iter_mut().zip(a) {
                *o = x.as_u64().and_then(|n| u8::try_from(n).ok()).ok_or_else(|| bad("`color` is [r, g, b], 0 to 255"))?;
            }
            c
        }
    };
    let behavior = match p.get("behavior").and_then(Value::as_str).unwrap_or("fill") {
        "fill" => MockBehavior::Fill([colour[0], colour[1], colour[2], 255]),
        "gradient" => MockBehavior::Gradient,
        "echo" => MockBehavior::Echo,
        _ => return Err(bad("`behavior` is `fill`, `gradient` or `echo`")),
    };
    let mut mock = MockBackend::new(behavior);
    if p.get("remote").and_then(Value::as_bool).unwrap_or(false) {
        mock = mock.remote();
    }
    s.ai.set_backend(Some(Arc::new(mock)));
    Ok(json!({ "connected": "Mock" }))
}

fn disconnect(s: &mut Session, _p: &Value) -> Result<Value> {
    s.ai.set_backend(None);
    Ok(json!({}))
}

fn always(_: &Session) -> std::result::Result<(), String> {
    Ok(())
}

/// Generative Fill command spec (no menu path yet: the contextual task bar and Edit menu come
/// with the UI).
pub fn specs() -> Vec<CommandSpec> {
    vec![
        CommandSpec {
            id: CMD,
            label: "Generative Fill",
            menu: &[],
            shortcut: None,
            params: r##"{"prompt":string?,"variations":1..4=1,"task":"fill"|"remove"="fill"} → {layer,variations,bounds} (sends the selection and its surroundings to the connected AI model and adds each result as a new layer shaped by the selection; needs a connected model and an RGB, 8 or 16 bit document; a remote model also needs the user's agreement, which only the app UI can record, never a command)"##,
            enabled,
            run,
            journal: true,
        },
        CommandSpec {
            id: "ai.connectMock",
            label: "Connect Test AI Model",
            menu: &[],
            shortcut: None,
            params: r##"{"behavior":"fill"|"gradient"|"echo"="fill","color":[r,g,b]?,"remote":bool=false} → {connected} (connects a built-in stand-in that paints the selection, so Generative Fill can be tried and tested without a service)"##,
            enabled: always,
            run: connect_mock,
            journal: false,
        },
        CommandSpec {
            id: "ai.disconnect",
            label: "Disconnect AI Model",
            menu: &[],
            shortcut: None,
            params: "{} → {} (Generative Fill is unavailable until a model is connected again)",
            enabled: always,
            run: disconnect,
            journal: false,
        },
    ]
}

#[cfg(test)]
mod tests;
