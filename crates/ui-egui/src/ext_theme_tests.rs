//! The launcher's shared theme mapped onto PhotoCraft's tokens: readable, layout kept, safe fallback.

use super::*;

fn rgb(hex: u32) -> Rgb {
    let [_, r, g, b] = hex.to_be_bytes();
    Rgb(r, g, b)
}

fn palette(dark: bool, c: [u32; 9]) -> Palette {
    Palette {
        dark,
        background: rgb(c[0]),
        surface: rgb(c[1]),
        text: rgb(c[2]),
        muted_text: rgb(c[3]),
        line: rgb(c[4]),
        accent: rgb(c[5]),
        good: rgb(c[6]),
        warn: rgb(c[7]),
        bad: rgb(c[8]),
    }
}

fn midnight() -> Active {
    Active { palette: palette(true, [0x101820, 0x18222d, 0xe8eef4, 0xa9b6c2, 0x35424f, 0x4aa3ff, 0x3fd6b8, 0xffc14d, 0xff6259]), overrides: BTreeMap::new() }
}

fn paper() -> Active {
    Active { palette: palette(false, [0xf5f1e8, 0xece6d8, 0x1d1b16, 0x55504a, 0xcdc5b3, 0x1a5fb4, 0x0a7563, 0x8a5800, 0xb3261e]), overrides: BTreeMap::new() }
}

fn ratio(a: Color32, b: Color32) -> f64 {
    contrast_ratio(Rgb(a.r(), a.g(), a.b()), Rgb(b.r(), b.g(), b.b()))
}

/// Every colour a person reads text on, and the translucent accent over the ones it sits on.
fn surfaces(t: &Tokens) -> Vec<Color32> {
    let mut out = vec![t.chrome, t.canvas, t.dock, t.card, t.field, t.tab_strip, t.hover, t.pressed, t.row_selected];
    let k = f32::from(t.accent_soft.a()) / 255.0;
    for under in [t.dock, t.card, t.chrome, t.field] {
        let ch = |over: u8, under: u8| (f32::from(over) * k + f32::from(under) * (1.0 - k)).round() as u8;
        out.push(Color32::from_rgb(ch(t.accent.r(), under.r()), ch(t.accent.g(), under.g()), ch(t.accent.b(), under.b())));
    }
    out
}

/// The same checks PhotoCraft's own themes are held to, in the form this module promises.
fn meets_the_floor(t: &Tokens) -> bool {
    surfaces(t).into_iter().all(|s| {
        [t.text, t.text_dim, t.text_faint, t.accent_text].into_iter().all(|c| ratio(c, s) >= TEXT_RATIO)
            && [t.danger, t.warning].into_iter().all(|c| ratio(c, s) >= MARK_RATIO)
    }) && [1.0_f32, 0.93, 0.85].into_iter().all(|k| ratio(t.primary_text, t.primary_bg.gamma_multiply(k)) >= TEXT_RATIO)
}

#[test]
fn a_dark_and_a_light_palette_become_readable_tokens_in_the_current_layout() {
    for (kind, active, dark) in [
        (ThemeKind::ProMedium, midnight(), true),
        (ThemeKind::Pro, midnight(), true),
        (ThemeKind::Studio, midnight(), true),
        (ThemeKind::ProLight, paper(), false),
        (ThemeKind::StudioLight, paper(), false),
        (ThemeKind::ProMedium, paper(), false),
        (ThemeKind::StudioLight, midnight(), true),
        // A theme of the other brightness than the person's own layout keeps that layout's grammar.
        (ThemeKind::ProLight, midnight(), true),
        (ThemeKind::Studio, paper(), false),
        (ThemeKind::Classic, paper(), false),
    ] {
        let r = restyle(kind, &active).unwrap_or_else(|| panic!("{kind:?} should wear the theme"));
        assert_eq!(r.tokens.dark(), dark, "{kind:?}: the look follows the theme's own light or dark");
        assert_eq!(r.tokens.pro, Tokens::for_kind(kind).pro, "{kind:?}: the layout grammar stays");
        assert!(meets_the_floor(&r.tokens), "{kind:?}: unreadable");
    }
}

#[test]
fn the_palette_colours_land_where_a_person_would_expect() {
    let a = midnight();
    let t = restyle(ThemeKind::ProMedium, &a).unwrap().tokens;
    let c = |x: Rgb| Color32::from_rgb(x.0, x.1, x.2);
    assert_eq!(t.canvas, c(a.palette.background));
    assert_eq!(t.card, c(a.palette.surface));
    assert_eq!(t.text, c(a.palette.text));
    assert_eq!(t.accent, c(a.palette.accent));
    assert_eq!(t.separator, c(a.palette.line));
    assert_eq!(t.danger, c(a.palette.bad));
    assert_eq!(t.warning, c(a.palette.warn));
    assert_ne!(t.hover, t.card, "a hovered control is visibly different");
    assert_ne!(t.pressed, t.hover);
}

