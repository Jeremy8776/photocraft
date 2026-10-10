//! Preparing a request: which region to send, at what size, and the request itself.
//!
//! A model fills a hole best when it can see what surrounds it, so the region is the selection
//! plus a margin of context, clamped to the document. Models also have input limits, so the region
//! is scaled down (never up) to fit a pixel budget, with both sides a multiple of what the model
//! wants (8 by default). The host crops the document to [`RegionPlan::crop`], hands the crop and the
//! selection coverage over it to [`build_request`], and later blends the result back with
//! [`crate::apply`].

use crate::backend::{GenRequest, Task};
use crate::error::AiError;
use crate::image::{IRect, Image, Mask};
use crate::resize::{resize_image, resize_mask};

/// How much to send.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PrepLimits {
    /// Most pixels in the image sent.
    pub max_pixels: u32,
    /// Longest side of the image sent.
    pub max_side: u32,
    /// Both sides are rounded down to a multiple of this (at least this).
    pub multiple: u32,
    /// Context around the selection, as a fraction of its longer side.
    pub margin_fraction: f32,
    /// Context is never less than this many pixels (unless the document edge is nearer).
    pub min_margin: u32,
}

impl Default for PrepLimits {
    fn default() -> Self {
        Self { max_pixels: 1024 * 1024, max_side: 2048, multiple: 8, margin_fraction: 0.5, min_margin: 64 }
    }
}

impl PrepLimits {
    fn validate(&self) -> Result<(), AiError> {
        let m = self.multiple;
        let ok = m >= 1
            && self.max_side >= m
            && u64::from(self.max_pixels) >= u64::from(m) * u64::from(m)
            && self.margin_fraction.is_finite()
            && self.margin_fraction >= 0.0;
        if ok { Ok(()) } else { Err(AiError::invalid("the size limits for the request are not usable")) }
    }
}

/// Where the region is and how big it is when sent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RegionPlan {
    /// The region in document pixels: selection plus context, inside the document.
    pub crop: IRect,
    pub send_width: u32,
    pub send_height: u32,
}

fn fit(side: u32, scale: f64, multiple: u32) -> u32 {
    let scaled = (f64::from(side) * scale).floor();
    let scaled = if scaled.is_finite() && scaled > 0.0 { scaled as u32 } else { 0 };
    (scaled / multiple * multiple).max(multiple)
}

/// Chooses the region and the size to send for a selection whose bounds are `selection`.
pub fn plan_region(doc_width: u32, doc_height: u32, selection: IRect, limits: &PrepLimits) -> Result<RegionPlan, AiError> {
    limits.validate()?;
    let doc = IRect::new(0, 0, i32::try_from(doc_width).unwrap_or(i32::MAX), i32::try_from(doc_height).unwrap_or(i32::MAX));
    let sel = selection.intersect(&doc);
    if sel.is_empty() {
        return Err(AiError::invalid("the selection is empty or outside the document"));
    }
    let longer = sel.width().max(sel.height());
    let margin = ((longer as f32 * limits.margin_fraction).ceil() as u32).max(limits.min_margin);
    let crop = sel.expand(margin).intersect(&doc);
    let (cw, ch) = (crop.width(), crop.height());
    let by_side = f64::from(limits.max_side) / f64::from(cw.max(ch));
    let by_pixels = (f64::from(limits.max_pixels) / (f64::from(cw) * f64::from(ch))).sqrt();
    let scale = by_side.min(by_pixels).min(1.0);
    Ok(RegionPlan { crop, send_width: fit(cw, scale, limits.multiple), send_height: fit(ch, scale, limits.multiple) })
}

