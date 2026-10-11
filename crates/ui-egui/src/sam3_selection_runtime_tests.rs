//! Runtime behaviour of the Object Selection prompt: accumulation, box, commit, text, panel,
//! plus two opt-in tests that drive the whole app runtime against the local headless ComfyUI.
use super::*;
use photocraft_engine::Session;
use serde_json::json;
use std::time::{Duration, Instant};

fn ready_runtime(app: &mut crate::PhotocraftApp) -> Runtime {
    app.session.execute("file.new", json!({"width":100,"height":100})).unwrap();
    app.ui.tool = Tool::ObjectSelection;
    let (task_tx, _task_rx) = mpsc::channel();
    let (_result_tx, result_rx) = mpsc::channel();
    let mut runtime = Runtime { tasks: Some(task_tx), results: Some(result_rx), ..Default::default() };
    runtime.tick(app, &egui::Context::default());
    runtime.ready = true;
    runtime.model_size = Some([100, 100]);
    runtime
}

#[test]
fn clicks_accumulate_alt_excludes_and_undo_removes_newest_first() {
    let mut app = crate::PhotocraftApp::new(Session::new(), crate::Services::default());
    let mut runtime = ready_runtime(&mut app);
    assert!(runtime.click([10.0, 10.0], false));
    assert!(runtime.click([50.0, 50.0], false));
    assert!(runtime.click([90.0, 90.0], true));
    assert_eq!((runtime.builder.positive.len(), runtime.builder.negative.len()), (2, 1));
    runtime.undo_point();
    assert_eq!((runtime.builder.positive.len(), runtime.builder.negative.len()), (2, 0));
    runtime.undo_point();
    assert_eq!(runtime.builder.positive, vec![[10.0, 10.0]]);
    runtime.clear_prompt();
    assert!(runtime.builder.to_prompt([100, 100], [100, 100]).is_none());
}

#[test]
fn box_drag_sets_the_box_and_a_tiny_drag_does_not() {
    let mut app = crate::PhotocraftApp::new(Session::new(), crate::Services::default());
    let mut runtime = ready_runtime(&mut app);
    assert!(!runtime.box_drag([5.0, 5.0], [5.2, 5.2]));
    assert!(runtime.box_drag([60.0, 70.0], [10.0, 20.0]));
    assert_eq!(runtime.builder.box_, Some(([10.0, 20.0], [60.0, 70.0])));
}

#[test]
fn commit_queues_the_whole_prompt_in_the_chosen_mode_and_blocks_a_second() {
    let mut app = crate::PhotocraftApp::new(Session::new(), crate::Services::default());
    app.session.execute("file.new", json!({"width":100,"height":100})).unwrap();
    app.ui.tool = Tool::ObjectSelection;
    let (task_tx, task_rx) = mpsc::channel();
    let mut runtime = Runtime { tasks: Some(task_tx), ..Default::default() };
    runtime.tick(&mut app, &egui::Context::default());
    let _prepare = task_rx.try_recv();
    runtime.ready = true;
    runtime.model_size = Some([50, 50]);
    assert!(!runtime.commit([100, 100]), "an empty prompt never reaches the model");
    runtime.click([20.0, 40.0], false);
    runtime.click([80.0, 80.0], true);
    runtime.builder.mode = crate::sam3_prompts::Mode::Subtract;
    assert!(runtime.commit([100, 100]));
    match task_rx.try_recv() {
        Ok(Job::Commit { prompt, mode, .. }) => {
            assert_eq!(mode, "subtract");
            assert_eq!(prompt.positive_points, vec![(10.0, 20.0)]);
            assert_eq!(prompt.negative_points, vec![(40.0, 40.0)]);
        }
        _ => panic!("expected one Commit job"),
    }
    assert!(!runtime.commit([100, 100]), "one selection job at a time");
}

#[test]
fn select_subject_sends_the_panel_text_and_empty_text_means_find_a_subject() {
    let mut app = crate::PhotocraftApp::new(Session::new(), crate::Services::default());
    app.session.execute("file.new", json!({"width":10,"height":10})).unwrap();
    app.ui.tool = Tool::ObjectSelection;
    let (task_tx, task_rx) = mpsc::channel();
    let mut runtime = Runtime { tasks: Some(task_tx), ..Default::default() };
    runtime.tick(&mut app, &egui::Context::default());
    let _prepare = task_rx.try_recv();
    assert!(!runtime.ground("person"), "not before the image is prepared");
    runtime.ready = true;
    assert!(runtime.ground("  red car "));
    assert!(matches!(task_rx.try_recv(), Ok(Job::Ground { text, .. }) if text == "red car"));
    runtime.click_busy = false;
    assert!(runtime.ground(""));
    assert!(matches!(task_rx.try_recv(), Ok(Job::Ground { text, .. }) if text.is_empty()));
}