#[test]
fn a_text_colour_that_is_barely_readable_still_gives_readable_hover_and_pressed_surfaces() {
    // Text at 4.6:1 on the surface: a plain 8% mix toward the text for hover would drop it under 4.5.
    let tight = Active {
        palette: palette(true, [0x101820, 0x18222d, 0x7e8b98, 0x7e8b98, 0x35424f, 0x4aa3ff, 0x3fd6b8, 0xffc14d, 0xff6259]),
        overrides: BTreeMap::new(),
    };
    let r = restyle(ThemeKind::ProMedium, &tight).expect("still wearable");
    assert!(meets_the_floor(&r.tokens));
}

#[test]
fn overrides_by_photocrafts_own_names_apply_when_they_stay_readable() {
    let mut a = midnight();
    a.overrides.insert("chrome".into(), rgb(0x0a1016));
    a.overrides.insert("not_a_colour_here".into(), rgb(0xff00ff));
    let r = restyle(ThemeKind::ProMedium, &a).unwrap();
    assert!(r.used_overrides);
    assert_eq!(r.tokens.chrome, Color32::from_rgb(0x0a, 0x10, 0x16));
}

#[test]
fn overrides_with_names_photocraft_does_not_know_change_nothing() {
    let mut a = midnight();
    a.overrides.insert("not_a_colour_here".into(), rgb(0xff00ff));
    let r = restyle(ThemeKind::ProMedium, &a).unwrap();
    assert!(!r.used_overrides);
    assert_eq!(r.tokens, restyle(ThemeKind::ProMedium, &midnight()).unwrap().tokens);
}

#[test]
fn overrides_that_make_text_unreadable_are_dropped_and_the_palette_is_worn_alone() {
    let mut a = midnight();
    a.overrides.insert("card".into(), rgb(0xe8eef4));
    let r = restyle(ThemeKind::ProMedium, &a).expect("the palette itself is fine");
    assert!(!r.used_overrides);
    assert_eq!(r.tokens, restyle(ThemeKind::ProMedium, &midnight()).unwrap().tokens);
    assert!(meets_the_floor(&r.tokens));
}

#[test]
fn a_palette_that_cannot_be_made_readable_is_not_worn() {
    // Text, muted text and surface all one grey: nothing can be derived that reads.
    let flat = Active {
        palette: palette(true, [0x202020, 0x202020, 0x303030, 0x303030, 0x404040, 0x505050, 0x606060, 0x707070, 0x808080]),
        overrides: BTreeMap::new(),
    };
    assert_eq!(restyle(ThemeKind::ProMedium, &flat), None);
}

#[test]
fn status_colours_too_close_to_the_surfaces_to_see_are_not_worn() {
    // Text is perfectly readable; the error colour is almost the surface colour.
    let mut a = midnight();
    a.palette.bad = rgb(0x1d2833);
    assert_eq!(restyle(ThemeKind::ProMedium, &a), None);
    let mut a = midnight();
    a.palette.warn = rgb(0x1d2833);
    assert_eq!(restyle(ThemeKind::ProMedium, &a), None);
}

#[test]
fn an_accent_no_button_label_can_be_read_on_is_not_worn() {
    // A mid grey accent: neither light nor dark text reaches 4.5:1 on it, even at rest.
    let mut a = midnight();
    a.palette.accent = rgb(0x7f7f7f);
    assert_eq!(restyle(ThemeKind::ProMedium, &a), None);
}

#[test]
fn apply_publishes_the_worn_theme_and_gives_the_built_in_look_back_when_it_is_cleared() {
    let ctx = egui::Context::default();
    crate::theme::install_fonts(&ctx);
    crate::theme::apply(&ctx, ThemeKind::ProMedium);
    let built_in = Tokens::get(&ctx);
    set_active(&ctx, Some(midnight()));
    crate::theme::apply(&ctx, ThemeKind::ProMedium);
    let worn = Tokens::get(&ctx);
    assert_eq!(worn.card, Color32::from_rgb(0x18, 0x22, 0x2d));
    assert_ne!(worn, built_in);
    // The same theme survives the app re-applying its own choice (a menu pick, a font change).
    crate::theme::apply(&ctx, ThemeKind::ProMedium);
    assert_eq!(Tokens::get(&ctx), worn);
    set_active(&ctx, None);
    crate::theme::apply(&ctx, ThemeKind::ProMedium);
    assert_eq!(Tokens::get(&ctx), built_in);
}

