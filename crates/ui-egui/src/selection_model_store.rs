//! Shared-store operations for the model picker. Called only on a worker, never while drawing.
use craft_ai_client::{AiClient, ToolDesk};
use craft_ai_core::{AppId, ConnectionId, ModelId};
use craft_ai_native::secrets::MemoryStore;
use craft_ai_native::shared::SharedStore;
use std::sync::Arc;

fn client(shared: &SharedStore, app: &str) -> Result<AiClient, String> {
    Ok(AiClient::new(AppId::new(app).map_err(|e| e.to_string())?, shared.clone(), Arc::new(MemoryStore::new())))
}

pub(crate) fn supported(provider: &str, model: &str) -> Option<crate::sam3_route::SelectionModel> {
    match (provider, model) {
        ("comfyui", "sam3-segment") => Some(crate::sam3_route::SelectionModel::Sam3),
        ("comfyui", "sam21-segment") => Some(crate::sam3_route::SelectionModel::Sam21),
        _ => None,
    }
}

pub(crate) fn choose_saved(shared: &SharedStore, connection: &str, model: &str) -> Result<(), String> {
    let connection = ConnectionId::new(connection).map_err(|e| e.to_string())?;
    let model = ModelId::new(model).map_err(|e| e.to_string())?;
    let role = crate::sam3_route::select_subject_role()?;
    let app = client(shared, "photocraft")?;
    let choice = app
        .choices(&role)
        .map_err(|e| e.message)?
        .into_iter()
        .find(|c| c.connection == connection && c.model == model)
        .ok_or("That model is no longer available. Refresh the model list.")?;
    let selected = supported(&choice.provider, model.as_str())
        .filter(|_| choice.locality == craft_ai_core::Locality::Local)
        .ok_or("This app cannot use that model yet.")?;
    let store = shared.load().map_err(|e| e.to_string())?;
    let url = store.connections.iter().find(|c| c.id == connection).and_then(|c| c.endpoint.as_deref()).ok_or("That model is not fully set up.")?;
    match selected {
        crate::sam3_route::SelectionModel::Sam3 => {
            craft_ai_comfyui::Sam3Session::new(url).map_err(|e| e.message)?;
        }
        crate::sam3_route::SelectionModel::Sam21 => {
            craft_ai_comfyui::Sam21Session::new(url).map_err(|e| e.message)?;
        }
    }
    let desk = ToolDesk::new(client(shared, "launcher")?, shared.clone(), None);
    desk.choose(&role, &connection, &model).map_err(|e| e.message)?;
    let app = client(shared, "photocraft")?;
    app.clear_binding(&role.id).map_err(|e| e.message)?;
    let route = app.local_route_for(&role).map_err(|e| e.message)?.ok_or("The model choice was not saved")?;
    if route.connection != connection || route.model != model {
        return Err("The model choice changed. Try again.".into());
    }
    Ok(())
}

