//! The History panel's footer buttons (Photoshop theme): Delete current state and Create new
//! document from current state. Both dispatch engine commands (`history.deleteState`,
//! `image.duplicate`); nothing here edits a document itself.
//!
//! "Create new snapshot" is not offered: History has no snapshot feature yet, and a button that
//! discards its click is worse than none (#1117).
//!
//! An open Free Transform owns Undo (`transform_tool::intercept`), so Delete current state waits
//! for it exactly as clicking a History row does.

use serde_json::json;

use crate::{PhotocraftApp, icons, widgets};

/// Draw the footer and run the clicked command.
pub(crate) fn show(app: &mut PhotocraftApp, ui: &mut egui::Ui) {
    let can_delete = app.ui.transform.is_none() && app.session.active().is_some_and(|d| d.history.can_undo());
    let can_copy = app.session.active().is_some();
    let mut run: Option<&str> = None;
    widgets::panel_footer(ui, |ui| {
        ui.add_enabled_ui(can_delete, |ui| {
            let response = icons::button(ui, "trash", 26.0, false, tl!("Delete current state"));
            response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, can_delete, tl!("Delete current state")));
            if response.clicked() {
                run = Some("history.deleteState");
            }
        });
        ui.add_enabled_ui(can_copy, |ui| {
            let response = icons::button(ui, "file-plus", 26.0, false, tl!("Create new document from current state"));
            response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, can_copy, tl!("Create new document from current state")));
            if response.clicked() {
                run = Some("image.duplicate");
            }
        });
    });
    if let Some(cmd) = run {
        // A refused command reports itself through the app's notice; there is nothing more to do here.
        let _ = app.run(cmd, json!({}));
    }
}

#[cfg(test)]
mod tests {
    use egui::vec2;
    use egui_kittest::{Harness, kittest::Queryable};

    use super::*;

    fn app_with_steps(steps: usize) -> PhotocraftApp {
        let mut app = PhotocraftApp::new(photocraft_engine::Session::new(), crate::Services::default());
        app.run("file.new", json!({"width": 120, "height": 90})).unwrap();
        for _ in 0..steps {
            app.run("layer.new.layer", json!({})).unwrap();
        }
        app
    }

    fn harness(app: PhotocraftApp) -> Harness<'static, PhotocraftApp> {
        let mut h = Harness::builder().with_size(vec2(300.0, 120.0)).build_ui_state(|ui, app: &mut PhotocraftApp| show(app, ui), app);
        h.run_steps(2);
        h
    }

    fn entries(h: &Harness<'static, PhotocraftApp>) -> usize {
        h.state().session.active().unwrap().history.entries().len()
    }

    #[test]
    fn delete_current_state_removes_the_last_step() {
        let mut h = harness(app_with_steps(2));
        let (steps, layers) = (entries(&h), h.state().session.active().unwrap().doc.layers.len());
        h.get_by_label("Delete current state").click();
        h.run_steps(2);
        assert_eq!(entries(&h), steps - 1);
        assert_eq!(h.state().session.active().unwrap().doc.layers.len(), layers - 1);
        assert!(!h.state().session.active().unwrap().history.can_redo(), "a deleted state cannot be redone");
    }

    #[test]
    fn delete_current_state_waits_for_an_open_free_transform() {
        let mut app = app_with_steps(2);
        app.run("select.rect", json!({"x": 10, "y": 10, "width": 40, "height": 30})).unwrap();
        app.run("edit.fill", json!({"color": "#ff0000"})).unwrap();
        let mut h = harness(app);
        let ctx = h.ctx.clone();
        crate::transform_tool::begin(h.state_mut(), &ctx).unwrap();
        h.run_steps(2);
        let steps = entries(&h);
        h.get_by_label("Delete current state").click();
        h.run_steps(2);
        assert_eq!(entries(&h), steps, "the transform owns Undo, so the click does nothing");
        assert!(h.state().ui.transform.is_some());
    }

    #[test]
    fn delete_current_state_does_nothing_without_history() {
        let mut h = harness(app_with_steps(0));
        let steps = entries(&h);
        h.get_by_label("Delete current state").click();
        h.run_steps(2);
        assert_eq!(entries(&h), steps);
        assert_eq!(h.state().session.documents().len(), 1);
    }

    #[test]
    fn new_document_from_current_state_opens_a_copy() {
        let mut h = harness(app_with_steps(2));
        let layers = h.state().session.active().unwrap().doc.layers.len();
        h.get_by_label("Create new document from current state").click();
        h.run_steps(2);
        assert_eq!(h.state().session.documents().len(), 2);
        let copy = h.state().session.active().unwrap();
        assert_eq!(copy.doc.layers.len(), layers, "the copy has the current state's layers");
        assert!(copy.doc.name.ends_with("copy"), "{}", copy.doc.name);
    }

    #[test]
    fn there_is_no_dead_snapshot_button() {
        let h = harness(app_with_steps(1));
        assert!(h.query_by_label("Create new snapshot").is_none());
    }

    #[test]
    fn no_document_leaves_both_buttons_disabled() {
        let app = PhotocraftApp::new(photocraft_engine::Session::new(), crate::Services::default());
        let mut h = harness(app);
        h.get_by_label("Delete current state").click();
        h.get_by_label("Create new document from current state").click();
        h.run_steps(2);
        assert!(h.state().session.documents().is_empty());
    }
}
