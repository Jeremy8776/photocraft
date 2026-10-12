//! UI contracts for the input-led Object Selection pop-out, using real runtime channels.
use super::*;
use egui_kittest::{Harness, kittest::Queryable};

fn harness(theme: crate::theme::ThemeKind) -> (Harness<'static, crate::PhotocraftApp>, mpsc::Receiver<Job>) {
    let mut app = crate::PhotocraftApp::new(photocraft_engine::Session::new(), Default::default());
    app.session.execute("file.new", json!({"width":100,"height":100})).unwrap();
    app.ui.tool = Tool::ObjectSelection;
    let (tasks, jobs) = mpsc::channel();
    let (_replies, results) = mpsc::channel();
    let mut runtime = Runtime { tasks: Some(tasks), results: Some(results), ..Default::default() };
    runtime.tick(&mut app, &egui::Context::default());
    runtime.ready = true;
    runtime.model_size = Some([100, 100]);
    app.sam3 = runtime;
    assert!(matches!(jobs.try_recv(), Ok(Job::Prepare { .. })));
    let mut h = Harness::builder().with_size(egui::vec2(500.0, 400.0)).build_ui_state(
        |ui, app: &mut crate::PhotocraftApp| {
            if ui.ctx().fonts(|f| f.families().contains(&egui::FontFamily::Name("medium".into()))) {
                crate::sam3_panel::show(app, ui.ctx());
            }
        },
        app,
    );
    crate::PhotocraftApp::setup_context(&h.ctx, theme);
    h.run_steps(4);
    (h, jobs)
}

#[test]
fn prompt_input_leads_with_apply_inline_and_quieter_actions() {
    for theme in [crate::theme::ThemeKind::Pro, crate::theme::ThemeKind::Studio] {
        let (mut h, _) = harness(theme);
        let text = h.get_by_role(egui::accesskit::Role::TextInput).rect();
        let apply = h.get_by_label("Apply").rect();
        assert!((text.center().y - apply.center().y).abs() < 1.0, "{theme:?}: Apply belongs on the text row");
        assert!(apply.left() >= text.right(), "Apply follows the text field");
        assert!(text.width() > apply.width() * 2.0, "the prompt takes more space than its action");
        assert!(text.height() >= 26.0, "readable input height");
        for label in ["Select Subject", "Clear", "Undo"] {
            let action = h.get_by_label(label).rect();
            assert!(action.top() > text.bottom(), "secondary actions sit below the inputs");
            assert!(action.height() < text.height(), "secondary actions are visually smaller");
        }
        if let Ok(dir) = std::env::var("SAM3_PANEL_RENDER_DIR") {
            std::fs::create_dir_all(&dir).unwrap();
            h.render().unwrap().save(std::path::Path::new(&dir).join(format!("sam3-{theme:?}.png"))).unwrap();
        }
    }
}

#[test]
fn apply_is_disabled_without_a_prompt_or_while_preparing_or_busy() {
    use egui_kittest::kittest::NodeT;
    let (mut h, jobs) = harness(crate::theme::ThemeKind::Pro);
    assert!(h.get_by_label("Apply").accesskit_node().is_disabled(), "no empty action");
    h.state_mut().sam3.builder.text = "nose".into();
    h.run_steps(2);
    assert!(!h.get_by_label("Apply").accesskit_node().is_disabled());
    h.state_mut().sam3.ready = false;
    h.run_steps(2);
    assert!(h.get_by_label("Apply").accesskit_node().is_disabled(), "preparing");
    h.state_mut().sam3.ready = true;
    h.state_mut().sam3.click_busy = true;
    h.run_steps(2);
    assert!(h.get_by_label("Apply").accesskit_node().is_disabled(), "busy");
    assert!(jobs.try_recv().is_err());
}

#[test]
fn blank_text_apply_commits_canvas_points() {
    let (mut h, jobs) = harness(crate::theme::ThemeKind::Pro);
    h.state_mut().sam3.builder.text = "  ".into();
    assert!(h.state_mut().sam3.click([20.0, 30.0], false));
    h.run_steps(2);
    h.get_by_label("Apply").click();
    h.run_steps(2);
    assert!(matches!(jobs.try_recv(), Ok(Job::Commit { prompt, .. }) if prompt.positive_points == vec![(20.0, 30.0)]));
}

#[test]
fn enter_submits_the_text_prompt() {
    let (mut h, jobs) = harness(crate::theme::ThemeKind::Pro);
    h.state_mut().sam3.builder.text = "nose".into();
    h.run_steps(2);
    h.get_by_role(egui::accesskit::Role::TextInput).click();
    h.run_steps(1);
    h.key_press(egui::Key::Enter);
    h.run_steps(2);
    assert!(matches!(jobs.try_recv(), Ok(Job::Ground { text, .. }) if text == "nose"));
}

