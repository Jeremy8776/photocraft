use std::sync::Arc;

use photocraft_ai::{AiError, ErrorKind, MockBackend, MockBehavior, Task};
use photocraft_cms::Builtin;
use photocraft_geom::Rect;
use serde_json::json;

use super::*;
use crate::jobs::JobCtx;

fn session(depth: u32, behavior: MockBehavior) -> (Session, Arc<MockBackend>) {
    let mut s = Session::new();
    s.execute("file.new", json!({"width": 200, "height": 160, "depth": depth})).unwrap();
    s.edit("paint", |doc, _| {
        let bg = doc.layers[0].surface_mut().unwrap();
        let n = bg.channels();
        let mut data = Vec::new();
        for _ in 0..200 * 160 {
            data.extend_from_slice(&[0.2, 0.4, 0.6][..n.min(3)]);
            if n == 4 {
                data.push(1.0);
            }
        }
        bg.write_region(Rect::new(0, 0, 200, 160), &data);
        Ok(())
    })
    .unwrap();
    s.execute("select.rect", json!({"x": 60, "y": 50, "width": 40, "height": 30})).unwrap();
    let mock = Arc::new(MockBackend::new(behavior));
    s.ai.set_backend(Some(mock.clone() as Arc<dyn GenerationBackend>));
    (s, mock)
}

fn red() -> MockBehavior {
    MockBehavior::Fill([255, 0, 0, 255])
}

fn layer_count(s: &Session) -> usize {
    s.active().unwrap().doc.layers.len()
}

fn active_surface(s: &Session) -> Surface {
    let st = s.active().unwrap();
    st.doc.layer(st.active_layer.unwrap()).unwrap().surface().unwrap().clone()
}

fn near(a: f32, b: f32) -> bool {
    (a - b).abs() < 0.02
}

#[test]
fn fill_adds_a_layer_shaped_by_the_selection_at_8_and_16_bits() {
    for depth in [8, 16] {
        let (mut s, mock) = session(depth, red());
        let r = s.execute(CMD, json!({"prompt": "  a red door  "})).unwrap();
        assert_eq!(r["variations"], 1);
        assert_eq!(layer_count(&s), 2, "depth {depth}");
        let surf = active_surface(&s);
        let inside = surf.rgba(80, 65);
        assert!(near(inside[0], 1.0) && near(inside[1], 0.0) && near(inside[3], 1.0), "depth {depth}: {inside:?}");
        assert!(near(surf.rgba(10, 10)[3], 0.0), "outside the selection is clear");
        assert!(near(surf.rgba(59, 65)[3], 0.0), "just outside the left edge");
        // The original is untouched and the selection is kept for another attempt.
        let doc = &s.active().unwrap().doc;
        assert!(near(doc.layers[0].surface().unwrap().rgba(80, 65)[2], 0.6));
        assert!(has_selection(doc));
        let calls = mock.calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].prompt.as_deref(), Some("a red door"));
        assert_eq!(calls[0].task, Task::Fill);
        assert!(calls[0].selected_pixels > 0 && calls[0].width % 8 == 0);
    }
}

#[test]
fn the_whole_command_is_one_undo_step() {
    let (mut s, _) = session(8, red());
    s.execute(CMD, json!({})).unwrap();
    assert_eq!(layer_count(&s), 2);
    s.execute("edit.undo", json!({})).unwrap();
    assert_eq!(layer_count(&s), 1);
    s.execute("edit.redo", json!({})).unwrap();
    assert_eq!(layer_count(&s), 2);
}

#[test]
fn several_variations_stack_up_and_only_the_first_shows() {
    let (mut s, _) = session(8, red());
    let r = s.execute(CMD, json!({"variations": 3})).unwrap();
    assert_eq!(r["variations"], 3);
    let doc = &s.active().unwrap().doc;
    assert_eq!(doc.layers.len(), 4);
    let visible: Vec<_> = doc.layers.iter().skip(1).map(|l| (l.name.clone(), l.visible)).collect();
    assert_eq!(visible, [("Generative Fill 1".to_owned(), true), ("Generative Fill 2".to_owned(), false), ("Generative Fill 3".to_owned(), false)]);
}

#[test]
fn remove_passes_the_task_through() {
    let (mut s, mock) = session(8, MockBehavior::Echo);
    s.execute(CMD, json!({"task": "remove"})).unwrap();
    assert_eq!(mock.calls()[0].task, Task::Remove);
}

