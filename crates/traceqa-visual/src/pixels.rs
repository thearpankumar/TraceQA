//! Pixel comparison of two same-sized RGBA images.
//!
//! The method is the one pixelmatch and Playwright's `toHaveScreenshot` use: a per-pixel distance
//! in YIQ colour space, scaled by a threshold, with anti-aliased pixels detected and ignored.
//!
//! Ported from pixelmatch (https://github.com/mapbox/pixelmatch, ISC licence, Copyright (c) 2019
//! Mapbox), whose anti-aliasing detector follows Vysniauskas, "Anti-aliased pixel and intensity
//! slope detector" (2009). This is a re-implementation in Rust of the 5.x/7.x algorithm (YIQ);
//! pixelmatch 8 switched its distance to OKLab, which this does not follow, so results agree with
//! Playwright's rather than with the newest pixelmatch.

use std::io::Cursor;

use image::{ImageFormat, ImageReader, Limits, RgbaImage};

/// The largest width or height of a crop that is decoded. A crop is a component, not a page.
pub const MAX_SIDE: u32 = 8192;
/// The most memory the PNG decoder may allocate for one image.
pub const MAX_DECODE_BYTES: u64 = 128 * 1024 * 1024;

/// A decoded image: `width * height * 4` bytes of RGBA, rows top to bottom.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rgba {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

impl Rgba {
    /// A blank (transparent) image, for tests and callers that build pictures.
    pub fn filled(width: u32, height: u32, pixel: [u8; 4]) -> Self {
        let data = pixel
            .iter()
            .copied()
            .cycle()
            .take(width as usize * height as usize * 4)
            .collect();
        Self { width, height, data }
    }

    pub fn set(&mut self, x: u32, y: u32, pixel: [u8; 4]) {
        let at = self.at(x as usize, y as usize);
        self.data[at..at + 4].copy_from_slice(&pixel);
    }

    fn at(&self, x: usize, y: usize) -> usize {
        (y * self.width as usize + x) * 4
    }
}

/// Decodes PNG bytes, refusing images larger than [`MAX_SIDE`] or that need more than
/// [`MAX_DECODE_BYTES`] to decode, so a hostile run folder cannot exhaust memory.
pub fn decode_png(bytes: &[u8]) -> Result<Rgba, String> {
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_SIDE);
    limits.max_image_height = Some(MAX_SIDE);
    limits.max_alloc = Some(MAX_DECODE_BYTES);
    let mut reader = ImageReader::with_format(Cursor::new(bytes), ImageFormat::Png);
    reader.limits(limits);
    let image = reader.decode().map_err(|e| e.to_string())?.into_rgba8();
    let (width, height) = image.dimensions();
    Ok(Rgba {
        width,
        height,
        data: image.into_raw(),
    })
}

/// The result of comparing two images.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Comparison {
    /// Pixels that differ beyond the threshold and are not anti-aliasing.
    pub diff_pixels: u64,
    pub total_pixels: u64,
    /// One flag per pixel, true where it differs.
    pub mask: Vec<bool>,
}

impl Comparison {
    pub fn ratio(&self) -> f64 {
        if self.total_pixels == 0 {
            0.0
        } else {
            self.diff_pixels as f64 / self.total_pixels as f64
        }
    }
}

/// Compares two images of equal size. `threshold` is 0 to 1 (clamped); the distance a pixel may
/// have is `35215 * threshold^2` in the units of [`color_delta`]. Returns `None` when the sizes differ.
pub fn compare(a: &Rgba, b: &Rgba, threshold: f64) -> Option<Comparison> {
    if a.width != b.width || a.height != b.height {
        return None;
    }
    let (width, height) = (a.width as usize, a.height as usize);
    let threshold = threshold.clamp(0.0, 1.0);
    let max_delta = 35215.0 * threshold * threshold;
    let mut mask = vec![false; width * height];
    let mut diff_pixels = 0u64;
    for y in 0..height {
        for x in 0..width {
            let at = (y * width + x) * 4;
            let delta = color_delta(&a.data, &b.data, at, at, false);
            if delta.abs() > max_delta && !(antialiased(a, x, y, b) || antialiased(b, x, y, a)) {
                mask[y * width + x] = true;
                diff_pixels += 1;
            }
        }
    }
    Some(Comparison {
        diff_pixels,
        total_pixels: (width * height) as u64,
        mask,
    })
}

