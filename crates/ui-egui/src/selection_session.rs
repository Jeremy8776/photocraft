//! Selected-model execution seam. Never substitute a different model for the shared choice.
use crate::sam3_route::{SelectionModel, SelectionRoute};
use craft_ai_comfyui::{Sam3Canceller, Sam3Session, Sam21Canceller, Sam21Session, sam3::Prompt};

pub(super) enum ModelSession {
    Sam3(Sam3Session),
    Sam21(Sam21Session),
}

#[derive(Clone)]
pub(super) enum ModelCanceller {
    Sam3(Sam3Canceller),
    Sam21(Sam21Canceller),
}
impl ModelCanceller {
    pub(super) fn cancel(&self) {
        match self {
            Self::Sam3(c) => c.cancel(),
            Self::Sam21(c) => c.cancel(),
        }
    }
}

impl ModelSession {
    pub(super) fn new(route: &SelectionRoute) -> Result<Self, String> {
        match route.model {
            SelectionModel::Sam3 => Sam3Session::new(&route.url).map(Self::Sam3),
            SelectionModel::Sam21 => Sam21Session::new(&route.url).map(Self::Sam21),
        }
        .map_err(|e| e.message)
    }
    pub(super) fn model(&self) -> SelectionModel {
        match self {
            Self::Sam3(_) => SelectionModel::Sam3,
            Self::Sam21(_) => SelectionModel::Sam21,
        }
    }
    pub(super) fn canceller(&self) -> ModelCanceller {
        match self {
            Self::Sam3(s) => ModelCanceller::Sam3(s.canceller()),
            Self::Sam21(s) => ModelCanceller::Sam21(s.canceller()),
        }
    }
    pub(super) fn prepare(&mut self, image: &[u8]) -> Result<(), String> {
        match self {
            Self::Sam3(s) => s.verify_server().and_then(|()| s.prepare(image)).map(|_| ()),
            Self::Sam21(s) => s.verify_server().and_then(|()| s.prepare(image)),
        }
        .map_err(|e| e.message)
    }
    pub(super) fn ground(&self, text: &str) -> Result<Vec<u8>, String> {
        match self {
            Self::Sam3(s) => s.ground(text),
            Self::Sam21(s) => s.ground(text),
        }
        .map_err(|e| e.message)
    }
    pub(super) fn preview(&self, prompt: &Prompt) -> Result<Vec<u8>, String> {
        match self {
            Self::Sam3(s) => s.preview_prompt(prompt).map_err(|e| e.message),
            Self::Sam21(_) => Err(super::SAM21_HELP.into()),
        }
    }
    pub(super) fn commit(&self, prompt: &Prompt) -> Result<Vec<u8>, String> {
        match self {
            Self::Sam3(s) => s.commit_prompt(prompt).map_err(|e| e.message),
            Self::Sam21(_) => Err(super::SAM21_HELP.into()),
        }
    }
    pub(super) fn ground_subject(&self) -> Result<(String, Vec<u8>), String> {
        for text in super::DEFAULT_SUBJECTS {
            match self.ground(text) {
                Ok(mask) => return Ok(((*text).to_owned(), mask)),
                Err(error) if error.starts_with("Nothing matched") => continue,
                Err(error) => return Err(error),
            }
        }
        Err("Nothing matched a common subject. Type a subject name to try another.".into())
    }
}

#[cfg(test)]
#[path = "selection_session_tests.rs"]
mod tests;