#[test]
fn a_theme_that_cannot_be_worn_leaves_the_built_in_look() {
    let ctx = egui::Context::default();
    crate::theme::install_fonts(&ctx);
    let flat = Active {
        palette: palette(true, [0x202020, 0x202020, 0x303030, 0x303030, 0x404040, 0x505050, 0x606060, 0x707070, 0x808080]),
        overrides: BTreeMap::new(),
    };
    set_active(&ctx, Some(flat));
    crate::theme::apply(&ctx, ThemeKind::Studio);
    assert_eq!(Tokens::get(&ctx), Tokens::for_kind(ThemeKind::Studio));
}

#[cfg(not(target_arch = "wasm32"))]
mod watcher {
    use super::*;
    use std::path::{Path, PathBuf};

    fn scratch() -> PathBuf {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
        std::env::temp_dir().join(format!("photocraft-ext-theme-{}-{nanos}", std::process::id()))
    }

    /// A store folder as the launcher leaves it, written by hand so this test needs no archive code.
    fn write_store(root: &Path, generation: u64, worn: bool) {
        let package = root.join("packages").join("org.example.midnight").join("1.0.0");
        std::fs::create_dir_all(&package).unwrap();
        let theme = serde_json::json!({
            "schema": 1,
            "palette": {
                "dark": true, "background": "#101820", "surface": "#18222d", "text": "#e8eef4", "muted_text": "#a9b6c2",
                "line": "#35424f", "accent": "#4aa3ff", "good": "#3fd6b8", "warn": "#ffc14d", "bad": "#ff6259"
            },
            "apps": {"photocraft": {"chrome": "#0a1016"}, "filmcraft": {"chrome": "#ff0000"}}
        });
        std::fs::write(package.join("theme.json"), theme.to_string()).unwrap();
        let mut state = serde_json::json!({
            "schema": 1, "generation": generation,
            "installed": [{"id": "org.example.midnight", "version": "1.0.0", "kind": "theme", "name": "Midnight", "tier": "from-file", "apps": ["photocraft"]}],
            "enabled": {"photocraft": ["org.example.midnight"]},
            "active_theme": {}
        });
        if worn {
            state["active_theme"]["photocraft"] = serde_json::json!("org.example.midnight");
        }
        std::fs::write(root.join("state.json"), state.to_string()).unwrap();
    }

    fn poll_at(w: &mut Watcher, ctx: &egui::Context, time: f64) -> bool {
        let mut changed = false;
        ctx.run_ui(egui::RawInput { time: Some(time), ..Default::default() }, |ui| changed = w.poll(ui.ctx())).textures_delta.clear();
        changed
    }

    #[test]
    fn a_theme_chosen_elsewhere_arrives_within_a_second_or_two_and_leaves_when_unchosen() {
        let root = scratch();
        let ctx = egui::Context::default();
        let mut w = Watcher::default();
        w.follow(root.clone());
        assert!(!poll_at(&mut w, &ctx, 0.0), "no store yet means no change");
        assert_eq!(active(&ctx), None);

        write_store(&root, 1, true);
        assert!(!poll_at(&mut w, &ctx, 0.5), "the store is looked at once a second at most");
        assert!(poll_at(&mut w, &ctx, 1.5));
        let worn = active(&ctx).expect("the chosen theme");
        assert!(worn.palette.dark);
        assert_eq!(worn.overrides.len(), 1, "only PhotoCraft's own overrides come through");
        assert!(worn.overrides.contains_key("chrome"));
        assert!(!poll_at(&mut w, &ctx, 3.0), "nothing changed, so nothing to do");

        write_store(&root, 2, true);
        assert!(!poll_at(&mut w, &ctx, 4.5), "something else changed in the store, not this app's theme");
        write_store(&root, 3, false);
        assert!(poll_at(&mut w, &ctx, 6.0));
        assert_eq!(active(&ctx), None, "unchoosing hands the built-in look back");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_watcher_that_was_never_pointed_at_a_store_reads_nothing() {
        let ctx = egui::Context::default();
        let mut w = Watcher::default();
        assert!(!poll_at(&mut w, &ctx, 10.0));
        assert_eq!(active(&ctx), None);
    }

    #[test]
    fn a_damaged_store_counts_as_nothing_chosen() {
        let root = scratch();
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("state.json"), b"{ broken").unwrap();
        let ctx = egui::Context::default();
        let mut w = Watcher::default();
        w.follow(root.clone());
        assert!(!poll_at(&mut w, &ctx, 0.0));
        assert_eq!(active(&ctx), None);
        let _ = std::fs::remove_dir_all(root);
    }
}
