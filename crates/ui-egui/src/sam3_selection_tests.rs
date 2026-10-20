use super::*;
use photocraft_engine::Session;
use serde_json::json;
use std::time::{Duration, Instant};

#[test]
fn source_capture_is_a_bounded_srgb_png_at_the_document_aspect_ratio() {
    let mut s = Session::new();
    s.execute("file.new", json!({"width": 7, "height": 4, "depth": 16})).unwrap();
    let doc = &s.active().unwrap().doc;
    let (png, size) = source_png(doc).unwrap();
    assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));
    assert_eq!(size, [7, 4]);
    let decoded = photocraft_codecs::decode_as(photocraft_codecs::Format::Png, &png).unwrap();
    assert_eq!(decoded.dimensions(), (7, 4));
}

#[test]
fn point_scaling_uses_the_full_document_and_rejects_outside_clicks() {
    assert_eq!(model_point([4000, 3000], [1000, 750], [2000.0, 1500.0]), Some((500.0, 375.0)));
    assert_eq!(model_point([4000, 3000], [1000, 750], [-1.0, 100.0]), None);
    assert_eq!(model_point([4000, 3000], [1000, 750], [4000.0, 100.0]), None);
}

#[test]
fn selecting_the_tool_queues_one_prepare_and_leaving_invalidates_it() {
    let mut app = crate::PhotocraftApp::new(Session::new(), crate::Services::default());
    app.session.execute("file.new", json!({"width":64,"height":64})).unwrap();
    app.ui.tool = crate::state::Tool::ObjectSelection;
    let (sender, receiver) = std::sync::mpsc::channel();
    let mut runtime = Runtime { tasks: Some(sender), ..Default::default() };
    let ctx = egui::Context::default();
    runtime.tick(&mut app, &ctx);
    assert!(matches!(receiver.try_recv(), Ok(Job::Prepare { .. })));
    runtime.tick(&mut app, &ctx);
    assert!(receiver.try_recv().is_err(), "not one graph per frame");
    app.ui.tool = crate::state::Tool::Move;
    runtime.tick(&mut app, &ctx);
    assert!(runtime.key.is_none());
}

#[test]
fn old_reply_cannot_apply_after_switching_tools_and_reselecting() {
    let mut app = crate::PhotocraftApp::new(Session::new(), crate::Services::default());
    app.session.execute("file.new", json!({"width":4,"height":4})).unwrap();
    let (task_tx, _task_rx) = mpsc::channel();
    let (result_tx, result_rx) = mpsc::channel();
    let mut runtime = Runtime { tasks: Some(task_tx), results: Some(result_rx), ..Default::default() };
    let ctx = egui::Context::default();
    app.ui.tool = Tool::ObjectSelection;
    runtime.tick(&mut app, &ctx);
    let stale = runtime.key.unwrap();
    app.ui.tool = Tool::Move;
    runtime.tick(&mut app, &ctx);
    let image = Image::from_u8(4, 4, ChannelLayout::Gray, vec![255; 16]).unwrap();
    let mask = photocraft_codecs::encode(&image, Format::Png, &EncodeOptions::default()).unwrap();
    result_tx.send(Reply::Prepared { key: stale, size: Ok(PreparedModel { size: [4, 4], model: Default::default(), canceller: None }) }).unwrap();
    result_tx.send(Reply::Committed { key: stale, mode: "replace", mask: Ok(mask) }).unwrap();
    app.ui.tool = Tool::ObjectSelection;
    runtime.tick(&mut app, &ctx);
    assert!(app.session.active().unwrap().doc.selection.is_none());
    assert_ne!(runtime.key, Some(stale));
    assert!(!runtime.ready);
}

#[test]
fn completed_selection_keeps_prepared_image_for_the_next_click() {
    let mut app = crate::PhotocraftApp::new(Session::new(), crate::Services::default());
    app.session.execute("file.new", json!({"width":4,"height":4})).unwrap();
    app.ui.tool = Tool::ObjectSelection;
    let st = app.session.active().unwrap();
    let old = Key { doc_id: st.doc.id.0, revision: st.revision, generation: 1 };
    let image = Image::from_u8(4, 4, ChannelLayout::Gray, vec![255; 16]).unwrap();
    let mask = photocraft_codecs::encode(&image, Format::Png, &EncodeOptions::default()).unwrap();
    let (task_tx, task_rx) = mpsc::channel();
    let (result_tx, result_rx) = mpsc::channel();
    let mut runtime = Runtime {
        tasks: Some(task_tx),
        results: Some(result_rx),
        key: Some(old),
        generation: 1,
        ready: true,
        document_size: Some([4, 4]),
        model_size: Some([4, 4]),
        ..Default::default()
    };
    result_tx.send(Reply::Committed { key: old, mode: "replace", mask: Ok(mask) }).unwrap();
    runtime.tick(&mut app, &egui::Context::default());
    let current = runtime.key.unwrap();
    assert_ne!(old.revision, current.revision);
    assert_eq!(current.revision, app.session.active().unwrap().revision);
    assert!(runtime.ready);
    assert!(matches!(task_rx.try_recv(), Ok(Job::Rekey { from, to }) if from == old && to == current));
}

