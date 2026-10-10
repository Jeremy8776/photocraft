//! Resampling for the port: a separable tent filter that widens when shrinking, so a large crop
//! sent to a small model input is averaged rather than aliased. Colour is weighted by alpha, so a
//! transparent edge does not bleed black into its neighbours.

use crate::error::AiError;
use crate::image::{Image, Mask};

/// For each destination index, the first source index and its weights (summing to 1).
fn weights(src: usize, dst: usize) -> Vec<(usize, Vec<f32>)> {
    let scale = src as f32 / dst as f32;
    let support = scale.max(1.0);
    (0..dst)
        .map(|d| {
            let centre = (d as f32 + 0.5) * scale;
            let lo = (centre - support).floor().max(0.0) as usize;
            let hi = ((centre + support).ceil() as usize).min(src);
            let lo = lo.min(src.saturating_sub(1));
            let hi = hi.max(lo + 1).min(src);
            let mut w: Vec<f32> = (lo..hi).map(|i| (1.0 - ((i as f32 + 0.5 - centre).abs() / support)).max(0.0)).collect();
            let sum: f32 = w.iter().sum();
            if sum <= f32::EPSILON {
                // The window caught no weight (a very thin overlap): fall back to the nearest pixel.
                let nearest = (centre.floor() as usize).clamp(lo, hi - 1) - lo;
                w.iter_mut().for_each(|v| *v = 0.0);
                if let Some(v) = w.get_mut(nearest) {
                    *v = 1.0;
                }
            } else {
                w.iter_mut().for_each(|v| *v /= sum);
            }
            (lo, w)
        })
        .collect()
}

/// Resamples `src` (`ch` interleaved f32 channels) from `sw` x `sh` to `dw` x `dh`.
fn resample(src: &[f32], sw: usize, sh: usize, ch: usize, dw: usize, dh: usize) -> Vec<f32> {
    let wx = weights(sw, dw);
    let wy = weights(sh, dh);
    // Horizontal pass: sh rows of dw pixels.
    let mut mid = vec![0.0f32; sh * dw * ch];
    for y in 0..sh {
        let row = &src[y * sw * ch..(y + 1) * sw * ch];
        for (x, (lo, w)) in wx.iter().enumerate() {
            let out = &mut mid[(y * dw + x) * ch..(y * dw + x + 1) * ch];
            for (k, wk) in w.iter().enumerate() {
                let px = &row[(lo + k) * ch..(lo + k + 1) * ch];
                for c in 0..ch {
                    out[c] += px[c] * wk;
                }
            }
        }
    }
    // Vertical pass.
    let mut out = vec![0.0f32; dh * dw * ch];
    for (y, (lo, w)) in wy.iter().enumerate() {
        for (k, wk) in w.iter().enumerate() {
            let row = &mid[(lo + k) * dw * ch..(lo + k + 1) * dw * ch];
            let dst = &mut out[y * dw * ch..(y + 1) * dw * ch];
            for (d, s) in dst.iter_mut().zip(row) {
                *d += s * wk;
            }
        }
    }
    out
}

fn target(dw: u32, dh: u32) -> Result<(usize, usize), AiError> {
    if dw == 0 || dh == 0 {
        return Err(AiError::invalid("cannot resize to an empty image"));
    }
    Ok((dw as usize, dh as usize))
}

/// Resizes an image. Same size returns a copy.
pub fn resize_image(img: &Image, dw: u32, dh: u32) -> Result<Image, AiError> {
    let (w, h) = target(dw, dh)?;
    if dw == img.width() && dh == img.height() {
        return Ok(img.clone());
    }
    let mut premul = Vec::with_capacity(img.rgba().len());
    for p in img.rgba().as_chunks::<4>().0 {
        let a = f32::from(p[3]) / 255.0;
        premul.extend([f32::from(p[0]) * a, f32::from(p[1]) * a, f32::from(p[2]) * a, f32::from(p[3])]);
    }
    let out = resample(&premul, img.width() as usize, img.height() as usize, 4, w, h);
    let mut bytes = Vec::with_capacity(out.len());
    for p in out.as_chunks::<4>().0 {
        let a = p[3].clamp(0.0, 255.0);
        let un = if a > 0.5 { 255.0 / a } else { 0.0 };
        bytes.extend([
            (p[0] * un).round().clamp(0.0, 255.0) as u8,
            (p[1] * un).round().clamp(0.0, 255.0) as u8,
            (p[2] * un).round().clamp(0.0, 255.0) as u8,
            a.round() as u8,
        ]);
    }
    Image::new(dw, dh, bytes)
}

