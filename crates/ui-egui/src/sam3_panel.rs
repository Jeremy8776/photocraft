//! Object Selection owns this independent pop-out, never a document, layer or mask
//! Properties section. Selection actions share the same SAM 3 runtime.
use crate::PhotocraftApp;
use crate::sam3_prompts::Mode;
use crate::state::Tool;
use crate::theme::Tokens;
use egui::{Align2, RichText, Sense, pos2, vec2};

#[derive(Default)]
struct Actions {
    commit: bool,
    ground: bool,
    clear: bool,
    undo: bool,
}

pub(crate) fn show(app: &mut PhotocraftApp, ctx: &egui::Context) {
    if app.ui.tool != Tool::ObjectSelection || !app.ui.sam3_panel || app.session.active().is_none() {
        return;
    }
    let t = Tokens::get(ctx);
    let canvas = app.last_canvas_rect;
    let width = 320.0;
    let mut open = true;
    let (positive, negative, has_box) = {
        let b = &app.sam3.builder;
        (b.positive.len(), b.negative.len(), b.box_.is_some())
    };
    let task_status = app.sam3.task_status().map(|(message, warning, busy)| (message.to_owned(), warning, busy));
    let can_submit = app.sam3.can_submit();
    let mut mode = app.sam3.builder.mode;
    let mut text = app.sam3.builder.text.clone();
    let mut act = Actions::default();
    egui::Window::new(tl!(Tool::ObjectSelection.label()))
        .id(egui::Id::new("sam3-panel"))
        .title_bar(false)
        .collapsible(false)
        .resizable(false)
        .movable(!app.session.prefs().workspace_locked)
        .min_width(width)
        .max_width(width)
        .default_pos(pos2((canvas.right() - width - 16.0).max(canvas.left() + 8.0), canvas.top() + 8.0))
        .show(ctx, |ui| {
            ui.set_width(width);
            ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
            ui.spacing_mut().interact_size.y = 28.0;
            ui.style_mut().override_font_id = Some(egui::FontId::proportional(13.0));
            // Inputs lead; the close affordance shares the Mode row rather than a title strip.
            ui.horizontal(|ui| {
                field_label(ui, tl!("Mode"), &t);
                let mode_width = (ui.available_width() - 22.0 - ui.spacing().item_spacing.x).max(80.0);
                egui::ComboBox::from_id_salt("sam3-mode").selected_text(mode_label(mode)).width(mode_width).show_ui(ui, |ui| {
                    for m in [Mode::Replace, Mode::Add, Mode::Subtract, Mode::Intersect] {
                        ui.selectable_value(&mut mode, m, mode_label(m));
                    }
                });
                if crate::icons::button(ui, "x", 22.0, false, tl!("Close")).clicked() {
                    open = false;
                }
            });
            let field = ui.horizontal(|ui| {
                field_label(ui, tl!("Text"), &t);
                let prompt_width = (ui.available_width() - 56.0 - ui.spacing().item_spacing.x).max(80.0);
                let response = ui.add_sized(
                    vec2(prompt_width, 28.0),
                    egui::TextEdit::singleline(&mut text).font(egui::FontId::proportional(14.0)).margin(vec2(6.0, 5.0)).hint_text(tl!("person, red car…")),
                );
                let apply = egui::Button::new(RichText::new(tl!("Apply")).font(crate::theme::medium(12.0)))
                    .fill(t.field)
                    .stroke(egui::Stroke::new(1.0, t.accent))
                    .corner_radius(t.radius_sm);
                let has_prompt = !text.trim().is_empty() || positive + negative > 0 || has_box;
                act.commit |= ui.add_enabled_ui(can_submit && has_prompt, |ui| ui.add_sized(vec2(56.0, 28.0), apply)).inner.clicked();
                response
            });
            // Keyboard and inline Apply submit the same prompt path.
            if field.inner.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                act.commit = true;
            }
            ui.scope(|ui| {
                ui.spacing_mut().interact_size.y = 20.0;
                ui.horizontal(|ui| {
                    ui.add_space(44.0);
                    act.ground |= ui.add_enabled_ui(can_submit, |ui| quiet_button(ui, tl!("Select Subject"), &t)).inner.clicked();
                    act.clear |= quiet_button(ui, tl!("Clear"), &t).clicked();
                    act.undo |= quiet_button(ui, tl!("Undo"), &t).clicked();
                });
            });
            if task_status.is_none() && (positive + negative > 0 || has_box) {
                let boxed = if has_box { "  ☐" } else { "" };
                ui.label(RichText::new(format!("{positive} +   {negative} −{boxed}")).size(11.0).color(t.text_dim));
            } else if task_status.is_none() {
                ui.label(RichText::new(tl!("Click objects to add points. Alt-click excludes.")).size(11.0).color(t.text_dim));
            }
            if let Some((message, warning, busy)) = &task_status {
                ui.horizontal_wrapped(|ui| {
                    if *busy {
                        ui.add(egui::Spinner::new().size(14.0));
                    }
                    ui.label(RichText::new(message).size(12.0).color(if *warning { t.warning } else { t.text }));
                });
            }
        });
    if !open {
        crate::sam3_selection::Runtime::panel_closed(app);
    }
    app.sam3.builder.mode = mode;
    app.sam3.builder.text = text.clone();
    // Apply submits typed text; without text it commits the canvas points or box.
    if act.commit && !text.trim().is_empty() {
        act.commit = false;
        act.ground = true;
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

/// Right-aligned field label in a fixed-width column, like the other Properties rows.
fn field_label(ui: &mut egui::Ui, s: &str, t: &Tokens) {
    let (r, response) = ui.allocate_exact_size(vec2(44.0, 28.0), Sense::hover());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, true, s));
    ui.painter().text(pos2(r.right() - 4.0, r.center().y), Align2::RIGHT_CENTER, s, crate::theme::medium(13.0), t.text);
}

fn quiet_button(ui: &mut egui::Ui, label: &str, t: &Tokens) -> egui::Response {
    ui.add(egui::Button::new(RichText::new(label).font(egui::FontId::proportional(11.5)).color(t.text_dim)).frame(false))
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
