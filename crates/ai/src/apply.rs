//! Blending a result back: the generated pixels, restored to the crop's size, with the selection as
//! their transparency.
//!
//! The patch is the crop's size and fully transparent outside the selection, so it can become a new
//! layer over the document with the original untouched underneath. A feathered selection gives a
//! soft edge, because the coverage is the alpha.

use crate::error::AiError;
use crate::image::{Image, Mask};
use crate::prep::RegionPlan;
use crate::resize::resize_image;

/// Returns the patch for one generated image: the crop's size, colour from `result`, alpha equal to
/// the selection's coverage times the result's own alpha.
pub fn compose_patch(result: &Image, plan: &RegionPlan, crop_selection: &Mask) -> Result<Image, AiError> {
    let (cw, ch) = (plan.crop.width(), plan.crop.height());
    if crop_selection.width() != cw || crop_selection.height() != ch {
        return Err(AiError::invalid("the selection does not match the planned region"));
    }
    if result.width() != plan.send_width || result.height() != plan.send_height {
        return Err(AiError::new(crate::ErrorKind::Provider, "the generated image is not the size that was asked for"));
    }
    let full = resize_image(result, cw, ch)?;
    let mut rgba = full.into_rgba();
    for (i, px) in rgba.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        let (x, y) = (i as u32 % cw, i as u32 / cw);
        let cover = u32::from(crop_selection.coverage(x, y));
        px[3] = ((u32::from(px[3]) * cover + 127) / 255) as u8;
    }
    Image::new(cw, ch, rgba)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::Task;
    use crate::image::IRect;
    use crate::prep::{PrepLimits, build_request, plan_region};

    fn setup() -> (RegionPlan, Mask) {
        let plan = plan_region(300, 300, IRect::new(100, 100, 140, 140), &PrepLimits { min_margin: 10, ..PrepLimits::default() }).unwrap();
        let (cw, ch) = (plan.crop.width(), plan.crop.height());
        let mut d = vec![0u8; (cw * ch) as usize];
        // Selection is the centre 40 x 40 of the crop, fully selected, with one half-selected pixel.
        for y in 10..50 {
            for x in 10..50 {
                d[(y * cw + x) as usize] = 255;
            }
        }
        d[(10 * cw + 10) as usize] = 128;
        (plan, Mask::new(cw, ch, d).unwrap())
    }

    #[test]
    fn the_patch_is_opaque_inside_the_selection_and_clear_outside() {
        let (plan, sel) = setup();
        let result = Image::solid(plan.send_width, plan.send_height, [9, 8, 7, 255]).unwrap();
        let patch = compose_patch(&result, &plan, &sel).unwrap();
        assert_eq!((patch.width(), patch.height()), (plan.crop.width(), plan.crop.height()));
        assert_eq!(patch.pixel(30, 30), Some([9, 8, 7, 255]));
        assert_eq!(patch.pixel(0, 0).unwrap()[3], 0);
        assert_eq!(patch.pixel(10, 10).unwrap()[3], 128, "soft edges keep their softness");
    }

    #[test]
    fn a_result_with_its_own_transparency_multiplies_in() {
        let (plan, sel) = setup();
        let result = Image::solid(plan.send_width, plan.send_height, [9, 8, 7, 128]).unwrap();
        let patch = compose_patch(&result, &plan, &sel).unwrap();
        assert_eq!(patch.pixel(30, 30).unwrap()[3], 128);
    }

    #[test]
    fn mismatched_inputs_are_errors() {
        let (plan, sel) = setup();
        let wrong_size = Image::solid(plan.send_width + 8, plan.send_height, [0; 4]).unwrap();
        assert!(compose_patch(&wrong_size, &plan, &sel).is_err());
        let ok = Image::solid(plan.send_width, plan.send_height, [0; 4]).unwrap();
        let bad_sel = Mask::new(5, 5, vec![0; 25]).unwrap();
        assert!(compose_patch(&ok, &plan, &bad_sel).is_err());
    }

    #[test]
    fn round_trip_through_a_mock_changes_only_the_selection() {
        use crate::backend::{GenerationBackend, NoControl};
        use crate::mock::{MockBackend, MockBehavior};
        let (plan, sel) = setup();
        let (cw, ch) = (plan.crop.width(), plan.crop.height());
        let photo = Image::solid(cw, ch, [10, 20, 30, 255]).unwrap();
        let req = build_request(Task::Fill, None, 1, &photo, &sel, &plan).unwrap();
        let out = MockBackend::new(MockBehavior::Fill([250, 0, 0, 255])).generate(&req, &NoControl).unwrap();
        out.check(&req).unwrap();
        let patch = compose_patch(&out.images[0], &plan, &sel).unwrap();
        // Inside the selection the patch is red; outside it contributes nothing.
        assert_eq!(patch.pixel(30, 30), Some([250, 0, 0, 255]));
        assert_eq!(patch.pixel(2, 2).unwrap()[3], 0);
    }
}