#[test]
fn closing_the_panel_sticks_until_the_tool_is_reselected() {
    let mut app = crate::PhotocraftApp::new(Session::new(), crate::Services::default());
    app.session.execute("file.new", json!({"width":10,"height":10})).unwrap();
    let mut runtime = Runtime::default();
    let ctx = egui::Context::default();
    app.ui.tool = Tool::ObjectSelection;
    runtime.tick(&mut app, &ctx);
    assert!(app.ui.sam3_panel, "opens with the tool");
    Runtime::panel_closed(&mut app);
    runtime.tick(&mut app, &ctx);
    assert!(!app.ui.sam3_panel, "stays closed");
    app.ui.tool = Tool::Move;
    runtime.tick(&mut app, &ctx);
    app.ui.tool = Tool::ObjectSelection;
    runtime.tick(&mut app, &ctx);
    assert!(app.ui.sam3_panel, "reselecting the tool brings it back");
}

fn paint(app: &mut crate::PhotocraftApp, w: i32, h: i32, px: impl Fn(i32, i32) -> [f32; 3]) {
    app.session
        .edit("Synthetic scene", |doc, _| {
            let surface =
                doc.layers.first_mut().and_then(|l| l.surface_mut()).ok_or_else(|| photocraft_engine::EngineError::Other("missing test layer".into()))?;
            let channels = surface.channels();
            let mut data = Vec::with_capacity((w * h) as usize * channels);
            for y in 0..h {
                for x in 0..w {
                    data.extend_from_slice(&px(x, y)[..channels.min(3)]);
                    if channels == 4 {
                        data.push(1.0);
                    }
                }
            }
            surface.write_region(photocraft_geom::Rect::new(0, 0, w, h), &data);
            Ok(())
        })
        .unwrap();
}

fn wait_until(app: &mut crate::PhotocraftApp, runtime: &mut Runtime, done: impl Fn(&crate::PhotocraftApp, &Runtime) -> bool) {
    let ctx = egui::Context::default();
    let deadline = Instant::now() + Duration::from_secs(100);
    while !done(app, runtime) && runtime.error.is_none() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
        runtime.tick(app, &ctx);
    }
}

#[test]
#[ignore = "requires the existing headless ComfyUI server and SAM 3 model"]
fn two_clicks_shape_one_selection_through_the_app_runtime() {
    let mut app = crate::PhotocraftApp::new(Session::new(), crate::Services::default());
    app.session.execute("file.new", json!({"width":256,"height":128,"depth":8})).unwrap();
    paint(&mut app, 256, 128, |x, y| {
        let big = (x - 64).pow(2) + (y - 64).pow(2) < 50 * 50;
        let small = (x - 190).pow(2) + (y - 64).pow(2) < 30 * 30;
        if big || small { [0.27; 3] } else { [1.0; 3] }
    });
    let mut runtime = Runtime::default();
    app.ui.tool = Tool::ObjectSelection;
    runtime.tick(&mut app, &egui::Context::default());
    wait_until(&mut app, &mut runtime, |_, r| r.ready);
    assert!(runtime.ready, "{}", app.ui.status);
    assert!(runtime.click([64.0, 64.0], false));
    assert!(runtime.click([190.0, 64.0], false));
    assert!(runtime.commit([256, 128]));
    wait_until(&mut app, &mut runtime, |a, _| a.session.active().is_some_and(|s| s.doc.selection.is_some()));
    assert!(!app.ui.status_error, "{}", app.ui.status);
    let sel = app.session.active().unwrap().doc.selection.as_ref().unwrap();
    assert!(sel.pixel(64, 64)[0] > 0.5, "first disc");
    assert!(sel.pixel(190, 64)[0] > 0.5, "second disc reached by the second point");
    assert_eq!(sel.pixel(10, 10)[0], 0.0, "background stays out");
}

