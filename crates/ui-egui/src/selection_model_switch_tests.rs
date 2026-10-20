use super::super::*;
use super::*;
use crate::sam3_route::model_store::{OptionEntry, Snapshot, Target};

#[test]
fn changing_models_discards_old_results_and_prepares_a_new_generation() {
    let mut app = crate::PhotocraftApp::new(photocraft_engine::Session::new(), Default::default());
    app.session.execute("file.new", json!({"width":8,"height":8})).unwrap();
    app.ui.tool = Tool::ObjectSelection;
    let (tasks, jobs) = mpsc::channel();
    let (responses, results) = mpsc::channel();
    let mut runtime = Runtime { tasks: Some(tasks), results: Some(results), ..Default::default() };
    let ctx = egui::Context::default();
    runtime.tick(&mut app, &ctx);
    let _ = jobs.try_recv();
    runtime.ready = true;
    runtime.model_size = Some([8, 8]);
    runtime.builder.text = "person".into();
    runtime.builder.add_point([4.0, 4.0], false);
    let old = runtime.key.unwrap();
    runtime.click_busy = true;
    let (requests, requested) = mpsc::channel();
    let (chosen, choices) = mpsc::channel();
    runtime.model_picker = Picker::with_transport(requests, choices);
    let target = Target::Saved { connection: "comfy".into(), model: "sam21-segment".into() };
    runtime.model_picker.snapshot.options.push(OptionEntry { label: "SAM 2.1".into(), target: target.clone(), reason: None });
    runtime.model_picker.request(target.clone());
    assert!(matches!(requested.try_recv(), Ok(Action::Choose(_))));
    runtime.tick(&mut app, &ctx);
    assert!(runtime.key.is_none(), "an in-flight old-model result must no longer own the document");
    assert!(!runtime.can_submit());
    assert!(runtime.builder.positive.is_empty());
    assert_eq!(runtime.builder.text, "person");
    let image = Image::from_u8(8, 8, ChannelLayout::Gray, vec![255; 64]).unwrap();
    let png = photocraft_codecs::encode(&image, Format::Png, &EncodeOptions::default()).unwrap();
    responses.send(Reply::Grounded { key: old, mode: "replace", mask: Ok(("old model".into(), png)) }).unwrap();
    runtime.tick(&mut app, &ctx);
    assert!(app.session.active().unwrap().doc.selection.is_none());
    chosen.send(Event { result: Ok(Snapshot { selected: Some(target), label: Some("SAM 2.1".into()), ..Default::default() }), changed: true }).unwrap();
    runtime.tick(&mut app, &ctx);
    let new = match jobs.try_recv().unwrap() {
        Job::Prepare { key, .. } => key,
        _ => panic!("new model must prepare the current document"),
    };
    assert_ne!(new.generation, old.generation);
    assert!(!runtime.ready);
    responses
        .send(Reply::Prepared { key: new, size: Ok(PreparedModel { size: [8, 8], model: crate::sam3_route::SelectionModel::Sam21, canceller: None }) })
        .unwrap();
    runtime.tick(&mut app, &ctx);
    assert_eq!(runtime.model_label(), "SAM 2.1");
    assert!(runtime.can_submit());
    assert!(runtime.ground("person"));
    assert!(matches!(jobs.try_recv(),Ok(Job::Ground{key,..}) if key==new));
}

#[test]
fn an_external_shared_choice_refresh_invalidates_the_old_session() {
    let mut app = crate::PhotocraftApp::new(photocraft_engine::Session::new(), Default::default());
    app.session.execute("file.new", json!({"width":8,"height":8})).unwrap();
    app.ui.tool = Tool::ObjectSelection;
    let (tasks, _jobs) = mpsc::channel();
    let mut runtime = Runtime { tasks: Some(tasks), ..Default::default() };
    let ctx = egui::Context::default();
    runtime.tick(&mut app, &ctx);
    runtime.ready = true;
    let old = runtime.key.unwrap();
    let (send, _choices) = mpsc::channel();
    let (reply, receive) = mpsc::channel();
    runtime.model_picker = Picker::with_transport(send, receive);
    reply
        .send(Event {
            changed: false,
            result: Ok(Snapshot {
                selected: Some(Target::Saved { connection: "comfy".into(), model: "sam21-segment".into() }),
                label: Some("SAM 2.1".into()),
                ..Default::default()
            }),
        })
        .unwrap();
    runtime.tick(&mut app, &ctx);
    assert_ne!(runtime.key.unwrap().generation, old.generation);
    assert!(!runtime.ready);
}