#[test]
#[ignore = "requires the existing headless ComfyUI server and SAM 3 model"]
fn object_selection_runtime_prepares_on_activation_and_commits_on_click() {
    let mut app = crate::PhotocraftApp::new(Session::new(), crate::Services::default());
    app.session.execute("file.new", json!({"width":256,"height":256,"depth":8})).unwrap();
    app.session
        .edit("Synthetic disc", |doc, _| {
            let surface =
                doc.layers.first_mut().and_then(|l| l.surface_mut()).ok_or_else(|| photocraft_engine::EngineError::Other("missing test layer".into()))?;
            let channels = surface.channels();
            let mut data = Vec::with_capacity(256 * 256 * channels);
            for y in 0i32..256 {
                for x in 0i32..256 {
                    let value = if (x - 128).pow(2) + (y - 128).pow(2) < 70 * 70 { 0.27 } else { 1.0 };
                    data.extend(std::iter::repeat_n(value, channels.min(3)));
                    if channels == 4 {
                        data.push(1.0);
                    }
                }
            }
            surface.write_region(photocraft_geom::Rect::new(0, 0, 256, 256), &data);
            Ok(())
        })
        .unwrap();
    let ctx = egui::Context::default();
    let mut runtime = Runtime::default();
    app.ui.tool = Tool::ObjectSelection;
    runtime.tick(&mut app, &ctx);
    let deadline = Instant::now() + Duration::from_secs(100);
    while !runtime.ready && runtime.error.is_none() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
        runtime.tick(&mut app, &ctx);
    }
    assert!(runtime.ready, "{}", app.ui.status);
    runtime.hover([256, 256], Some([128.0, 128.0]), &ctx);
    while runtime.overlay.is_none() && runtime.error.is_none() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
        runtime.tick(&mut app, &ctx);
    }
    assert!(runtime.overlay.is_some(), "hover preview: {}", app.ui.status);
    assert!(runtime.click([128.0, 128.0], false));
    assert!(runtime.commit([256, 256]));
    while app.session.active().unwrap().doc.selection.is_none() && runtime.error.is_none() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
        runtime.tick(&mut app, &ctx);
    }
    assert!(!app.ui.status_error, "{}", app.ui.status);
    let selection = app.session.active().unwrap().doc.selection.as_ref().unwrap();
    assert!(selection.pixel(128, 128)[0] > 0.5);
    assert_eq!(selection.pixel(0, 0)[0], 0.0);
    assert!(runtime.ready, "second click must reuse the prepared embedding");

    // Verify the actual SAM 3 selection survives PhotoCraft's native document format.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sam3-selection.pcraft");
    app.session.execute("file.saveACopy", json!({"path": path.to_string_lossy()})).unwrap();
    let bytes = std::fs::read(&path).unwrap();
    let reopened = photocraft_io::import("sam3-selection.pcraft", &bytes).unwrap().document;
    let saved_selection = reopened.selection.as_ref().expect("native save must retain the SAM 3 selection");
    assert!(saved_selection.pixel(128, 128)[0] > 0.5);
    assert_eq!(saved_selection.pixel(0, 0)[0], 0.0);
}

#[test]
#[ignore = "requires the existing headless ComfyUI server and SAM 3 model"]
fn photocraft_document_selects_a_disc_through_headless_comfyui() {
    let mut s = Session::new();
    s.execute("file.new", json!({"width": 256, "height": 256, "depth": 8})).unwrap();
    s.edit("Synthetic disc", |doc, _| {
        let surface = doc.layers.first_mut().and_then(|l| l.surface_mut()).ok_or_else(|| photocraft_engine::EngineError::Other("missing test layer".into()))?;
        let channels = surface.channels();
        let mut data = Vec::with_capacity(256 * 256 * channels);
        for y in 0i32..256 {
            for x in 0i32..256 {
                let value = if (x - 128).pow(2) + (y - 128).pow(2) < 70 * 70 { 0.27 } else { 1.0 };
                data.extend(std::iter::repeat_n(value, channels.min(3)));
                if channels == 4 {
                    data.push(1.0);
                }
            }
        }
        surface.write_region(photocraft_geom::Rect::new(0, 0, 256, 256), &data);
        Ok(())
    })
    .unwrap();
    let st = s.active().unwrap();
    let (id, revision) = (st.doc.id.0, st.revision);
    let (image, _) = source_png(&st.doc).unwrap();
    let mut sam3 = craft_ai_comfyui::Sam3Session::new("http://127.0.0.1:8000").unwrap();
    sam3.prepare(&image).unwrap();
    let jpeg = sam3.preview(128.0, 128.0).unwrap();
    assert_eq!(preview_image(&jpeg).unwrap().size, [256, 256]);
    let mask = sam3.commit(128.0, 128.0).unwrap();
    let encoded = base64::engine::general_purpose::STANDARD.encode(mask);
    let result = s.execute("select.sam3Mask", json!({"maskPng":encoded,"docId":id,"revision":revision})).unwrap();
    assert_eq!(result["changed"], true);
    let sel = s.active().unwrap().doc.selection.as_ref().unwrap();
    assert!(sel.pixel(128, 128)[0] > 0.5);
    assert_eq!(sel.pixel(0, 0)[0], 0.0);
    s.execute("edit.undo", json!({})).unwrap();
    assert!(s.active().unwrap().doc.selection.is_none());
}
