//! The prompt the Object Selection tool is building: accumulated points, negatives, one box,
//! plus the text-prompt field. Shared by the canvas gestures and the floating panel.
use craft_ai_comfyui::sam3::Prompt;

/// A prompt in document coordinates, converted to model pixels on submit.
#[derive(Default)]
pub(crate) struct Builder {
    pub positive: Vec<[f64; 2]>,
    pub negative: Vec<[f64; 2]>,
    pub box_: Option<([f64; 2], [f64; 2])>,
    pub mode: Mode,
    pub text: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Mode {
    #[default]
    Replace,
    Add,
    Subtract,
    Intersect,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Replace => "replace",
            Mode::Add => "add",
            Mode::Subtract => "subtract",
            Mode::Intersect => "intersect",
        }
    }
}

impl Builder {
    pub(crate) fn clear(&mut self) {
        self.positive.clear();
        self.negative.clear();
        self.box_ = None;
    }

    pub(crate) fn add_point(&mut self, at: [f64; 2], negative: bool) {
        let list = if negative { &mut self.negative } else { &mut self.positive };
        if list.len() < 16 && list.iter().all(|p| (p[0] - at[0]).abs() > 0.5 || (p[1] - at[1]).abs() > 0.5) {
            list.push(at);
        }
    }

    pub(crate) fn set_box(&mut self, from: [f64; 2], to: [f64; 2]) -> bool {
        let (x0, y0) = (from[0].min(to[0]), from[1].min(to[1]));
        let (x1, y1) = (from[0].max(to[0]), from[1].max(to[1]));
        if (x1 - x0) < 1.0 || (y1 - y0) < 1.0 {
            return false;
        }
        self.box_ = Some(([x0, y0], [x1, y1]));
        true
    }

    /// Convert to model pixels; `None` when the prompt has no content.
    pub(crate) fn to_prompt(&self, doc: [u32; 2], model: [u32; 2]) -> Option<Prompt> {
        let map = |p: [f64; 2]| -> (f32, f32) {
            ((p[0] * f64::from(model[0]) / f64::from(doc[0].max(1))) as f32, (p[1] * f64::from(model[1]) / f64::from(doc[1].max(1))) as f32)
        };
        let box_ = self.box_.map(|(a, b)| {
            let (ax, ay) = map(a);
            let (bx, by) = map(b);
            [ax, ay, bx, by]
        });
        let prompt =
            Prompt { positive_points: self.positive.iter().map(|p| map(*p)).collect(), negative_points: self.negative.iter().map(|p| map(*p)).collect(), box_ };
        if prompt.is_empty() { None } else { Some(prompt) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn points_accumulate_dedupe_and_cap_at_16() {
        let mut b = Builder::default();
        for _ in 0..20 {
            b.add_point([5.0, 5.0], false);
        }
        for i in 0..18 {
            b.add_point([i as f64, 9.0], true);
        }
        assert_eq!(b.positive, vec![[5.0, 5.0]]);
        assert_eq!(b.negative.len(), 16);
    }

    #[test]
    fn degenerate_drag_does_not_set_a_box() {
        let mut b = Builder::default();
        assert!(!b.set_box([10.0, 10.0], [10.5, 10.0]));
        assert!(b.box_.is_none());
        assert!(b.set_box([10.0, 10.0], [40.0, 30.0]));
        assert_eq!(b.box_, Some(([10.0, 10.0], [40.0, 30.0])));
    }

    #[test]
    fn empty_builder_never_becomes_a_model_prompt() {
        let mut b = Builder::default();
        assert!(b.to_prompt([100, 100], [50, 50]).is_none());
        b.add_point([80.0, 40.0], false);
        let prompt = b.to_prompt([100, 100], [50, 50]).unwrap();
        assert_eq!(prompt.positive_points, vec![(40.0, 20.0)]);
        assert!(prompt.box_.is_none());
    }
}
