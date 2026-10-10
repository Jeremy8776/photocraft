//! The backend trait: the one interface a generative service implements for PhotoCraft.
//!
//! It is blocking on purpose. The engine already runs long commands on a worker thread with
//! progress and cancel ([`Control`] mirrors that), so a backend that talks to a network service
//! simply blocks in `generate`. Credentials are the backend's business: it is built with whatever
//! it needs and nothing secret crosses this boundary.
//!
//! The contract a backend must keep:
//! - Return images the **same size** as `request.image`. Resizing is the backend's job, because it
//!   knows what its model needs.
//! - Return between one and `request.outputs` images.
//! - Check [`Control::cancelled`] between steps and return [`AiError::cancelled`] promptly.
//! - Refuse a task it cannot do with [`ErrorKind::Unsupported`](crate::ErrorKind), before sending
//!   anything.
//! - Never put a key or token in an error message.

use crate::error::AiError;
use crate::image::{Image, Mask};

/// Most variations one request may ask for.
pub const MAX_OUTPUTS: u32 = 4;
/// Longest prompt accepted, in characters.
pub const MAX_PROMPT_CHARS: usize = 4000;

/// What the user is asking for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Task {
    /// Fill the selection with something new (a prompt may steer it; empty means "match the
    /// surroundings").
    Fill,
    /// Remove what is inside the selection and fill it to match the surroundings.
    Remove,
}

/// One request to a backend.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GenRequest {
    pub task: Task,
    /// What to generate. `None` or empty for "match the surroundings".
    pub prompt: Option<String>,
    /// The region to work on, with context around the selection.
    pub image: Image,
    /// Which pixels of `image` to replace (255 = replace).
    pub mask: Mask,
    /// How many variations to return, 1 to [`MAX_OUTPUTS`].
    pub outputs: u32,
}

impl GenRequest {
    pub fn new(task: Task, prompt: Option<String>, image: Image, mask: Mask, outputs: u32) -> Result<Self, AiError> {
        let req = Self { task, prompt, image, mask, outputs };
        req.validate()?;
        Ok(req)
    }

    /// Checks everything a backend may rely on.
    pub fn validate(&self) -> Result<(), AiError> {
        if self.image.width() != self.mask.width() || self.image.height() != self.mask.height() {
            return Err(AiError::invalid("the mask and the image are different sizes"));
        }
        if self.mask.is_empty() {
            return Err(AiError::invalid("nothing is selected"));
        }
        if !(1..=MAX_OUTPUTS).contains(&self.outputs) {
            return Err(AiError::invalid(format!("ask for 1 to {MAX_OUTPUTS} variations")));
        }
        if self.prompt.as_deref().is_some_and(|p| p.chars().count() > MAX_PROMPT_CHARS) {
            return Err(AiError::invalid(format!("the prompt is limited to {MAX_PROMPT_CHARS} characters")));
        }
        Ok(())
    }

    /// The prompt with surrounding space removed, or `None` when there is nothing to say.
    pub fn prompt_text(&self) -> Option<&str> {
        self.prompt.as_deref().map(str::trim).filter(|p| !p.is_empty())
    }
}

/// What a backend returns.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GenResult {
    pub images: Vec<Image>,
}

impl GenResult {
    /// Enforces the contract on what a backend returned, so the host never trusts it blindly.
    pub fn check(&self, request: &GenRequest) -> Result<(), AiError> {
        if self.images.is_empty() {
            return Err(AiError::new(crate::ErrorKind::Provider, "the service returned no images"));
        }
        if self.images.len() > request.outputs as usize {
            return Err(AiError::new(crate::ErrorKind::Provider, "the service returned more images than were asked for"));
        }
        let (w, h) = (request.image.width(), request.image.height());
        if self.images.iter().any(|i| i.width() != w || i.height() != h) {
            return Err(AiError::new(crate::ErrorKind::Provider, "the service returned an image of the wrong size"));
        }
        Ok(())
    }
}

/// Progress and cancellation, supplied by the host's job system.
pub trait Control: Send + Sync {
    /// Has the user cancelled? Check between steps and return [`AiError::cancelled`].
    fn cancelled(&self) -> bool;
    /// Report progress in `0.0..=1.0` with a short status.
    fn progress(&self, fraction: f32, message: &str);
}

/// A [`Control`] that is never cancelled and ignores progress, for tests and simple callers.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoControl;

impl Control for NoControl {
    fn cancelled(&self) -> bool {
        false
    }

    fn progress(&self, _fraction: f32, _message: &str) {}
}

/// A generative service.
pub trait GenerationBackend: Send + Sync {
    /// A short name for the UI, such as "Ollama" or "OpenAI". Never a secret.
    fn label(&self) -> String;

    /// Whether this backend can do `task`, so the UI can grey out what it cannot.
    fn supports(&self, task: Task) -> bool;

    /// Whether data sent to this backend leaves the machine. The host asks for the user's
    /// agreement before the first remote request.
    fn is_remote(&self) -> bool;

    /// Does the work. Blocks until done, cancelled or failed.
    fn generate(&self, request: &GenRequest, control: &dyn Control) -> Result<GenResult, AiError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn img(w: u32, h: u32) -> Image {
        Image::solid(w, h, [0, 0, 0, 255]).unwrap()
    }

    fn mask(w: u32, h: u32, v: u8) -> Mask {
        Mask::new(w, h, vec![v; (w * h) as usize]).unwrap()
    }

    #[test]
    fn requests_are_checked_before_a_backend_sees_them() {
        assert!(GenRequest::new(Task::Fill, None, img(8, 8), mask(8, 8, 255), 1).is_ok());
        assert!(GenRequest::new(Task::Fill, None, img(8, 8), mask(8, 4, 255), 1).is_err(), "size mismatch");
        assert!(GenRequest::new(Task::Fill, None, img(8, 8), mask(8, 8, 0), 1).is_err(), "nothing selected");
        assert!(GenRequest::new(Task::Fill, None, img(8, 8), mask(8, 8, 255), 0).is_err());
        assert!(GenRequest::new(Task::Fill, None, img(8, 8), mask(8, 8, 255), MAX_OUTPUTS + 1).is_err());
        let long = "x".repeat(MAX_PROMPT_CHARS + 1);
        assert!(GenRequest::new(Task::Fill, Some(long), img(8, 8), mask(8, 8, 255), 1).is_err());
    }

    #[test]
    fn a_blank_prompt_counts_as_none() {
        let r = GenRequest::new(Task::Fill, Some("   \n".into()), img(8, 8), mask(8, 8, 255), 1).unwrap();
        assert_eq!(r.prompt_text(), None);
        let r = GenRequest::new(Task::Fill, Some(" a fox ".into()), img(8, 8), mask(8, 8, 255), 1).unwrap();
        assert_eq!(r.prompt_text(), Some("a fox"));
    }

    #[test]
    fn results_are_held_to_the_contract() {
        let req = GenRequest::new(Task::Fill, None, img(8, 8), mask(8, 8, 255), 2).unwrap();
        assert!(GenResult { images: vec![img(8, 8)] }.check(&req).is_ok());
        assert!(GenResult { images: vec![img(8, 8), img(8, 8)] }.check(&req).is_ok());
        assert!(GenResult { images: vec![] }.check(&req).is_err());
        assert!(GenResult { images: vec![img(8, 8), img(8, 8), img(8, 8)] }.check(&req).is_err());
        assert!(GenResult { images: vec![img(8, 9)] }.check(&req).is_err());
    }
}