/// Blends a colour channel with alpha over white.
fn blend(channel: f64, alpha: f64) -> f64 {
    255.0 + (channel - 255.0) * alpha
}

fn rgb2y(r: f64, g: f64, b: f64) -> f64 {
    r * 0.298_895_31 + g * 0.586_622_47 + b * 0.114_482_23
}

fn rgb2i(r: f64, g: f64, b: f64) -> f64 {
    r * 0.595_977_99 - g * 0.274_176_10 - b * 0.321_801_89
}

fn rgb2q(r: f64, g: f64, b: f64) -> f64 {
    r * 0.211_470_17 - g * 0.522_617_11 + b * 0.311_146_94
}

/// The squared YIQ distance of two pixels (weights 0.5053, 0.299, 0.1957), with alpha blended over
/// white. As in pixelmatch the sign says which pixel is brighter: negative when the first is.
/// With `y_only` only the brightness difference is returned.
fn color_delta(a: &[u8], b: &[u8], k: usize, m: usize, y_only: bool) -> f64 {
    let (mut r1, mut g1, mut b1, a1) = (
        f64::from(a[k]),
        f64::from(a[k + 1]),
        f64::from(a[k + 2]),
        f64::from(a[k + 3]),
    );
    let (mut r2, mut g2, mut b2, a2) = (
        f64::from(b[m]),
        f64::from(b[m + 1]),
        f64::from(b[m + 2]),
        f64::from(b[m + 3]),
    );
    if (a1 - a2).abs() < f64::EPSILON && r1 == r2 && g1 == g2 && b1 == b2 {
        return 0.0;
    }
    if a1 < 255.0 {
        let alpha = a1 / 255.0;
        r1 = blend(r1, alpha);
        g1 = blend(g1, alpha);
        b1 = blend(b1, alpha);
    }
    if a2 < 255.0 {
        let alpha = a2 / 255.0;
        r2 = blend(r2, alpha);
        g2 = blend(g2, alpha);
        b2 = blend(b2, alpha);
    }
    let y = rgb2y(r1, g1, b1) - rgb2y(r2, g2, b2);
    if y_only {
        return y;
    }
    let i = rgb2i(r1, g1, b1) - rgb2i(r2, g2, b2);
    let q = rgb2q(r1, g1, b1) - rgb2q(r2, g2, b2);
    let delta = 0.5053 * y * y + 0.299 * i * i + 0.1957 * q * q;
    if y > 0.0 {
        -delta
    } else {
        delta
    }
}

/// Whether the pixel at (x1, y1) of `img` looks anti-aliased: it has neighbours both darker and
/// lighter, and the darkest or the lightest of them has many identical siblings in both images
/// (Vysniauskas 2009, as in pixelmatch).
fn antialiased(img: &Rgba, x1: usize, y1: usize, other: &Rgba) -> bool {
    let (width, height) = (img.width as usize, img.height as usize);
    let x0 = x1.saturating_sub(1);
    let y0 = y1.saturating_sub(1);
    let x2 = (x1 + 1).min(width - 1);
    let y2 = (y1 + 1).min(height - 1);
    let pos = img.at(x1, y1);
    let mut zeroes = usize::from(x1 == x0 || x1 == x2 || y1 == y0 || y1 == y2);
    let (mut min, mut max) = (0.0f64, 0.0f64);
    let (mut min_at, mut max_at) = ((0, 0), (0, 0));
    for x in x0..=x2 {
        for y in y0..=y2 {
            if x == x1 && y == y1 {
                continue;
            }
            let delta = color_delta(&img.data, &img.data, pos, img.at(x, y), true);
            if delta == 0.0 {
                zeroes += 1;
                if zeroes > 2 {
                    return false;
                }
            } else if delta < min {
                min = delta;
                min_at = (x, y);
            } else if delta > max {
                max = delta;
                max_at = (x, y);
            }
        }
    }
    if min == 0.0 || max == 0.0 {
        return false;
    }
    (has_many_siblings(img, min_at) && has_many_siblings(other, min_at))
        || (has_many_siblings(img, max_at) && has_many_siblings(other, max_at))
}

