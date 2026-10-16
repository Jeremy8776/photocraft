//! A theme chosen in ArtCraft Launcher, worn by PhotoCraft.
//!
//! The shared theme is a small palette (background, surface, text, muted text, line, accent and
//! three status colours) plus optional overrides that use PhotoCraft's own colour names. This module
//! maps it onto [`Tokens`].
//!
//! Invariants:
//! - A theme never makes the interface unreadable. After the palette is mapped and the overrides are
//!   applied, every text colour is checked against every surface it can sit on (4.5:1) and every
//!   status colour as well (3:1). If the overrides break that, they are dropped and the palette is
//!   used alone; if the palette alone breaks it, the theme is not worn and the built-in look stays.
//! - The built-in layout (Pro or Studio grammar, round chips, bevels) is kept: only colours change.
//! - Nothing here reads the disk except [`Watcher`], and it never writes. Bad or missing data counts
//!   as absent.

use std::collections::BTreeMap;

use craft_ext_core::palette::{Palette, Rgb, contrast_ratio};
use egui::Color32;

use crate::theme::{ThemeKind, Tokens};

/// PhotoCraft's id in the shared store.
pub const APP_ID: &str = "photocraft";

const TEXT_RATIO: f64 = 4.5;
const MARK_RATIO: f64 = 3.0;
const CTX_KEY: &str = "photocraft-ext-theme";

/// The theme PhotoCraft has been asked to wear.
#[derive(Clone, Debug, PartialEq)]
pub struct Active {
    pub palette: Palette,
    /// PhotoCraft's own colour names, as the theme's author wrote them.
    pub overrides: BTreeMap<String, Rgb>,
}

/// A mapped theme.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Restyled {
    pub tokens: Tokens,
    /// The theme's overrides were applied (they passed the readability check).
    pub used_overrides: bool,
}

fn c32(c: Rgb) -> Color32 {
    Color32::from_rgb(c.0, c.1, c.2)
}

fn rgb(c: Color32) -> Rgb {
    Rgb(c.r(), c.g(), c.b())
}

/// `from` moved `t` (0 to 1) of the way toward `to`.
fn mix(from: Rgb, to: Rgb, t: f32) -> Rgb {
    let ch = |a: u8, b: u8| (f32::from(a) + (f32::from(b) - f32::from(a)) * t).round().clamp(0.0, 255.0) as u8;
    Rgb(ch(from.0, to.0), ch(from.1, to.1), ch(from.2, to.2))
}

/// The furthest point (at most `max` of the way) from `from` toward `to` where `holds` is still true.
fn shift(from: Rgb, to: Rgb, max: f32, holds: impl Fn(Rgb) -> bool) -> Rgb {
    (0..=8).map(|i| mix(from, to, max * (1.0 - i as f32 / 8.0))).find(|c| holds(*c)).unwrap_or(from)
}

/// The layout family of `kind` in the theme's own light or dark.
fn base_kind(kind: ThemeKind, dark: bool) -> ThemeKind {
    let t = Tokens::for_kind(kind);
    match (t.dark() == dark, t.pro, dark) {
        (true, ..) => kind,
        (false, true, true) => ThemeKind::ProMedium,
        (false, true, false) => ThemeKind::ProLight,
        (false, false, true) => ThemeKind::Studio,
        (false, false, false) => ThemeKind::StudioLight,
    }
}

