//! A scripted backend for tests: the engine, the UI and the control channel can run the whole
//! Generative Fill path with no network and no model.

use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use crate::backend::{Control, GenRequest, GenResult, GenerationBackend, Task};
use crate::error::AiError;
use crate::image::Image;

/// What the mock does when asked to generate.
#[derive(Clone, Debug)]
pub enum MockBehavior {
    /// Paints the selected pixels this colour. Variation `n` shifts the red channel by `30 * n`
    /// so several variations are told apart.
    Fill([u8; 4]),
    /// Returns the image unchanged.
    Echo,
    /// Paints the selected pixels with a left-to-right grey ramp.
    Gradient,
    /// Fails with this error.
    Fail(AiError),
    /// Works in `steps` steps of `step_ms` each, reporting progress and honouring cancel, then
    /// fills with grey.
    Slow { steps: u32, step_ms: u64 },
    /// Breaks the contract by returning an image one pixel too wide.
    WrongSize,
}

/// What the mock was asked, for assertions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Call {
    pub task: Task,
    pub prompt: Option<String>,
    pub width: u32,
    pub height: u32,
    /// Pixels with any selection.
    pub selected_pixels: usize,
    pub outputs: u32,
}

/// The scripted backend.
pub struct MockBackend {
    behavior: MockBehavior,
    remote: bool,
    refused: Vec<Task>,
    calls: Mutex<Vec<Call>>,
}

impl MockBackend {
    pub fn new(behavior: MockBehavior) -> Self {
        Self { behavior, remote: false, refused: Vec::new(), calls: Mutex::new(Vec::new()) }
    }

    /// Pretends to be a remote service, so consent paths can be tested.
    pub fn remote(mut self) -> Self {
        self.remote = true;
        self
    }

    /// Reports that it cannot do `task`, so the host's refusal path can be tested.
    pub fn refusing(mut self, task: Task) -> Self {
        self.refused.push(task);
        self
    }