/// Whether more than two neighbours of the pixel have exactly its colour.
fn has_many_siblings(img: &Rgba, (x1, y1): (usize, usize)) -> bool {
    let (width, height) = (img.width as usize, img.height as usize);
    let x0 = x1.saturating_sub(1);
    let y0 = y1.saturating_sub(1);
    let x2 = (x1 + 1).min(width - 1);
    let y2 = (y1 + 1).min(height - 1);
    let pos = img.at(x1, y1);
    let mut zeroes = usize::from(x1 == x0 || x1 == x2 || y1 == y0 || y1 == y2);
    for x in x0..=x2 {
        for y in y0..=y2 {
            if x == x1 && y == y1 {
                continue;
            }
            let at = img.at(x, y);
            if img.data[pos..pos + 4] == img.data[at..at + 4] {
                zeroes += 1;
            }
            if zeroes > 2 {
                return true;
            }
        }
    }
    false
}

/// A picture of the differences: the second image faded to a light grey, with the differing pixels red.
pub fn diff_image(b: &Rgba, comparison: &Comparison) -> Option<RgbaImage> {
    let mut out = Vec::with_capacity(b.data.len());
    for (index, differs) in comparison.mask.iter().enumerate() {
        if *differs {
            out.extend_from_slice(&[255, 0, 0, 255]);
            continue;
        }
        let at = index * 4;
        let alpha = f64::from(b.data[at + 3]) / 255.0;
        let grey = rgb2y(
            blend(f64::from(b.data[at]), alpha),
            blend(f64::from(b.data[at + 1]), alpha),
            blend(f64::from(b.data[at + 2]), alpha),
        );
        // 90% towards white, as pixelmatch's drawGray does with alpha 0.1.
        let faded = (255.0 + (grey - 255.0) * 0.1).round().clamp(0.0, 255.0) as u8;
        out.extend_from_slice(&[faded, faded, faded, 255]);
    }
    RgbaImage::from_raw(b.width, b.height, out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const WHITE: [u8; 4] = [255, 255, 255, 255];
    const BLACK: [u8; 4] = [0, 0, 0, 255];

    fn count(a: &Rgba, b: &Rgba, threshold: f64) -> u64 {
        compare(a, b, threshold).expect("same size").diff_pixels
    }

    #[test]
    fn identical_images_have_no_differences() {
        let a = Rgba::filled(20, 10, [10, 120, 200, 255]);
        let result = compare(&a, &a.clone(), 0.2).expect("same size");
        assert_eq!((result.diff_pixels, result.total_pixels), (0, 200));
        assert_eq!(result.ratio(), 0.0);
    }

    #[test]
    fn different_sizes_are_not_compared() {
        assert!(compare(&Rgba::filled(4, 4, WHITE), &Rgba::filled(4, 5, WHITE), 0.2).is_none());
    }

    #[test]
    fn one_pixel_is_counted_and_the_ratio_decides() {
        let a = Rgba::filled(100, 100, WHITE);
        let mut one = a.clone();
        one.set(50, 50, BLACK);
        let result = compare(&a, &one, 0.2).expect("same size");
        assert_eq!(result.diff_pixels, 1);
        assert!(result.ratio() <= 0.001, "1 of 10000 is within a ratio of 0.001");

        let mut twenty = a.clone();
        for x in 0..20 {
            twenty.set(x * 4, 10, BLACK);
        }
        let result = compare(&a, &twenty, 0.2).expect("same size");
        assert_eq!(result.diff_pixels, 20);
        assert!(result.ratio() > 0.001, "20 of 10000 is over a ratio of 0.001");
    }

    #[test]
    fn an_anti_aliased_edge_shift_is_ignored() {
        // A hard edge between column 4 and 5; B draws column 4 as an intermediate grey.
        let mut a = Rgba::filled(10, 10, WHITE);
        for y in 0..10 {
            for x in 0..5 {
                a.set(x, y, BLACK);
            }
        }
        let mut b = a.clone();
        for y in 0..10 {
            b.set(4, y, [128, 128, 128, 255]);
        }
        assert_eq!(count(&a, &b, 0.2), 0, "the grey column is anti-aliasing");
        // A solid block moving by a pixel is not anti-aliasing.
        let mut moved = Rgba::filled(10, 10, WHITE);
        for y in 0..10 {
            for x in 0..4 {
                moved.set(x, y, BLACK);
            }
        }
        assert!(
            count(&a, &moved, 0.2) > 0,
            "a hard one-column shift is a real difference"
        );
    }

    #[test]
    fn alpha_is_blended_over_white() {
        let white = Rgba::filled(4, 4, WHITE);
        // Fully transparent black is white once blended.
        assert_eq!(count(&white, &Rgba::filled(4, 4, [0, 0, 0, 0]), 0.0), 0);
        // Half transparent black is mid grey: a difference.
        assert_eq!(count(&white, &Rgba::filled(4, 4, [0, 0, 0, 128]), 0.2), 16);
    }

    #[test]
    fn a_higher_threshold_never_finds_more_differences() {
        let a = Rgba::filled(30, 30, [100, 100, 100, 255]);
        let mut b = a.clone();
        for x in 0..30 {
            for y in 0..30 {
                let shade = 100 + ((x + 7 * y) % 60) as u8;
                b.set(x, y, [shade, shade, shade, 255]);
            }
        }
        let counts: Vec<u64> = [0.0, 0.05, 0.1, 0.2, 0.4, 0.8, 1.0]
            .iter()
            .map(|t| count(&a, &b, *t))
            .collect();
        assert!(counts.windows(2).all(|w| w[0] >= w[1]), "{counts:?}");
        assert!(counts[0] > 0 && counts[counts.len() - 1] == 0, "{counts:?}");
    }

    #[test]
    fn the_diff_picture_marks_exactly_the_differing_pixels() {
        let a = Rgba::filled(5, 5, WHITE);
        let mut b = a.clone();
        b.set(2, 3, BLACK);
        let result = compare(&a, &b, 0.2).expect("same size");
        let picture = diff_image(&b, &result).expect("sizes match");
        assert_eq!(picture.get_pixel(2, 3).0, [255, 0, 0, 255]);
        assert_eq!(picture.get_pixel(0, 0).0, WHITE);
    }

    #[test]
    fn png_decoding_round_trips_and_refuses_junk() {
        let mut image = RgbaImage::new(3, 2);
        image.put_pixel(1, 1, image::Rgba([1, 2, 3, 255]));
        let mut bytes = Vec::new();
        image
            .write_to(&mut Cursor::new(&mut bytes), ImageFormat::Png)
            .expect("encodes");
        let decoded = decode_png(&bytes).expect("decodes");
        assert_eq!((decoded.width, decoded.height), (3, 2));
        assert_eq!(&decoded.data[(3 + 1) * 4..(3 + 1) * 4 + 4], &[1, 2, 3, 255]);
        assert!(decode_png(b"not a png").is_err());
    }

    #[test]
    fn a_huge_image_is_refused_before_it_is_allocated() {
        let image = RgbaImage::new(MAX_SIDE + 1, 1);
        let mut bytes = Vec::new();
        image
            .write_to(&mut Cursor::new(&mut bytes), ImageFormat::Png)
            .expect("encodes");
        assert!(decode_png(&bytes).is_err());
    }
}
