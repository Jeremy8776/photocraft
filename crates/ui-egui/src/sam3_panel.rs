//! Floating Object Selection panel: everything the local SAM 3 model offers on one card.
//! Points so far, the box, the selection mode, Select Subject (text grounding) and Apply.
use crate::PhotocraftApp;
use crate::sam3_prompts::Mode;
use crate::state::Tool;
use egui::{Align, Button, Layout, RichText};

#[derive(Default)]
struct Actions {
    commit: bool,
    ground: bool,
    clear: bool,
    undo: bool,
    close: bool,
}

pub(crate) fn show(app: &mut PhotocraftApp, ctx: &egui::Context) {
    if app.ui.tool != Tool::ObjectSelection || !app.ui.sam3_panel {
        return;
    }
    let t = crate::theme::Tokens::get(ctx);
    let canvas = app.last_canvas_rect;
    let width = 250.0;
    let movable = !app.session.prefs().workspace_locked;
    let (positive, negative, has_box) = {
        let b = &app.sam3.builder;
        (b.positive.len(), b.negative.len(), b.box_.is_some())
    };
    let error = app.sam3.error();
    let mut mode = app.sam3.builder.mode;
    let mut text = app.sam3.builder.text.clone();
    let mut act = Actions::default();
    egui::Window::new(tl!("Select Subject"))
        .id(egui::Id::new("sam3-panel"))
        .title_bar(false)
        .resizable(false)
        .movable(movable)
        .min_width(width)
        .max_width(width)
        .default_pos(egui::pos2((canvas.right() - width - 16.0).max(canvas.left() + 8.0), canvas.top() + 8.0))
        .show(ctx, |ui| {
            ui.set_width(width);
            ui.horizontal(|ui| {
                ui.label(RichText::new(tl!("Select Subject")).strong().size(12.0));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.add(Button::new("×").frame(false)).clicked() {
                        act.close = true;
                    }
                });
            });
            ui.separator();
            if positive + negative == 0 && !has_box {
                ui.label(RichText::new(tl!("Click objects to add points. Alt-click excludes.")).size(11.0).color(t.text_faint));
            } else {
                let boxed = if has_box { "  ☐" } else { "" };
                ui.label(RichText::new(format!("{positive} +   {negative} −{boxed}")).size(11.0).color(t.text));
            }
            ui.horizontal(|ui| {
                ui.label(RichText::new(tl!("Mode")).size(11.0).color(t.text_dim));
                egui::ComboBox::from_id_salt("sam3-mode").selected_text(mode_label(mode)).width(ui.available_width() - 8.0).show_ui(ui, |ui| {
                    for m in [Mode::Replace, Mode::Add, Mode::Subtract, Mode::Intersect] {
                        ui.selectable_value(&mut mode, m, mode_label(m));
                    }
                });
            });
            ui.horizontal(|ui| {
                ui.label(RichText::new(tl!("Text")).size(11.0).color(t.text_dim));
                let field = ui.add(egui::TextEdit::singleline(&mut text).desired_width(ui.available_width() - 8.0).hint_text(tl!("person, red car…")));
                // Enter in the field runs Select Subject, as the button does.
                if field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    act.ground = true;
                }
            });
            ui.horizontal(|ui| {
                act.ground |= ui.add(Button::new(tl!("Select Subject")).small()).clicked();
                act.commit |= ui.add(Button::new(tl!("Apply")).small()).clicked();
                act.clear |= ui.add(Button::new(tl!("Clear")).small()).clicked();
                act.undo |= ui.add(Button::new(tl!("Undo")).small()).clicked();
            });
            if let Some(error) = &error {
                ui.label(RichText::new(error).size(11.0).color(t.warning));
            }
        });
    app.sam3.builder.mode = mode;
    app.sam3.builder.text = text.clone();
    if act.close {
        crate::sam3_selection::Runtime::panel_closed(app);
    }
    if act.clear {
        app.sam3.clear_prompt();
    }
    if act.undo {
        app.sam3.undo_point();
    }
    let size = app.session.active().map(|st| [st.doc.size.width, st.doc.size.height]);
    if act.commit
        && let Some(size) = size
        && app.sam3.commit(size)
    {
        app.ui.status = "Selecting object with SAM 3…".into();
        app.ui.status_error = false;
    }
    if act.ground && app.sam3.ground(&text) {
        app.ui.status = "Selecting subject with SAM 3…".into();
        app.ui.status_error = false;
    }
}

/// `select.subject` while the Object Selection tool is active (the options-bar button, the menu
/// item, automation): use SAM 3 with the panel's text instead of the older colour-based
/// algorithm. Returns false to let that algorithm run when SAM 3 is not in play or has failed.
pub(crate) fn select_subject(app: &mut PhotocraftApp) -> bool {
    if app.ui.tool != Tool::ObjectSelection || app.sam3.error().is_some() {
        return false;
    }
    let text = app.sam3.builder.text.clone();
    app.ui.status_error = false;
    app.ui.status = if app.sam3.ground(&text) { "Selecting subject with SAM 3…" } else { "SAM 3 is busy, try again in a moment" }.into();
    true
}

fn mode_label(mode: Mode) -> &'static str {
    match mode {
        Mode::Replace => tl!("Replace"),
        Mode::Add => tl!("Add"),
        Mode::Subtract => tl!("Subtract"),
        Mode::Intersect => tl!("Intersect"),
    }
}
