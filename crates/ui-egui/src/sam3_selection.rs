//! Native prepared SAM 3 object selection for the current document.
use crate::{PhotocraftApp, state::Tool};
use base64::Engine;
use photocraft_codecs::{ChannelLayout, EncodeOptions, Format, Image};
use photocraft_doc::Document;
use serde_json::json;
use std::sync::{Arc, mpsc};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Key {
    doc_id: u64,
    revision: u64,
    generation: u64,
}

enum Job {
    Prepare { key: Key, doc: Arc<Document> },
    Preview { key: Key, prompt: craft_ai_comfyui::sam3::Prompt },
    Commit { key: Key, prompt: craft_ai_comfyui::sam3::Prompt, mode: &'static str },
    Ground { key: Key, text: String, mode: &'static str },
    Rekey { from: Key, to: Key },
}

enum Reply {
    Prepared { key: Key, size: Result<PreparedModel, String> },
    Previewed { key: Key, jpeg: Result<Vec<u8>, String> },
    Committed { key: Key, mode: &'static str, mask: Result<Vec<u8>, String> },
    Grounded { key: Key, mode: &'static str, mask: Result<(String, Vec<u8>), String> },
}

struct PreparedModel {
    size: [u32; 2],
    model: crate::sam3_route::SelectionModel,
    canceller: Option<model_session::ModelCanceller>,
}

#[derive(Default)]
pub(crate) struct Runtime {
    pub(crate) model_picker: model_switch::Picker,
    model: crate::sam3_route::SelectionModel,
    canceller: Option<model_session::ModelCanceller>,
    tasks: Option<mpsc::Sender<Job>>,
    results: Option<mpsc::Receiver<Reply>>,
    key: Option<Key>,
    generation: u64,
    ready: bool,
    model_size: Option<[u32; 2]>,
    document_size: Option<[u32; 2]>,
    error: Option<String>,
    /// Tool-owned feedback survives unrelated global status-bar messages.
    task_notice: Option<(String, bool)>,
    overlay: Option<egui::TextureHandle>,
    /// The prompt the in-flight or latest hover preview was asked for; `None` means stale.
    sent: Option<craft_ai_comfyui::sam3::Prompt>,
    hover_busy: bool,
    pending_click: Option<([f64; 2], bool)>,
    click_busy: bool,
    pub(crate) builder: crate::sam3_prompts::Builder,
}

impl Runtime {
    pub(crate) fn tick(&mut self, app: &mut PhotocraftApp, ctx: &egui::Context) {
        if app.ui.tool != Tool::ObjectSelection {
            self.cancel_active();
            self.key = None;
            self.document_size = None;
            self.clear_preview();
            self.pending_click = None;
            self.builder.clear();
            self.task_notice = None;
            app.ui.sam3_panel = false;
            app.ui.sam3_panel_closed = false;
            if app.ui.status == "Preparing selection…" || app.ui.status.starts_with("Click") {
                app.ui.status.clear();
            }
            return;
        }
        Self::follow_panel(app);
        if self.poll_model_picker(ctx) {
            return;
        }
        if let Some(key) = self.key
            && !app.session.active().is_some_and(|st| st.doc.id.0 == key.doc_id && st.revision == key.revision)
        {
            self.cancel_active();
            self.key = None;
            self.clear_preview();
        }
        self.receive(app, ctx);
        let Some(st) = app.session.active() else {
            self.key = None;
            self.clear_preview();
            return;
        };
        if self.key.is_some_and(|k| k.doc_id == st.doc.id.0 && k.revision == st.revision) {
            return;
        }
        self.generation = self.generation.wrapping_add(1);
        let key = Key { doc_id: st.doc.id.0, revision: st.revision, generation: self.generation };
        let doc = Arc::clone(&st.doc);
        self.key = Some(key);
        self.document_size = Some([doc.size.width, doc.size.height]);
        self.ready = false;
        self.model_size = None;
        self.error = None;
        self.task_notice = None;
        self.clear_preview();
        self.pending_click = None;
        self.builder.clear();
        if let Some(hint) = self.hint() {
            app.ui.status = hint.into();
            app.ui.status_error = false;
        }
        if self.tasks.is_none() {
            let (tasks, jobs) = mpsc::channel();
            let (results, receiver) = mpsc::channel();
            let wake = ctx.clone();
            std::thread::spawn(move || worker(jobs, results, wake));
            self.tasks = Some(tasks);
            self.results = Some(receiver);
        }
        if let Some(tasks) = &self.tasks
            && tasks.send(Job::Prepare { key, doc }).is_err()
        {
            self.error = Some("Object Selection worker is unavailable".into());
        }
        ctx.request_repaint();
    }

    pub(crate) fn receive(&mut self, app: &mut PhotocraftApp, ctx: &egui::Context) {
        while let Some(reply) = self.results.as_ref().and_then(|r| r.try_recv().ok()) {
            match reply {
                Reply::Prepared { key, size } if self.key == Some(key) => match size {
                    Ok(size) => {
                        self.ready = true;
                        self.model_size = Some(size.size);
                        self.model = size.model;
                        self.canceller = size.canceller;
                        self.task_notice = None;
                        if let Some((at, negative)) = self.pending_click.take() {
                            self.click(at, negative);
                        } else {
                            app.ui.status = self.hint().unwrap_or("Selection model is ready").into();
                            app.ui.status_error = false;
                        }
                    }
                    Err(error) => {
                        self.error = Some(error.clone());
                        app.ui.status = format!("Object Selection: {error}");
                        app.ui.status_error = true;
                    }
                },
                Reply::Previewed { key, jpeg } if self.key == Some(key) => {
                    self.hover_busy = false;
                    match jpeg.and_then(|bytes| preview_image(&bytes)) {
                        Ok(image) => {
                            self.overlay = Some(ctx.load_texture("sam3-object-preview", image, egui::TextureOptions::LINEAR));
                        }
                        Err(error) => {
                            if !error.contains("returned no object") {
                                app.ui.status = format!("Object Selection preview: {error}");
                                app.ui.status_error = true;
                            }
                            self.overlay = None;
                        }
                    }
                }
                Reply::Committed { key, mode, mask } if self.key == Some(key) => {
                    self.click_busy = false;
                    match mask {
                        Ok(mask) => {
                            self.apply_mask(app, key, &mask, mode);
                        }
                        Err(error) => {
                            self.error = Some(error.clone());
                            app.ui.status = format!("Object Selection: {error}");
                            app.ui.status_error = true;
                        }
                    }
                }
                Reply::Grounded { key, mode, mask } if self.key == Some(key) => {
                    self.click_busy = false;
                    match mask {
                        Ok((word, mask)) => {
                            self.apply_mask(app, key, &mask, mode);
                            if app.ui.status == "Object selected" {
                                app.ui.status = format!("Selected \u{201c}{word}\u{201d}");
                            }
                            self.task_notice = Some((app.ui.status.clone(), app.ui.status_error));
                        }
                        Err(error) => {
                            app.ui.status = format!("Select Subject: {error}");
                            app.ui.status_error = true;
                            self.task_notice = Some((app.ui.status.clone(), true));
                        }
                    }
                }
                _ => {}
            }
            ctx.request_repaint();
        }
    }

    fn cancel_active(&mut self) {
        if (self.click_busy || self.hover_busy)
            && let Some(cancel) = self.canceller.take()
        {
            cancel.cancel();
        }
    }

    pub(crate) fn queue_name(&self) -> &'static str {
        match self.model {
            crate::sam3_route::SelectionModel::Sam3 => "sam3",
            crate::sam3_route::SelectionModel::Sam21 => "sam21",
        }
    }

    pub(crate) fn supports_points(&self) -> bool {
        self.model.supports_points()
    }

    pub(crate) fn model_label(&self) -> &'static str {
        self.model.label()
    }

    fn clear_preview(&mut self) {
        self.ready = false;
        self.overlay = None;
        self.sent = None;
        self.hover_busy = false;
        self.click_busy = false;
    }

    /// Apply a returned mask through the engine command, then re-key to the new revision.
    fn apply_mask(&mut self, app: &mut PhotocraftApp, key: Key, mask: &[u8], mode: &'static str) {
        let encoded = base64::engine::general_purpose::STANDARD.encode(mask);
        match app.run("select.sam3Mask", json!({"docId":key.doc_id,"revision":key.revision,"maskPng":encoded,"mode":mode})) {
            Ok(result) if result["changed"] == false => {
                app.ui.status = "Object Selection found nothing there".into();
                app.ui.status_error = true;
            }
            Ok(_) => {
                app.ui.status = "Object selected".into();
                app.ui.status_error = false;
                self.overlay = None;
                self.sent = None;
                self.builder.clear();
                if let Some(st) = app.session.active() {
                    let to = Key { revision: st.revision, ..key };
                    self.key = Some(to);
                    if let Some(tasks) = &self.tasks {
                        let _ = tasks.send(Job::Rekey { from: key, to });
                    }
                }
            }
            Err(error) => {
                app.ui.status = format!("Object Selection: {error}");
                app.ui.status_error = true;
                self.error = Some(error);
            }
        }
        self.task_notice = Some((app.ui.status.clone(), app.ui.status_error));
    }

    pub(crate) fn hint(&self) -> Option<&str> {
        if let Some(error) = &self.error {
            Some(error)
        } else if self.key.is_some() && !self.ready {
            Some("Preparing selection…")
        } else if self.ready && !self.model.supports_points() {
            Some(SAM21_HELP)
        } else if self.ready {
            Some("Click an object to select it")
        } else {
            None
        }
    }

    /// Feedback owned by this tool: message, warning state and whether work is running.
    pub(crate) fn task_status(&self) -> Option<(&str, bool, bool)> {
        if let Some(message) = &self.model_picker.notice {
            let busy = self.model_picker.busy || self.model_picker.loading;
            return Some((message, !busy, busy));
        }
        if let Some(error) = &self.error {
            return Some((error, true, false));
        }
        if self.key.is_some() && !self.ready {
            return Some(("Preparing selection…", false, true));
        }
        self.task_notice
            .as_ref()
            .map(|(message, warning)| (message.as_str(), *warning, self.click_busy))
            .or_else(|| (!self.model.supports_points() && self.ready).then_some((SAM21_HELP, false, false)))
    }

    /// Whether a selection job can be queued, shared by the pop-out actions.
    pub(crate) fn can_submit(&self) -> bool {
        self.key.is_some() && self.ready && self.error.is_none() && !self.click_busy && !self.model_picker.busy
    }

    /// The latest failure, if any, for the panel to show.
    pub(crate) fn error(&self) -> Option<String> {
        self.error.clone()
    }

    /// The floating panel follows tool activation: it opens when the tool is selected and
    /// stays closed once the user closes it, until the tool is selected again.
    pub(crate) fn follow_panel(app: &mut PhotocraftApp) {
        if !app.ui.sam3_panel && !app.ui.sam3_panel_closed {
            app.ui.sam3_panel = true;
        }
    }

    /// The user closed the panel; remember that until the tool is reselected.
    pub(crate) fn panel_closed(app: &mut PhotocraftApp) {
        app.ui.sam3_panel = false;
        app.ui.sam3_panel_closed = true;
    }

    /// Drop the whole accumulated prompt and its preview.
    pub(crate) fn clear_prompt(&mut self) {
        self.builder.clear();
        if !self.click_busy {
            self.task_notice = None;
        }
        self.overlay = None;
        self.sent = None;
    }

    pub(crate) fn overlay(&self) -> Option<egui::TextureId> {
        self.ready.then(|| self.overlay.as_ref().map(egui::TextureHandle::id)).flatten()
    }

    /// Keep a preview of the prompt built so far plus the point under the cursor. One request is
    /// in flight at a time; when it returns, the next frame sends the newest cursor position, so
    /// the preview follows the pointer at the model's own pace and never queues up.
    pub(crate) fn hover(&mut self, doc: [u32; 2], at: Option<[f64; 2]>, ctx: &egui::Context) {
        if !self.model.supports_points() {
            return;
        }
        let Some(at) = at else {
            self.overlay = None;
            self.sent = None;
            return;
        };
        let (Some(key), Some(size)) = (self.key, self.model_size) else { return };
        if !self.ready || self.click_busy || self.error.is_some() {
            return;
        }
        let Some(hover_point) = model_point(doc, size, at) else {
            self.overlay = None;
            self.sent = None;
            return;
        };
        // The point under the cursor joins the prompt; with Alt held it previews an exclusion.
        let mut prompt = self.builder.to_prompt(doc, size).unwrap_or_default();
        if ctx.input(|i| i.modifiers.alt) {
            prompt.negative_points.push(hover_point);
        } else {
            prompt.positive_points.push(hover_point);
        }
        if self.hover_busy || self.sent.as_ref() == Some(&prompt) {
            return;
        }
        if let Some(tasks) = &self.tasks
            && tasks.send(Job::Preview { key, prompt: prompt.clone() }).is_ok()
        {
            self.hover_busy = true;
            self.sent = Some(prompt);
        }
    }

    /// A canvas click: add a point to the prompt (Alt = negative) and refresh the preview.
    pub(crate) fn click(&mut self, at: [f64; 2], negative: bool) -> bool {
        if !self.model.supports_points() {
            self.task_notice = Some((SAM21_HELP.into(), false));
            return true;
        }
        if self.key.is_none() {
            return false;
        }
        if self.error.is_some() {
            return false;
        }
        if !self.ready {
            self.pending_click = Some((at, negative));
            return true;
        }
        self.builder.add_point(at, negative);
        self.sent = None;
        true
    }

    /// A drag larger than a click sets the box prompt instead.
    pub(crate) fn box_drag(&mut self, from: [f64; 2], to: [f64; 2]) -> bool {
        if !self.model.supports_points() {
            self.task_notice = Some((SAM21_HELP.into(), false));
            return true;
        }
        if self.error.is_some() || !self.ready {
            return false;
        }
        if !self.builder.set_box(from, to) {
            return false;
        }
        self.sent = None;
        true
    }

    /// Backspace: drop the most recent prompt element.
    pub(crate) fn undo_point(&mut self) {
        if self.builder.negative.pop().is_none() && self.builder.positive.pop().is_none() {
            self.builder.box_ = None;
        }
        self.sent = None;
    }

    /// Enter (or the panel's Apply): commit the accumulated prompt as a selection.
    pub(crate) fn commit(&mut self, doc: [u32; 2]) -> bool {
        if !self.model.supports_points() {
            return false;
        }
        let Some(key) = self.key else { return false };
        if self.error.is_some() || self.click_busy {
            return false;
        }
        let Some(size) = self.model_size else { return false };
        let Some(prompt) = self.builder.to_prompt(doc, size) else { return false };
        if let Some(tasks) = &self.tasks
            && tasks.send(Job::Commit { key, prompt, mode: self.builder.mode.as_str() }).is_ok()
        {
            self.task_notice = Some(("Selecting object with SAM 3…".into(), false));
            self.click_busy = true;
            true
        } else {
            false
        }
    }

    /// Select Subject: run the text path. Empty text lets the worker try common subjects, since
    /// the model understands concrete nouns ("person"), not the word "subject".
    pub(crate) fn ground(&mut self, text: &str) -> bool {
        let Some(key) = self.key else { return false };
        if self.error.is_some() || self.click_busy {
            return false;
        }
        if !self.ready {
            return false;
        }
        let owned = text.trim().to_owned();
        if let Some(tasks) = &self.tasks
            && tasks.send(Job::Ground { key, text: owned, mode: self.builder.mode.as_str() }).is_ok()
        {
            self.task_notice = Some((format!("Selecting subject with {}…", self.model.label()), false));
            self.click_busy = true;
            true
        } else {
            false
        }
    }
}

#[path = "selection_model_switch.rs"]
pub(crate) mod model_switch;

#[path = "selection_image.rs"]
mod image;
use image::{model_point, preview_image, source_png};

#[path = "selection_worker.rs"]
mod selection_worker;
use selection_worker::worker;

#[path = "selection_session.rs"]
mod model_session;

#[path = "sam3_multi_select.rs"]
mod text_selection;

/// Tried in order by Select Subject with no text; the first that matches something wins.
const DEFAULT_SUBJECTS: &[&str] = &["person", "animal", "clothing", "bag", "shoe", "car", "plant", "furniture"];

const SAM21_HELP: &str = "SAM 2.1 selects by name. Type a subject or choose Select Subject.";
#[cfg(test)]
#[path = "sam3_selection_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "sam3_panel_ui_tests.rs"]
mod panel_tests;

#[cfg(test)]
#[path = "sam3_selection_runtime_tests.rs"]
mod runtime_tests;