/// The palette mapped onto the layout of `kind`, before any check.
fn derive(kind: ThemeKind, p: &Palette) -> Tokens {
    let base = Tokens::for_kind(base_kind(kind, p.dark));
    let reads = |c: Rgb| contrast_ratio(p.text, c) >= TEXT_RATIO && contrast_ratio(p.muted_text, c) >= TEXT_RATIO;
    let hover = shift(p.surface, p.text, 0.08, reads);
    let pressed = shift(p.surface, p.text, 0.16, reads);
    let selected = shift(p.surface, p.accent, 0.30, reads);
    let over = |alpha: u8, under: Rgb| mix(under, p.accent, f32::from(alpha) / 255.0);
    let soft = [72_u8, 56, 40, 24, 12, 0].into_iter().find(|a| reads(over(*a, p.background)) && reads(over(*a, p.surface))).unwrap_or(0);
    let on = [p.background, p.surface, hover, pressed, selected, over(soft, p.background), over(soft, p.surface)];
    let accent_text =
        (0..=10).map(|i| mix(p.accent, p.text, i as f32 / 10.0)).find(|c| on.iter().all(|s| contrast_ratio(*c, *s) >= TEXT_RATIO)).unwrap_or(p.text);
    let dim = |k: f32| Rgb((f32::from(p.accent.0) * k) as u8, (f32::from(p.accent.1) * k) as u8, (f32::from(p.accent.2) * k) as u8);
    let primary_text = [p.background, p.text, Rgb(255, 255, 255), Rgb(0, 0, 0)]
        .into_iter()
        .find(|c| [1.0, 0.93, 0.85].into_iter().all(|k| contrast_ratio(*c, dim(k)) >= TEXT_RATIO))
        .unwrap_or(p.text);
    Tokens {
        chrome: c32(p.surface),
        canvas: c32(p.background),
        canvas_dot: c32(mix(p.background, p.line, 0.6)),
        dock: c32(p.background),
        card: c32(p.surface),
        card_border: c32(p.line),
        field: c32(p.background),
        field_border: c32(p.line),
        hover: c32(hover),
        pressed: c32(pressed),
        text: c32(p.text),
        text_dim: c32(mix(p.text, p.muted_text, 0.5)),
        text_faint: c32(p.muted_text),
        icon: c32(p.text),
        accent: c32(p.accent),
        accent_soft: Color32::from_rgba_unmultiplied(p.accent.0, p.accent.1, p.accent.2, soft),
        accent_border: c32(p.accent),
        accent_text: c32(accent_text),
        separator: c32(p.line),
        primary_bg: c32(p.accent),
        primary_text: c32(primary_text),
        danger: c32(p.bad),
        warning: c32(p.warn),
        tab_strip: c32(mix(p.surface, p.background, 0.5)),
        row_selected: c32(selected),
        ..base
    }
}

/// Applies the theme's own-name overrides; the flag says whether any known name was present.
fn with_overrides(mut t: Tokens, overrides: &BTreeMap<String, Rgb>) -> (Tokens, bool) {
    let mut any = false;
    for (name, value) in overrides {
        let c = c32(*value);
        macro_rules! set {
            ($($f:ident),*) => {
                match name.as_str() {
                    $(stringify!($f) => { t.$f = c; any = true; })*
                    _ => {}
                }
            };
        }
        set!(
            chrome,
            canvas,
            canvas_dot,
            dock,
            card,
            card_border,
            field,
            field_border,
            hover,
            pressed,
            text,
            text_dim,
            text_faint,
            icon,
            accent,
            accent_soft,
            accent_border,
            accent_text,
            separator,
            shadow,
            primary_bg,
            primary_text,
            danger,
            warning,
            tab_strip,
            row_selected
        );
    }
    (t, any)
}

/// Every colour a person reads text on, including the translucent accent over the ones it sits on.
fn surfaces(t: &Tokens) -> Vec<Color32> {
    let mut out = vec![t.chrome, t.canvas, t.dock, t.card, t.field, t.tab_strip, t.hover, t.pressed, t.row_selected];
    let k = f32::from(t.accent_soft.a()) / 255.0;
    for under in [t.dock, t.card, t.chrome, t.field] {
        let ch = |over: u8, under: u8| (f32::from(over) * k + f32::from(under) * (1.0 - k)).round() as u8;
        out.push(Color32::from_rgb(ch(t.accent.r(), under.r()), ch(t.accent.g(), under.g()), ch(t.accent.b(), under.b())));
    }
    out
}

