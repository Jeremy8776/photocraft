//! Object Selection owns its pop-out, never a section inside layer/document Properties.
use super::*;
use egui_kittest::Harness;

#[test]
fn object_selection_leaves_the_properties_context_unchanged() {
    for theme in [crate::theme::ThemeKind::Pro, crate::theme::ThemeKind::Studio] {
        for target in ["raster", "background", "adjustment", "mask", "none"] {
            let mut session = photocraft_engine::Session::new();
            session.execute("file.new", json!({"width": 64, "height": 64})).unwrap();
            match target {
                "raster" | "mask" => {
                    session.execute("layer.new.layer", json!({})).unwrap();
                    if target == "mask" {
                        session.execute("layer.layerMask.revealAll", json!({})).unwrap();
                    }
                }
                "adjustment" => {
                    session.execute("layer.newAdjustmentLayer.curves", json!({})).unwrap();
                }
                "none" => session.active_mut().unwrap().active_layer = None,
                _ => {}
            }
            let mut app = PhotocraftApp::new(session, Default::default());
            app.ui.tool = Tool::Move;
            app.ui.mask_target = target == "mask";
            app.ui.panels.properties = true;
            app.last_canvas_rect = Rect::from_min_size(egui::Pos2::ZERO, vec2(800.0, 600.0));
            let mut h = Harness::builder().with_size(vec2(800.0, 600.0)).build_ui_state(
                |ui, app: &mut PhotocraftApp| {
                    if !ui.ctx().fonts(|f| f.families().contains(&egui::FontFamily::Name("semibold".into()))) {
                        return;
                    }
                    if Tokens::get(ui.ctx()).pro {
                        properties_body(app, ui);
                    } else {
                        properties_window(app, ui.ctx());
                    }
                },
                app,
            );
            PhotocraftApp::setup_context(&h.ctx, theme);
            h.run_steps(4);
            let section_titles = |ctx: &egui::Context| crate::props_layout::sections_drawn(ctx).into_iter().map(|(title, _)| title).collect::<Vec<_>>();
            let before = section_titles(&h.ctx);
            h.state_mut().ui.tool = Tool::ObjectSelection;
            h.run_steps(4);
            assert_eq!(section_titles(&h.ctx), before, "{theme:?}, {target}: tool controls must not leak into Properties");
            h.state_mut().ui.tool = Tool::Move;
            h.run_steps(4);
            assert_eq!(section_titles(&h.ctx), before, "{theme:?}, {target}: switching back keeps the original context");
        }
    }
}