/// Builds the request from the cropped document pixels and the selection over the same crop.
pub fn build_request(
    task: Task,
    prompt: Option<String>,
    outputs: u32,
    crop_image: &Image,
    crop_selection: &Mask,
    plan: &RegionPlan,
) -> Result<GenRequest, AiError> {
    let (cw, ch) = (plan.crop.width(), plan.crop.height());
    let sizes_match = crop_image.width() == cw && crop_image.height() == ch && crop_selection.width() == cw && crop_selection.height() == ch;
    if !sizes_match {
        return Err(AiError::invalid("the cropped pixels and selection do not match the planned region"));
    }
    let image = resize_image(crop_image, plan.send_width, plan.send_height)?;
    let mask = resize_mask(crop_selection, plan.send_width, plan.send_height)?;
    GenRequest::new(task, prompt, image, mask, outputs)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(doc: (u32, u32), sel: IRect) -> RegionPlan {
        plan_region(doc.0, doc.1, sel, &PrepLimits::default()).unwrap()
    }

    #[test]
    fn context_surrounds_the_selection_and_stays_inside_the_document() {
        let p = plan((4000, 3000), IRect::new(1000, 1000, 1200, 1100));
        // 200 wide: margin is half of 200 = 100, but not below 64.
        assert_eq!(p.crop, IRect::new(900, 900, 1300, 1200));
        let edge = plan((4000, 3000), IRect::new(0, 0, 100, 100));
        // 100 wide: margin is max(50, 64) = 64 on the sides that have room, none past the edge.
        assert_eq!(edge.crop, IRect::new(0, 0, 164, 164), "the margin stops at the document edge");
    }

    #[test]
    fn small_regions_are_not_upscaled_and_stay_multiples() {
        let p = plan((500, 500), IRect::new(200, 200, 230, 230));
        assert!(p.send_width <= p.crop.width() && p.send_height <= p.crop.height());
        assert_eq!(p.send_width % 8, 0);
        assert_eq!(p.send_height % 8, 0);
        assert!(p.send_width >= 8);
    }

    #[test]
    fn large_regions_are_scaled_down_to_the_budget() {
        let p = plan((20_000, 20_000), IRect::new(0, 0, 8000, 6000));
        assert!(u64::from(p.send_width) * u64::from(p.send_height) <= 1024 * 1024);
        assert!(p.send_width <= 2048 && p.send_height <= 2048);
        // The crop is 12000 x 10000 (the selection plus 4000 of context), and its aspect ratio is
        // kept to within one multiple.
        assert_eq!(p.crop, IRect::new(0, 0, 12_000, 10_000));
        let r = f64::from(p.send_width) / f64::from(p.send_height);
        assert!((r - 1.2).abs() < 0.02, "{r}");
    }

    #[test]
    fn bad_selections_and_limits_are_errors() {
        let l = PrepLimits::default();
        assert!(plan_region(100, 100, IRect::new(200, 200, 300, 300), &l).is_err(), "outside");
        assert!(plan_region(100, 100, IRect::new(10, 10, 10, 50), &l).is_err(), "empty");
        assert!(plan_region(0, 0, IRect::new(0, 0, 1, 1), &l).is_err());
        let zero = PrepLimits { multiple: 0, ..l };
        assert!(plan_region(100, 100, IRect::new(0, 0, 50, 50), &zero).is_err());
        let nan = PrepLimits { margin_fraction: f32::NAN, ..l };
        assert!(plan_region(100, 100, IRect::new(0, 0, 50, 50), &nan).is_err());
        let tiny_budget = PrepLimits { max_pixels: 1, ..l };
        assert!(plan_region(100, 100, IRect::new(0, 0, 50, 50), &tiny_budget).is_err());
    }

    #[test]
    fn extreme_rectangles_do_not_overflow() {
        let huge = IRect::new(i32::MIN, i32::MIN, i32::MAX, i32::MAX);
        let p = plan_region(30_000, 30_000, huge, &PrepLimits::default()).unwrap();
        assert_eq!(p.crop, IRect::new(0, 0, 30_000, 30_000));
        assert!(p.send_width <= 2048);
    }

    #[test]
    fn the_request_is_resized_to_the_plan() {
        let p = plan((2000, 2000), IRect::new(500, 500, 900, 700));
        let (cw, ch) = (p.crop.width(), p.crop.height());
        let image = Image::solid(cw, ch, [1, 2, 3, 255]).unwrap();
        let mask = Mask::new(cw, ch, vec![255; (cw * ch) as usize]).unwrap();
        let req = build_request(Task::Remove, None, 1, &image, &mask, &p).unwrap();
        assert_eq!((req.image.width(), req.image.height()), (p.send_width, p.send_height));
        assert_eq!((req.mask.width(), req.mask.height()), (p.send_width, p.send_height));
        // A crop that does not match the plan is refused, not guessed at.
        let wrong = Image::solid(cw + 1, ch, [0; 4]).unwrap();
        assert!(build_request(Task::Remove, None, 1, &wrong, &mask, &p).is_err());
    }
}