/// Text holds 4.5:1 on every surface, status colours 3:1, and a primary button's label 4.5:1 while it darkens.
fn readable(t: &Tokens) -> bool {
    let ratio = |a: Color32, b: Color32| contrast_ratio(rgb(a), rgb(b));
    surfaces(t).into_iter().all(|s| {
        [t.text, t.text_dim, t.text_faint, t.accent_text].into_iter().all(|c| ratio(c, s) >= TEXT_RATIO)
            && [t.danger, t.warning].into_iter().all(|c| ratio(c, s) >= MARK_RATIO)
    }) && [1.0_f32, 0.93, 0.85].into_iter().all(|k| ratio(t.primary_text, t.primary_bg.gamma_multiply(k)) >= TEXT_RATIO)
}

/// Maps `active` onto the layout of `kind`. `None` when the result cannot be made readable.
pub fn restyle(kind: ThemeKind, active: &Active) -> Option<Restyled> {
    let base = derive(kind, &active.palette);
    let (with, applied) = with_overrides(base, &active.overrides);
    if applied && readable(&with) {
        return Some(Restyled { tokens: with, used_overrides: true });
    }
    readable(&base).then_some(Restyled { tokens: base, used_overrides: false })
}

/// The tokens `apply` should publish for `kind`: the theme in the context if it can be worn,
/// otherwise the built-in ones.
pub fn tokens_for(ctx: &egui::Context, kind: ThemeKind) -> Tokens {
    active(ctx).and_then(|a| restyle(kind, &a)).map_or_else(|| Tokens::for_kind(kind), |r| r.tokens)
}

/// Sets (or clears) the theme worn by this context.
pub fn set_active(ctx: &egui::Context, active: Option<Active>) {
    ctx.data_mut(|d| d.insert_temp(egui::Id::new(CTX_KEY), active));
}

/// The theme set on this context, if any.
pub fn active(ctx: &egui::Context) -> Option<Active> {
    ctx.data(|d| d.get_temp::<Option<Active>>(egui::Id::new(CTX_KEY))).flatten()
}

/// Follows the shared store: when anything changes there (an install or a choice in the launcher or
/// another app), the theme PhotoCraft should wear is read again. Starts switched off, so tests and
/// tools that build the app never read the person's real store.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Default)]
pub struct Watcher {
    reader: Option<craft_ext_store::reader::Reader>,
    generation: Option<u64>,
    next_check: f64,
}

#[cfg(not(target_arch = "wasm32"))]
impl Watcher {
    /// Starts following the store in its usual place (`CRAFT_EXT_DIR` overrides it).
    pub fn follow_default_store(&mut self) {
        if let Ok(root) = craft_ext_store::location::default_root() {
            self.follow(root);
        }
    }

    /// Starts following the store in `root`.
    pub fn follow(&mut self, root: std::path::PathBuf) {
        self.reader = Some(craft_ext_store::reader::Reader::open(root));
        self.generation = None;
        self.next_check = 0.0;
    }

    /// Looks at the store at most once a second. True when the theme to wear changed.
    pub fn poll(&mut self, ctx: &egui::Context) -> bool {
        let Some(reader) = &self.reader else { return false };
        let now = ctx.input(|i| i.time);
        if now < self.next_check {
            return false;
        }
        self.next_check = now + 1.0;
        // A slow wake-up so a theme chosen while PhotoCraft sits in the background still arrives.
        ctx.request_repaint_after(std::time::Duration::from_secs(2));
        let generation = reader.generation();
        if self.generation == Some(generation) {
            return false;
        }
        self.generation = Some(generation);
        let found = craft_ext_core::id::AppId::new(APP_ID)
            .ok()
            .and_then(|app| reader.active_theme(&app))
            .map(|t| Active { palette: t.palette, overrides: t.overrides });
        if found == active(ctx) {
            return false;
        }
        set_active(ctx, found);
        true
    }
}

#[cfg(test)]
#[path = "ext_theme_tests.rs"]
mod tests;