#[test]
#[ignore = "requires the existing headless ComfyUI server and SAM 3 model"]
fn select_subject_text_finds_the_named_object_through_the_app_runtime() {
    let mut app = crate::PhotocraftApp::new(Session::new(), crate::Services::default());
    app.session.execute("file.new", json!({"width":320,"height":240,"depth":8})).unwrap();
    paint(&mut app, 320, 240, |x, y| {
        if (60..100).contains(&x) && (170..220).contains(&y) { [0.7, 0.24, 0.2] } else { [0.5 + x as f32 / 1280.0, 0.6 + y as f32 / 960.0, 0.75] }
    });
    let mut runtime = Runtime::default();
    app.ui.tool = Tool::ObjectSelection;
    runtime.tick(&mut app, &egui::Context::default());
    wait_until(&mut app, &mut runtime, |_, r| r.ready);
    assert!(runtime.ground("red box"));
    wait_until(&mut app, &mut runtime, |a, _| a.session.active().is_some_and(|s| s.doc.selection.is_some()));
    assert!(!app.ui.status_error, "{}", app.ui.status);
    let sel = app.session.active().unwrap().doc.selection.as_ref().unwrap();
    assert!(sel.pixel(80, 195)[0] > 0.5, "the red box is selected");
    assert_eq!(sel.pixel(250, 40)[0], 0.0, "the sky is not");
}

#[test]
fn hover_preview_follows_the_cursor_one_request_at_a_time() {
    let mut app = crate::PhotocraftApp::new(Session::new(), crate::Services::default());
    app.session.execute("file.new", json!({"width":100,"height":100})).unwrap();
    app.ui.tool = Tool::ObjectSelection;
    let (task_tx, task_rx) = mpsc::channel();
    let mut runtime = Runtime { tasks: Some(task_tx), ..Default::default() };
    let ctx = egui::Context::default();
    runtime.tick(&mut app, &ctx);
    let _prepare = task_rx.try_recv();
    runtime.ready = true;
    runtime.model_size = Some([100, 100]);
    let preview_at = |rx: &mpsc::Receiver<Job>| match rx.try_recv() {
        Ok(Job::Preview { prompt, .. }) => Some(prompt.positive_points),
        _ => None,
    };
    runtime.hover([100, 100], Some([10.0, 10.0]), &ctx);
    assert_eq!(preview_at(&task_rx), Some(vec![(10.0, 10.0)]), "first position is sent");
    runtime.hover([100, 100], Some([60.0, 60.0]), &ctx);
    assert_eq!(preview_at(&task_rx), None, "nothing queues behind an in-flight preview");
    runtime.hover_busy = false; // the reply arrived
    runtime.hover([100, 100], Some([60.0, 60.0]), &ctx);
    assert_eq!(preview_at(&task_rx), Some(vec![(60.0, 60.0)]), "the newest position follows, not the stale one");
    runtime.hover_busy = false;
    runtime.hover([100, 100], Some([60.0, 60.0]), &ctx);
    assert_eq!(preview_at(&task_rx), None, "an unchanged prompt is not re-sent");
    runtime.click([60.0, 60.0], false);
    runtime.hover([100, 100], Some([60.0, 60.0]), &ctx);
    assert_eq!(preview_at(&task_rx), Some(vec![(60.0, 60.0), (60.0, 60.0)]), "a click makes the preview stale and it is refreshed");
}

#[test]
fn select_subject_command_uses_sam3_under_the_tool_and_the_old_algorithm_when_it_has_failed() {
    let mut app = crate::PhotocraftApp::new(Session::new(), crate::Services::default());
    app.session.execute("file.new", json!({"width":10,"height":10})).unwrap();
    app.ui.tool = Tool::ObjectSelection;
    let (task_tx, task_rx) = mpsc::channel();
    let key = Key { doc_id: 1, revision: 1, generation: 1 };
    app.sam3 = Runtime { tasks: Some(task_tx), key: Some(key), ready: true, ..Default::default() };
    app.sam3.builder.text = "red car".into();
    let reply = app.run("select.subject", json!({})).unwrap();
    assert_eq!(reply["queued"], "sam3");
    assert!(matches!(task_rx.try_recv(), Ok(Job::Ground { text, .. }) if text == "red car"));
    // Busy: still handled by SAM 3 (a status, not a surprise second algorithm).
    assert_eq!(app.run("select.subject", json!({})).unwrap()["queued"], "sam3");
    assert!(task_rx.try_recv().is_err());
    // A failed SAM 3 falls back to the engine command.
    app.sam3.error = Some("down".into());
    let fallback = app.run("select.subject", json!({}));
    assert!(!matches!(fallback, Ok(ref v) if v.get("queued").is_some()));
}
