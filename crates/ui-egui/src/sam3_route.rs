//! Which local address Select subject runs on, read from the suite's one tool-to-model binding.
//!
//! The launcher (or any Craft app) binds the `select-subject` tool to a connection and model once;
//! every app that adopts the role reads the same choice. Object Selection used to open a fixed
//! `127.0.0.1:8000`. Now it asks the shared store first, so a launcher rebinding moves PhotoCraft
//! too.
//!
//! Invariants:
//! - Only an *unbound* tool keeps the old default address (nothing has been chosen yet).
//! - A broken, remote or non-ComfyUI choice is an error with the reason. It never falls back to
//!   the default address, because that would silently run a model the person did not pick.
//! - PhotoCraft only has a ComfyUI adapter here. A choice that needs another backend is refused
//!   by name rather than run through ComfyUI.

use craft_ai_client::AiClient;
use craft_ai_model::{AppId, Role};

/// Where Object Selection looked before the binding existed, kept for an unbound tool only.
pub(crate) const DEFAULT_URL: &str = "http://127.0.0.1:8000";

/// This app's id in the shared store, for per-app overrides.
const APP: &str = "photocraft";

/// The address of the ComfyUI that serves the bound Select subject model.
pub(crate) fn resolve() -> Result<String, String> {
    let app = AppId::new(APP).map_err(|e| e.to_string())?;
    let client = AiClient::standard(app).map_err(|e| e.message)?;
    resolve_with(&client)
}

/// [`resolve`] against a given client, so tests can use a scripted store.
pub(crate) fn resolve_with(client: &AiClient) -> Result<String, String> {
    let role = select_subject_role()?;
    match client.endpoint_for(&role) {
        Ok(Some(url)) => Ok(url),
        Ok(None) => Ok(DEFAULT_URL.to_owned()),
        Err(error) => Err(format!("Select subject cannot run here: {}", error.message)),
    }
}

/// The shared `select-subject` role, as the catalogue defines it.
pub(crate) fn select_subject_role() -> Result<Role, String> {
    craft_ai_features::builtin()
        .into_iter()
        .find(|f| f.id == "select-subject")
        .and_then(|f| f.role())
        .ok_or_else(|| "The Select subject tool is missing from the shared catalogue".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use craft_ai_model::{Auth, Billing, CapabilitySpec, Connection, ConnectionId, Locality, ModelId, ModelInfo, StoreFile};
    use craft_ai_native::secrets::MemoryStore;
    use craft_ai_native::shared::SharedStore;
    use std::sync::Arc;

    fn connection(id: &str, provider: &str, local: bool, endpoint: Option<&str>) -> Connection {
        let role = select_subject_role().unwrap();
        let mut spec = CapabilitySpec::new(role.capability);
        for input in role.inputs {
            spec = spec.accepting(input, 1);
        }
        Connection {
            id: ConnectionId::new(id).unwrap(),
            label: id.to_owned(),
            provider: provider.to_owned(),
            endpoint: endpoint.map(str::to_owned),
            locality: if local { Locality::Local } else { Locality::Remote },
            auth: Auth::None,
            billing: Billing::Free,
            models: vec![ModelInfo::new(ModelId::new("sam3").unwrap(), "SAM 3").with(spec)],
            enabled: true,
            key_header: None,
        }
    }

    struct World {
        _dir: tempfile::TempDir,
        shared: SharedStore,
    }

    impl World {
        fn new(connections: Vec<Connection>) -> Self {
            let dir = tempfile::tempdir().unwrap();
            let shared = SharedStore::new(dir.path().join("connections.json"));
            shared.replace(&StoreFile { connections, ..StoreFile::default() }).unwrap();
            Self { _dir: dir, shared }
        }

        fn client(&self, app: &str) -> AiClient {
            AiClient::new(AppId::new(app).unwrap(), self.shared.clone(), Arc::new(MemoryStore::new()))
        }

        /// The launcher's own action: one suite default for the tool.
        fn bind_default(&self, connection: &str) {
            let role = select_subject_role().unwrap();
            self.shared
                .update(|store| {
                    store.defaults.push(craft_ai_model::Binding {
                        role: role.id,
                        connection: ConnectionId::new(connection).unwrap(),
                        model: ModelId::new("sam3").unwrap(),
                    });
                })
                .unwrap();
        }
    }

    #[test]
    fn an_unbound_tool_keeps_the_old_default_address() {
        let world = World::new(vec![connection("comfy", "comfyui", true, Some("http://127.0.0.1:9100"))]);
        assert_eq!(resolve_with(&world.client("photocraft")).unwrap(), DEFAULT_URL);
    }

    #[test]
    fn the_bound_address_is_used_not_the_default() {
        let world = World::new(vec![connection("comfy", "comfyui", true, Some("http://127.0.0.1:9100"))]);
        world.bind_default("comfy");
        assert_eq!(resolve_with(&world.client("photocraft")).unwrap(), "http://127.0.0.1:9100");
    }

    #[test]
    fn another_app_reads_the_same_binding() {
        let world = World::new(vec![connection("comfy", "comfyui", true, Some("http://127.0.0.1:9100"))]);
        world.bind_default("comfy");
        assert_eq!(resolve_with(&world.client("lightcraft")).unwrap(), "http://127.0.0.1:9100");
    }

    #[test]
    fn this_apps_own_choice_beats_the_suite_default() {
        let world = World::new(vec![
            connection("comfy", "comfyui", true, Some("http://127.0.0.1:9100")),
            connection("comfy-two", "comfyui", true, Some("http://127.0.0.1:9200")),
        ]);
        world.bind_default("comfy");
        let role = select_subject_role().unwrap();
        world.client("photocraft").bind(&role, &ConnectionId::new("comfy-two").unwrap(), &ModelId::new("sam3").unwrap()).unwrap();
        assert_eq!(resolve_with(&world.client("photocraft")).unwrap(), "http://127.0.0.1:9200");
        assert_eq!(resolve_with(&world.client("lightcraft")).unwrap(), "http://127.0.0.1:9100");
    }

    #[test]
    fn a_native_choice_is_refused_by_name_and_not_run_on_the_default_address() {
        let world = World::new(vec![connection("native", "candle", true, None)]);
        world.bind_default("native");
        let error = resolve_with(&world.client("photocraft")).unwrap_err();
        assert!(error.contains("candle"), "{error}");
        assert!(!error.contains("8000"), "{error}");
    }

    #[test]
    fn a_remote_choice_is_refused() {
        let world = World::new(vec![connection("hosted", "comfyui", false, Some("https://example.com"))]);
        world.bind_default("hosted");
        assert!(resolve_with(&world.client("photocraft")).is_err());
    }

    #[test]
    fn a_choice_whose_connection_was_removed_is_an_error_not_unbound() {
        let world = World::new(vec![connection("comfy", "comfyui", true, Some("http://127.0.0.1:9100"))]);
        world.bind_default("comfy");
        world.shared.update(|store| store.connections.clear()).unwrap();
        assert!(resolve_with(&world.client("photocraft")).is_err());
    }
}