#[test]
fn it_says_why_it_is_disabled() {
    let mut s = Session::new();
    assert_eq!(enabled(&s).unwrap_err(), "no document open");
    s.execute("file.new", json!({"width": 50, "height": 50})).unwrap();
    assert!(enabled(&s).unwrap_err().contains("no AI model is connected"));
    let mock: Arc<dyn GenerationBackend> = Arc::new(MockBackend::new(red()));
    s.ai.set_backend(Some(mock));
    assert_eq!(enabled(&s).unwrap_err(), "make a selection first");
    s.execute("select.rect", json!({"x": 0, "y": 0, "width": 10, "height": 10})).unwrap();
    assert!(enabled(&s).is_ok());
}

#[test]
fn unsupported_documents_get_a_clear_error_not_a_guess() {
    let (mut s, mock) = session(8, red());
    s.execute("file.new", json!({"width": 50, "height": 50, "depth": 32})).unwrap();
    s.execute("select.rect", json!({"x": 0, "y": 0, "width": 10, "height": 10})).unwrap();
    let msg = enabled(&s).unwrap_err();
    assert!(msg.contains("8 and 16 bit"), "{msg}");
    assert!(s.execute(CMD, json!({})).is_err());
    assert!(mock.calls().is_empty(), "nothing is sent for a document that cannot be handled");
}

#[test]
fn a_remote_service_is_refused_until_the_caller_says_the_user_agreed() {
    let (mut s, _) = session(8, red());
    let remote = Arc::new(MockBackend::new(red()).remote());
    s.ai.set_backend(Some(remote.clone() as Arc<dyn GenerationBackend>));
    let err = s.execute(CMD, json!({})).unwrap_err().to_string();
    assert!(err.contains("remote service"), "{err}");
    assert!(remote.calls().is_empty());
    assert_eq!(layer_count(&s), 1);
    // Asking for it through the command does not count: only the host can record the agreement.
    let err = s.execute(CMD, json!({"allowRemote": true})).unwrap_err().to_string();
    assert!(err.contains("remote service"), "{err}");
    assert!(remote.calls().is_empty());
    s.ai.grant_remote();
    s.execute(CMD, json!({})).unwrap();
    assert_eq!(remote.calls().len(), 1);
}

#[test]
fn the_agreement_ends_when_the_destination_changes() {
    let (mut s, _) = session(8, red());
    let first = Arc::new(MockBackend::new(red()).remote());
    s.ai.set_backend(Some(first as Arc<dyn GenerationBackend>));
    s.ai.grant_remote();
    assert!(s.ai.remote_granted());
    let second = Arc::new(MockBackend::new(red()).remote());
    s.ai.set_backend(Some(second.clone() as Arc<dyn GenerationBackend>));
    assert!(!s.ai.remote_granted());
    assert!(s.execute(CMD, json!({})).is_err());
    assert!(second.calls().is_empty());
    // Disconnecting also forgets it.
    s.ai.grant_remote();
    s.ai.set_backend(None);
    assert!(!s.ai.remote_granted());
}

#[test]
fn a_failing_service_leaves_the_document_alone_and_the_message_through() {
    let (mut s, _) = session(8, MockBehavior::Fail(AiError::new(ErrorKind::Quota, "out of credits")));
    let before = s.active().unwrap().doc.clone();
    let err = s.execute(CMD, json!({})).unwrap_err().to_string();
    assert!(err.contains("out of credits"), "{err}");
    assert!(Arc::ptr_eq(&before, &s.active().unwrap().doc), "no history step, no change");
}

#[test]
fn a_service_that_breaks_the_contract_is_caught() {
    let (mut s, _) = session(8, MockBehavior::WrongSize);
    let err = s.execute(CMD, json!({})).unwrap_err().to_string();
    assert!(err.contains("wrong size"), "{err}");
    assert_eq!(layer_count(&s), 1);
}

#[test]
fn cancelling_stops_the_work_and_changes_nothing() {
    let (mut s, mock) = session(8, MockBehavior::Slow { steps: 100, step_ms: 20 });
    let ctx = JobCtx::new();
    ctx.cancel();
    s.set_inline_job_ctx(Some(ctx));
    let before = s.active().unwrap().doc.clone();
    let err = s.execute(CMD, json!({})).unwrap_err();
    assert!(matches!(err, EngineError::Cancelled), "{err:?}");
    assert!(Arc::ptr_eq(&before, &s.active().unwrap().doc));
    assert!(mock.calls().len() <= 1);
}