    pub fn calls(&self) -> Vec<Call> {
        self.calls.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

fn painted(req: &GenRequest, colour: impl Fn(u32, u32) -> [u8; 4]) -> Result<Image, AiError> {
    let (w, h) = (req.image.width(), req.image.height());
    let mut out = req.image.rgba().to_vec();
    for y in 0..h {
        for x in 0..w {
            if req.mask.coverage(x, y) >= 128 {
                let i = (y as usize * w as usize + x as usize) * 4;
                if let Some(px) = out.get_mut(i..i + 4) {
                    px.copy_from_slice(&colour(x, y));
                }
            }
        }
    }
    Image::new(w, h, out)
}

impl GenerationBackend for MockBackend {
    fn label(&self) -> String {
        "Mock".to_owned()
    }

    fn supports(&self, task: Task) -> bool {
        !self.refused.contains(&task)
    }

    fn is_remote(&self) -> bool {
        self.remote
    }

    fn generate(&self, req: &GenRequest, control: &dyn Control) -> Result<GenResult, AiError> {
        self.calls.lock().unwrap_or_else(PoisonError::into_inner).push(Call {
            task: req.task,
            prompt: req.prompt_text().map(str::to_owned),
            width: req.image.width(),
            height: req.image.height(),
            selected_pixels: req.mask.data().iter().filter(|&&v| v > 0).count(),
            outputs: req.outputs,
        });
        if control.cancelled() {
            return Err(AiError::cancelled());
        }
        let mut images = Vec::new();
        match &self.behavior {
            MockBehavior::Fill(c) => {
                for n in 0..req.outputs {
                    let shift = u8::try_from(n.saturating_mul(30)).unwrap_or(u8::MAX);
                    images.push(painted(req, |_, _| [c[0].wrapping_add(shift), c[1], c[2], c[3]])?);
                }
            }
            MockBehavior::Echo => images.push(req.image.clone()),
            MockBehavior::Gradient => {
                let w = req.image.width().max(2) - 1;
                images.push(painted(req, |x, _| {
                    let v = u8::try_from(x.saturating_mul(255) / w).unwrap_or(u8::MAX);
                    [v, v, v, 255]
                })?);
            }
            MockBehavior::Fail(e) => return Err(e.clone()),
            MockBehavior::Slow { steps, step_ms } => {
                for i in 0..*steps {
                    if control.cancelled() {
                        return Err(AiError::cancelled());
                    }
                    control.progress(i as f32 / (*steps).max(1) as f32, "Generating");
                    std::thread::sleep(Duration::from_millis(*step_ms));
                }
                images.push(painted(req, |_, _| [128, 128, 128, 255])?);
            }
            MockBehavior::WrongSize => {
                images.push(Image::solid(req.image.width() + 1, req.image.height(), [0, 0, 0, 255])?);
            }
        }
        control.progress(1.0, "Done");
        Ok(GenResult { images })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::NoControl;
    use crate::image::Mask;
    use std::sync::atomic::{AtomicBool, Ordering};

    fn request(outputs: u32) -> GenRequest {
        let image = Image::solid(4, 2, [10, 20, 30, 255]).unwrap();
        // Left half selected.
        let mask = Mask::new(4, 2, vec![255, 255, 0, 0, 255, 255, 0, 0]).unwrap();
        GenRequest::new(Task::Fill, Some(" a fox ".into()), image, mask, outputs).unwrap()
    }

    #[test]
    fn fill_changes_only_the_selected_pixels_and_variations_differ() {
        let b = MockBackend::new(MockBehavior::Fill([200, 0, 0, 255]));
        let r = request(2);
        let out = b.generate(&r, &NoControl).unwrap();
        r_check(&out, &r);
        assert_eq!(out.images[0].pixel(0, 0), Some([200, 0, 0, 255]));
        assert_eq!(out.images[0].pixel(3, 0), Some([10, 20, 30, 255]));
        assert_ne!(out.images[0].pixel(0, 0), out.images[1].pixel(0, 0));
        let calls = b.calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].prompt.as_deref(), Some("a fox"));
        assert_eq!(calls[0].selected_pixels, 4);
    }

    fn r_check(out: &GenResult, r: &GenRequest) {
        out.check(r).unwrap();
    }

    #[test]
    fn gradient_and_echo_behave() {
        let r = request(1);
        let g = MockBackend::new(MockBehavior::Gradient).generate(&r, &NoControl).unwrap();
        assert_eq!(g.images[0].pixel(0, 0), Some([0, 0, 0, 255]));
        assert_eq!(g.images[0].pixel(1, 0), Some([85, 85, 85, 255]));
        let e = MockBackend::new(MockBehavior::Echo).generate(&r, &NoControl).unwrap();
        assert_eq!(e.images[0], r.image);
    }

    #[test]
    fn failures_and_contract_breaks_are_reported() {
        let r = request(1);
        let e = MockBackend::new(MockBehavior::Fail(AiError::new(crate::ErrorKind::Quota, "out of credits")));
        assert_eq!(e.generate(&r, &NoControl).unwrap_err().kind, crate::ErrorKind::Quota);
        let w = MockBackend::new(MockBehavior::WrongSize).generate(&r, &NoControl).unwrap();
        assert!(w.check(&r).is_err());
    }

    struct Flag(AtomicBool);
    impl Control for Flag {
        fn cancelled(&self) -> bool {
            self.0.load(Ordering::SeqCst)
        }
        fn progress(&self, _: f32, _: &str) {}
    }

    #[test]
    fn slow_work_stops_when_cancelled() {
        let b = MockBackend::new(MockBehavior::Slow { steps: 50, step_ms: 20 });
        let flag = Flag(AtomicBool::new(true));
        assert_eq!(b.generate(&request(1), &flag).unwrap_err().kind, crate::ErrorKind::Cancelled);
        let ok = MockBackend::new(MockBehavior::Slow { steps: 2, step_ms: 1 });
        assert!(ok.generate(&request(1), &NoControl).is_ok());
    }

    #[test]
    fn tasks_can_be_refused() {
        let b = MockBackend::new(MockBehavior::Echo).refusing(Task::Remove);
        assert!(b.supports(Task::Fill));
        assert!(!b.supports(Task::Remove));
    }

    #[test]
    fn remote_flag() {
        assert!(!MockBackend::new(MockBehavior::Echo).is_remote());
        assert!(MockBackend::new(MockBehavior::Echo).remote().is_remote());
    }
}