fn check_files(recipe: &craft_ai_features::LocalRecipe, install: &craft_ai_comfy_env::ComfyInstall) -> Result<(), String> {
    let inventory = craft_ai_comfy_env::scan(install);
    for file in &recipe.models {
        let found = inventory.find(file).ok_or("The model files are not in place. Set up this model first.")?;
        let checked = install.model_dirs.iter().filter(|(kind, _)| kind.eq_ignore_ascii_case(&file.folder)).any(|(_, dir)| {
            let path = dir.join(&found.filename);
            !path.is_symlink() && craft_ai_comfy_env::verify_file(&path, file, &std::sync::atomic::AtomicBool::new(false), &mut |_| {}).is_ok()
        });
        if !checked {
            return Err("The model files did not pass the check. Nothing was changed.".into());
        }
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Target {
    Saved { connection: String, model: String },
    Local { workflow: String },
}
#[derive(Clone, Debug)]
pub(crate) struct OptionEntry {
    pub label: String,
    pub target: Target,
    pub reason: Option<String>,
}
#[derive(Clone, Debug, Default)]
pub(crate) struct Snapshot {
    pub options: Vec<OptionEntry>,
    pub selected: Option<Target>,
    pub label: Option<String>,
    pub route: Option<craft_ai_client::LocalRoute>,
}

pub(crate) fn snapshot(shared: &SharedStore, install: Option<&craft_ai_comfy_env::ComfyInstall>) -> Result<Snapshot, String> {
    let app = client(shared, "photocraft")?;
    let role = crate::sam3_route::select_subject_role()?;
    let choices = app.choices(&role).map_err(|e| e.message)?;
    let store = shared.load().map_err(|e| e.to_string())?;
    let resolved = store.resolve(app.app(), &role).ok();
    let selected = resolved.as_ref().map(|r| Target::Saved { connection: r.connection.id.to_string(), model: r.model.id.to_string() });
    let label = resolved.as_ref().map(|r| r.model.label.clone());
    let mut options: Vec<_> = choices
        .iter()
        .map(|c| {
            let reason = (supported(&c.provider, c.model.as_str()).is_none() || c.locality != craft_ai_core::Locality::Local)
                .then(|| "Not available in this app yet".into());
            let duplicates = choices.iter().filter(|other| other.model_label == c.model_label).count() > 1;
            OptionEntry {
                label: if duplicates { format!("{} on {}", c.model_label, c.connection_label) } else { c.model_label.clone() },
                target: Target::Saved { connection: c.connection.to_string(), model: c.model.to_string() },
                reason,
            }
        })
        .collect();
    let feature = subject_feature()?;
    let inventory = install.map(craft_ai_comfy_env::scan).unwrap_or_default();
    let plans = craft_ai_features::plans(&feature, &inventory);
    for recipe in &feature.locals {
        if choices.iter().any(|c| c.model.as_str() == recipe.workflow && c.provider == "comfyui" && c.locality == craft_ai_core::Locality::Local) {
            continue;
        }
        let reason = if supported("comfyui", &recipe.workflow).is_none() {
            Some("Not available in this app yet".into())
        } else if !plans.iter().any(|p| p.recipe == recipe.workflow && p.state == craft_ai_features::State::Ready) {
            Some("Files are not set up".into())
        } else {
            None
        };
        options.push(OptionEntry { label: recipe.label.clone(), target: Target::Local { workflow: recipe.workflow.clone() }, reason });
    }
    let route = resolved.as_ref().map(|r| craft_ai_client::LocalRoute {
        connection: r.connection.id.clone(),
        model: r.model.id.clone(),
        provider: r.connection.provider.clone(),
        endpoint: r.connection.endpoint.clone(),
        file: r.model.local_file.clone(),
    });
    Ok(Snapshot { options, selected, label, route })
}

fn subject_feature() -> Result<craft_ai_features::Feature, String> {
    craft_ai_features::builtin().into_iter().find(|f| f.id == "select-subject").ok_or_else(|| "Select Subject is not available".into())
}

pub(crate) fn choose(shared: &SharedStore, install: Option<&craft_ai_comfy_env::ComfyInstall>, target: &Target) -> Result<Snapshot, String> {
    match target {
        Target::Saved { connection, model } => choose_saved(shared, connection, model)?,
        Target::Local { workflow } => {
            let feature = subject_feature()?;
            let recipe = feature
                .locals
                .iter()
                .find(|r| r.workflow == *workflow && r.usable && supported("comfyui", workflow).is_some())
                .ok_or("This app cannot use that model yet.")?;
            let install = install.ok_or("The model files are not set up on this computer.")?;
            let inventory = craft_ai_comfy_env::scan(install);
            if !craft_ai_features::plans(&feature, &inventory).iter().any(|p| p.recipe == *workflow && p.state == craft_ai_features::State::Ready) {
                return Err("The model files are not set up on this computer.".into());
            }
            check_files(recipe, install)?;
            let desk = ToolDesk::new(client(shared, "launcher")?, shared.clone(), None);
            let enabled = desk.enable_local_recipe(&feature, workflow, crate::sam3_route::DEFAULT_URL).map_err(|e| e.message)?;
            choose_saved(shared, enabled.connection.as_str(), enabled.model.as_str())?;
        }
    }
    snapshot(shared, install)
}

#[cfg(test)]
#[path = "selection_model_store_tests.rs"]
mod tests;
