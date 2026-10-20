//! A model change invalidates old work before another result can alter the document.
//! Store reads, file checks and binding writes use a worker, never the drawing thread.
#[cfg(test)]
#[path = "selection_model_switch_tests.rs"]
mod tests;

use crate::sam3_route::model_store::{Snapshot, Target};
use std::sync::mpsc;

pub(crate) enum Action {
    Refresh,
    Choose(Target),
}
pub(crate) struct Event {
    pub result: Result<Snapshot, String>,
    pub changed: bool,
}
#[derive(Default)]
pub(crate) struct Picker {
    pub snapshot: Snapshot,
    pub notice: Option<String>,
    pub enabled: bool,
    pub busy: bool,
    pub loading: bool,
    restart: bool,
    jobs: Option<mpsc::Sender<Action>>,
    replies: Option<mpsc::Receiver<Event>>,
}
impl Picker {
    pub(crate) fn with_transport(jobs: mpsc::Sender<Action>, replies: mpsc::Receiver<Event>) -> Self {
        Self { enabled: true, jobs: Some(jobs), replies: Some(replies), ..Default::default() }
    }
    pub(crate) fn request(&mut self, target: Target) {
        if !self.enabled || self.busy || self.loading || !self.snapshot.options.iter().any(|o| o.target == target && o.reason.is_none()) {
            return;
        }
        if self.jobs.as_ref().is_some_and(|jobs| jobs.send(Action::Choose(target)).is_ok()) {
            self.busy = true;
            self.restart = true;
            self.notice = Some("Changing model…".into());
        } else {
            self.notice = Some("The model list is unavailable. Reopen the tool and try again.".into());
        }
    }

    pub(crate) fn refresh(&mut self) {
        if self.busy || self.loading {
            return;
        }
        if self.jobs.as_ref().is_some_and(|jobs| jobs.send(Action::Refresh).is_ok()) {
            self.loading = true;
            if self.snapshot.options.is_empty() {
                self.notice = Some("Loading models…".into());
            }
        }
    }

    fn ensure_worker(&mut self, ctx: &egui::Context) {
        if !self.enabled || self.jobs.is_some() {
            return;
        }
        let (jobs, requests) = mpsc::channel();
        let (results, replies) = mpsc::channel();
        *self = Self::with_transport(jobs, replies);
        let wake = ctx.clone();
        std::thread::spawn(move || {
            use crate::sam3_route::model_store;
            use craft_ai_native::shared::SharedStore;
            for action in requests {
                let changing = matches!(&action, Action::Choose(_));
                let result = (|| {
                    let shared = SharedStore::at_default_location().map_err(|e| e.to_string())?;
                    let install = craft_ai_comfy_env::ComfyInstall::discover();
                    match action {
                        Action::Refresh => model_store::snapshot(&shared, install.as_ref()),
                        Action::Choose(target) => model_store::choose(&shared, install.as_ref(), &target),
                    }
                })();
                let changed = changing && result.is_ok();
                if results.send(Event { result, changed }).is_err() {
                    break;
                }
                wake.request_repaint();
            }
        });
        self.refresh();
    }

    fn poll(&mut self) -> bool {
        let mut changed = false;
        while let Some(event) = self.replies.as_ref().and_then(|r| r.try_recv().ok()) {
            self.busy = false;
            self.loading = false;
            match event.result {
                Ok(snapshot) => {
                    changed |= event.changed || self.snapshot.selected != snapshot.selected || self.snapshot.route != snapshot.route;
                    self.snapshot = snapshot;
                    self.notice = None;
                }
                Err(message) => self.notice = Some(message),
            }
        }
        changed
    }
}

impl crate::PhotocraftApp {
    /// Use the suite's shared model picker; headless library tests remain isolated by default.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn with_selection_model_picker(mut self) -> Self {
        self.sam3.model_picker.enabled = true;
        self
    }
}

impl super::Runtime {
    pub(super) fn poll_model_picker(&mut self, ctx: &egui::Context) -> bool {
        self.model_picker.ensure_worker(ctx);
        let restart = std::mem::take(&mut self.model_picker.restart);
        let changed = self.model_picker.poll();
        if restart || changed {
            self.cancel_active();
            self.key = None;
            self.clear_preview();
            self.pending_click = None;
            self.builder.clear();
            self.error = None;
            self.task_notice = None;
        }
        self.model_picker.busy
    }
}