/// Resizes a coverage mask. Same size returns a copy.
pub fn resize_mask(mask: &Mask, dw: u32, dh: u32) -> Result<Mask, AiError> {
    let (w, h) = target(dw, dh)?;
    if dw == mask.width() && dh == mask.height() {
        return Ok(mask.clone());
    }
    let src: Vec<f32> = mask.data().iter().map(|&v| f32::from(v)).collect();
    let out = resample(&src, mask.width() as usize, mask.height() as usize, 1, w, h);
    Mask::new(dw, dh, out.iter().map(|v| v.round().clamp(0.0, 255.0) as u8).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_flat_colour_stays_flat_at_any_size() {
        let img = Image::solid(37, 23, [200, 100, 50, 255]).unwrap();
        for (w, h) in [(5, 3), (37, 23), (100, 90), (1, 1), (64, 1)] {
            let out = resize_image(&img, w, h).unwrap();
            assert_eq!((out.width(), out.height()), (w, h));
            assert!(out.rgba().as_chunks::<4>().0.iter().all(|p| *p == [200, 100, 50, 255]), "{w}x{h}");
        }
    }

    #[test]
    fn shrinking_averages_instead_of_picking() {
        // Alternating black and white columns average to mid grey, not to one of the two.
        let mut d = Vec::new();
        for _y in 0..4 {
            for x in 0..8 {
                let v = if x % 2 == 0 { 0 } else { 255 };
                d.extend([v, v, v, 255]);
            }
        }
        let out = resize_image(&Image::new(8, 4, d).unwrap(), 2, 1).unwrap();
        for p in out.rgba().as_chunks::<4>().0 {
            assert!((100..=155).contains(&p[0]), "{p:?}");
        }
    }

    #[test]
    fn transparent_pixels_do_not_bleed_into_colour() {
        // Left half opaque red, right half fully transparent black.
        let mut d = Vec::new();
        for _y in 0..2 {
            for x in 0..4 {
                if x < 2 { d.extend([255, 0, 0, 255]) } else { d.extend([0, 0, 0, 0]) }
            }
        }
        let out = resize_image(&Image::new(4, 2, d).unwrap(), 2, 1).unwrap();
        // Averaging softens the alpha at the boundary, but the colour stays pure red: the
        // transparent black does not darken it.
        let inner = out.pixel(0, 0).unwrap();
        assert_eq!(&inner[..3], &[255, 0, 0]);
        let edge = out.pixel(1, 0).unwrap();
        assert!(edge[3] < inner[3], "{edge:?} vs {inner:?}");
        if edge[3] > 0 {
            assert!(edge[0] >= 250 && edge[1] == 0 && edge[2] == 0, "{edge:?}");
        }
    }

    #[test]
    fn masks_keep_their_coverage() {
        let m = Mask::new(8, 8, vec![255; 64]).unwrap();
        let out = resize_mask(&m, 3, 5).unwrap();
        assert!(out.data().iter().all(|&v| v == 255));
        let half = Mask::new(4, 1, vec![0, 0, 255, 255]).unwrap();
        let out = resize_mask(&half, 8, 1).unwrap();
        assert_eq!(out.coverage(0, 0), 0);
        assert_eq!(out.coverage(7, 0), 255);
    }

    #[test]
    fn empty_targets_are_errors_not_panics() {
        let img = Image::solid(2, 2, [0; 4]).unwrap();
        assert!(resize_image(&img, 0, 4).is_err());
        assert!(resize_mask(&Mask::new(2, 2, vec![0; 4]).unwrap(), 4, 0).is_err());
    }

    #[test]
    fn extreme_ratios_work() {
        let img = Image::solid(1, 1, [9, 8, 7, 255]).unwrap();
        assert_eq!(resize_image(&img, 50, 50).unwrap().pixel(49, 49), Some([9, 8, 7, 255]));
        let wide = Image::solid(3000, 2, [1, 2, 3, 255]).unwrap();
        assert_eq!(resize_image(&wide, 8, 2).unwrap().pixel(7, 1), Some([1, 2, 3, 255]));
    }
}