#[test]
fn a_service_that_cannot_do_the_task_is_not_asked() {
    let (mut s, _) = session(8, red());
    let mock = Arc::new(MockBackend::new(red()).refusing(Task::Remove));
    s.ai.set_backend(Some(mock.clone() as Arc<dyn GenerationBackend>));
    let err = s.execute(CMD, json!({"task": "remove"})).unwrap_err().to_string();
    assert!(err.contains("cannot do this"), "{err}");
    assert!(mock.calls().is_empty());
    assert!(s.execute(CMD, json!({"task": "fill"})).is_ok());
}

#[test]
fn the_test_model_can_be_connected_and_disconnected_by_command() {
    let mut s = Session::new();
    s.execute("file.new", json!({"width": 100, "height": 80})).unwrap();
    s.execute("select.rect", json!({"x": 10, "y": 10, "width": 30, "height": 30})).unwrap();
    assert!(enabled(&s).is_err());
    assert_eq!(s.execute("ai.connectMock", json!({"color": [0, 255, 0]})).unwrap()["connected"], "Mock");
    s.execute(CMD, json!({})).unwrap();
    let surf = active_surface(&s);
    let px = surf.rgba(20, 20);
    assert!(near(px[1], 1.0) && near(px[0], 0.0), "{px:?}");
    s.execute("ai.disconnect", json!({})).unwrap();
    assert!(enabled(&s).is_err());
    for bad in [json!({"behavior": "magic"}), json!({"color": [1, 2]}), json!({"color": [1, 2, 300]}), json!({"color": "red"})] {
        assert!(s.execute("ai.connectMock", bad.clone()).is_err(), "{bad}");
    }
}

#[test]
fn cancelling_while_the_service_works_is_reported_as_a_cancel() {
    let (mut s, mock) = session(8, MockBehavior::Slow { steps: 200, step_ms: 10 });
    let ctx = JobCtx::new();
    s.set_inline_job_ctx(Some(ctx.clone()));
    let canceller = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(80));
        ctx.cancel();
    });
    let before = s.active().unwrap().doc.clone();
    let started = std::time::Instant::now();
    let err = s.execute(CMD, json!({})).unwrap_err();
    canceller.join().unwrap();
    assert!(matches!(err, EngineError::Cancelled), "{err:?}");
    assert!(started.elapsed() < std::time::Duration::from_millis(1500), "it stopped well before the 2 s the work would take");
    assert_eq!(mock.calls().len(), 1, "the service had started");
    assert!(Arc::ptr_eq(&before, &s.active().unwrap().doc));
}

#[test]
fn bad_parameters_are_errors() {
    let (mut s, mock) = session(8, red());
    for p in [
        json!({"variations": 0}),
        json!({"variations": 5}),
        json!({"variations": "two"}),
        json!({"variations": -1}),
        json!({"task": "paint"}),
        json!({"prompt": 7}),
    ] {
        assert!(s.execute(CMD, p.clone()).is_err(), "{p}");
    }
    assert!(mock.calls().is_empty());
    assert_eq!(layer_count(&s), 1);
}

#[test]
fn a_selection_that_is_soft_keeps_its_softness() {
    let (mut s, _) = session(8, red());
    // Half-select one pixel inside the rectangle, as a feathered edge would.
    s.edit("soften", |doc, _| {
        let sel = doc.selection.as_mut().unwrap();
        let mut px = vec![0.5; sel.channels()];
        px[0] = 0.5;
        sel.write_pixel(80, 65, &px);
        Ok(())
    })
    .unwrap();
    s.execute(CMD, json!({})).unwrap();
    let surf = active_surface(&s);
    assert!(near(surf.rgba(80, 65)[3], 0.5), "{:?}", surf.rgba(80, 65));
    assert!(near(surf.rgba(81, 65)[3], 1.0));
}

#[test]
fn colours_survive_the_trip_through_srgb_for_a_wide_gamut_document() {
    // A Display P3 document: the service sees sRGB and the layer comes back in P3, so an
    // unchanged picture (the Echo backend) should leave the same colours the document had.
    let (mut s, _) = session(8, MockBehavior::Echo);
    let p3 = Builtin::DisplayP3.profile().to_bytes();
    s.edit("profile", move |doc, _| {
        doc.icc_profile = Some(p3);
        Ok(())
    })
    .unwrap();
    s.execute(CMD, json!({})).unwrap();
    let surf = active_surface(&s);
    let got = surf.rgba(80, 65);
    for (g, want) in got.iter().zip([0.2, 0.4, 0.6, 1.0]) {
        assert!((g - want).abs() < 0.02, "{got:?}");
    }
}