#[test]
fn task_ticker_shows_preparation_and_running_selection_in_the_popout() {
    let (mut h, jobs) = harness(crate::theme::ThemeKind::Pro);
    h.state_mut().sam3.ready = false;
    h.run_steps(2);
    assert!(h.query_all_by_label("Preparing SAM 3…").next().is_some(), "preparation is visible beside the inputs");
    assert!(h.state().sam3.task_status().unwrap().2, "preparing spinner");
    h.state_mut().sam3.ready = true;
    h.state_mut().sam3.builder.text = "shadows".into();
    h.run_steps(2);
    h.get_by_label("Apply").click();
    h.run_steps(2);
    assert!(h.query_all_by_label("Selecting subject with SAM 3…").next().is_some(), "selection work is visible in the pop-out");
    assert!(matches!(jobs.try_recv(), Ok(Job::Ground { text, .. }) if text == "shadows"));
    assert!(h.state().sam3.task_status().unwrap().2, "running spinner");
    h.state_mut().sam3.clear_prompt();
    h.run_steps(2);
    assert!(h.query_all_by_label("Selecting subject with SAM 3…").next().is_some(), "clearing points must not hide an in-flight task");
}

#[test]
fn task_completion_and_empty_results_are_visible_and_clear_with_the_prompt() {
    for (coverage, message, warning) in [(255, "Selected “shadows”", false), (0, "Object Selection found nothing there", true)] {
        let (mut h, _) = harness(crate::theme::ThemeKind::Pro);
        let ctx = h.ctx.clone();
        let image = Image::from_u8(100, 100, ChannelLayout::Gray, vec![coverage; 100 * 100]).unwrap();
        let png = photocraft_codecs::encode(&image, Format::Png, &EncodeOptions::default()).unwrap();
        let (sender, results) = mpsc::channel();
        sender.send(Reply::Grounded { key: h.state().sam3.key.unwrap(), mode: "replace", mask: Ok(("shadows".into(), png)) }).unwrap();
        let app = h.state_mut();
        let mut runtime = std::mem::take(&mut app.sam3);
        runtime.results = Some(results);
        runtime.receive(app, &ctx);
        app.sam3 = runtime;
        h.run_steps(2);
        assert!(h.query_all_by_label(message).next().is_some());
        let status = h.state().sam3.task_status().unwrap();
        assert_eq!((status.1, status.2), (warning, false), "completion stops the spinner");
        h.state_mut().sam3.clear_prompt();
        h.run_steps(2);
        assert!(h.query_all_by_label(message).next().is_none(), "Clear removes the old result");
    }
}

#[test]
fn grounding_failure_is_visible_in_the_popout_and_not_overwritten_by_global_status() {
    for theme in [crate::theme::ThemeKind::Pro, crate::theme::ThemeKind::Studio] {
        let (mut h, _) = harness(theme);
        let ctx = h.ctx.clone();
        let (sender, results) = mpsc::channel();
        let key = h.state().sam3.key.unwrap();
        let error = "Nothing matched “shadows”";
        sender.send(Reply::Grounded { key, mode: "add", mask: Err(error.into()) }).unwrap();
        let app = h.state_mut();
        let mut runtime = std::mem::take(&mut app.sam3);
        runtime.results = Some(results);
        runtime.receive(app, &ctx);
        app.sam3 = runtime;
        assert!(app.ui.status.contains(error), "the real reply currently reports in the global bar");
        app.ui.status = "Unrelated save completed".into();
        h.run_steps(3);
        assert!(h.query_all_by_label(&format!("Select Subject: {error}")).next().is_some(), "{theme:?}: the failure belongs in the tool pop-out");
        assert!(h.query_all_by_label("Unrelated save completed").next().is_none());
    }
}

#[test]
fn inline_apply_submits_text_in_the_selected_mode() {
    for theme in [crate::theme::ThemeKind::Pro, crate::theme::ThemeKind::Studio] {
        for mode in
            [crate::sam3_prompts::Mode::Replace, crate::sam3_prompts::Mode::Add, crate::sam3_prompts::Mode::Subtract, crate::sam3_prompts::Mode::Intersect]
        {
            let (mut h, jobs) = harness(theme);
            h.state_mut().sam3.builder.text = "  nose  ".into();
            h.state_mut().sam3.builder.mode = mode;
            h.run_steps(2);
            h.get_by_label("Apply").click();
            h.run_steps(2);
            assert!(
                matches!(jobs.try_recv(), Ok(Job::Ground { text, mode: selected, .. }) if text == "nose" && selected == mode.as_str()),
                "{theme:?}, {mode:?}: Apply must submit the text prompt"
            );
            assert!(h.state().sam3.click_busy);
            assert!(!h.state().ui.status_error);
            assert!(jobs.try_recv().is_err(), "one job per click");
        }
    }
}
