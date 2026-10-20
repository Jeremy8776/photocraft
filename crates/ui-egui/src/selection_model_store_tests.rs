use super::*;
use craft_ai_client::AiClient;
use craft_ai_core::{AppId, Auth, Billing, Binding, CapabilitySpec, Connection, ConnectionId, Locality, ModelId, ModelInfo, StoreFile};
use craft_ai_native::secrets::MemoryStore;
use std::sync::Arc;

fn client(shared: &SharedStore, app: &str) -> AiClient {
    AiClient::new(AppId::new(app).unwrap(), shared.clone(), Arc::new(MemoryStore::new()))
}

fn world() -> (tempfile::TempDir, SharedStore) {
    let dir = tempfile::tempdir().unwrap();
    let shared = SharedStore::new(dir.path().join("connections.json"));
    let role = crate::sam3_route::select_subject_role().unwrap();
    let mut spec = CapabilitySpec::new(role.capability.clone());
    for input in &role.inputs {
        spec = spec.accepting(input.clone(), 1);
    }
    let connection = Connection {
        id: ConnectionId::new("comfy").unwrap(),
        label: "My local models".into(),
        provider: "comfyui".into(),
        endpoint: Some("http://127.0.0.1:9100".into()),
        locality: Locality::Local,
        auth: Auth::None,
        billing: Billing::Free,
        enabled: true,
        key_header: None,
        models: [("sam3-segment", "SAM 3"), ("sam21-segment", "SAM 2.1"), ("sam1-segment", "SAM 1")]
            .into_iter()
            .map(|(id, label)| ModelInfo::new(ModelId::new(id).unwrap(), label).with(spec.clone()))
            .collect(),
    };
    shared
        .replace(&StoreFile {
            connections: vec![connection],
            defaults: vec![Binding { role: role.id, connection: ConnectionId::new("comfy").unwrap(), model: ModelId::new("sam3-segment").unwrap() }],
            ..Default::default()
        })
        .unwrap();
    (dir, shared)
}

#[test]
fn the_floating_dock_choice_is_read_by_this_app_and_another_app() {
    let (_dir, shared) = world();
    let role = crate::sam3_route::select_subject_role().unwrap();
    // A prior app override must not make an explicit shared switch appear to do nothing.
    client(&shared, "photocraft").bind(&role, &ConnectionId::new("comfy").unwrap(), &ModelId::new("sam3-segment").unwrap()).unwrap();
    choose_saved(&shared, "comfy", "sam21-segment").unwrap();
    for app in ["photocraft", "another-craft-app"] {
        let route = crate::sam3_route::selection_with(&client(&shared, app)).unwrap();
        assert_eq!(route.model, crate::sam3_route::SelectionModel::Sam21, "{app}");
        assert_eq!(route.url, "http://127.0.0.1:9100");
    }
    choose_saved(&shared, "comfy", "sam3-segment").unwrap();
    assert_eq!(crate::sam3_route::selection_with(&client(&shared, "photocraft")).unwrap().model, crate::sam3_route::SelectionModel::Sam3);
}

#[test]
fn an_unavailable_model_is_refused_before_changing_any_binding() {
    let (_dir, shared) = world();
    let before = std::fs::read(shared.path()).unwrap();
    assert!(choose_saved(&shared, "comfy", "sam1-segment").is_err());
    assert_eq!(std::fs::read(shared.path()).unwrap(), before);
}

#[test]
fn a_same_size_corrupt_model_is_not_accepted_or_repaired() {
    let dir = tempfile::tempdir().unwrap();
    let models = dir.path().join("models/sam2");
    std::fs::create_dir_all(&models).unwrap();
    let path = models.join("model.safetensors");
    std::fs::write(&path, b"evil").unwrap();
    let install = craft_ai_comfy_env::ComfyInstall::at(dir.path()).unwrap();
    let mut recipe = craft_ai_features::builtin().into_iter().find(|f| f.id == "select-subject").unwrap().locals.remove(1);
    recipe.models.truncate(1);
    let file = &mut recipe.models[0];
    file.filename = "model.safetensors".into();
    file.size_bytes = 4;
    file.sha256 = "770e607624d689265ca6c44884d0807d9b054d23c473c106c72be9de08b7376c".into();
    assert!(check_files(&recipe, &install).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), b"evil");
    std::fs::write(&path, b"good").unwrap();
    assert!(check_files(&recipe, &install).is_ok());
}

#[test]
fn model_list_is_read_only_and_marks_missing_app_support() {
    let (_dir, shared) = world();
    let before = std::fs::read(shared.path()).unwrap();
    let view = snapshot(&shared, None).unwrap();
    assert_eq!(view.label.as_deref(), Some("SAM 3"));
    assert!(view.options.iter().any(|o| matches!(&o.target,Target::Saved{model,..} if model=="sam21-segment") && o.reason.is_none()));
    assert!(view.options.iter().any(|o| matches!(&o.target,Target::Saved{model,..} if model=="sam1-segment") && o.reason.is_some()));
    assert_eq!(std::fs::read(shared.path()).unwrap(), before);
}
