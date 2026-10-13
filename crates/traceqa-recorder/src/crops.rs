//! Cuts component crops out of one page screenshot.

use std::fs;
use std::io;
use std::path::Path;

use crate::CropRequest;

/// Space left around a component so its border and focus ring are visible.
const PADDING: f64 = 4.0;

/// How a screenshot maps to page coordinates.
pub struct PageImage<'a> {
    pub path: &'a Path,
    /// Page y coordinate of the top edge of the image. 0 for a full-page screenshot.
    pub top: f64,
    /// Height in CSS pixels of the area the image shows.
    pub css_height: f64,
}

/// Writes one PNG per request into `crops_dir`. Returns, per request, whether a crop was written.
/// A request is skipped when its area is empty or lies outside the image.
pub fn save_crops(page: &PageImage, requests: &[CropRequest], crops_dir: &Path) -> io::Result<Vec<bool>> {
    let image = image::open(page.path).map_err(io::Error::other)?;
    let (width, height) = (f64::from(image.width()), f64::from(image.height()));
    // Screenshots are in device pixels, which differ from CSS pixels on high-density screens.
    let scale = if page.css_height > 0.0 {
        height / page.css_height
    } else {
        1.0
    };

    fs::create_dir_all(crops_dir)?;
    let mut written = Vec::with_capacity(requests.len());
    for request in requests {
        let area = &request.area;
        let left = ((area.x - PADDING) * scale).max(0.0);
        let top = ((area.y - page.top - PADDING) * scale).max(0.0);
        let right = ((area.x + area.w + PADDING) * scale).min(width);
        let bottom = ((area.y - page.top + area.h + PADDING) * scale).min(height);
        let visible = area.w > 0.0 && area.h > 0.0 && right - left >= 1.0 && bottom - top >= 1.0;
        if visible {
            let crop = image.crop_imm(left as u32, top as u32, (right - left) as u32, (bottom - top) as u32);
            crop.save(crops_dir.join(&request.file_name))
                .map_err(io::Error::other)?;
        }
        written.push(visible);
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;
    use traceqa_protocol::types::Rect;

    #[test]
    fn crops_are_scaled_padded_and_clamped() {
        let dir = std::env::temp_dir().join(format!("traceqa-crops-{}", crate::new_run_id()));
        fs::create_dir_all(&dir).unwrap();
        let page_path = dir.join("page.png");
        // A 200x400 CSS-pixel page captured at 2x.
        image::RgbImage::new(400, 800).save(&page_path).unwrap();
        let page = PageImage {
            path: &page_path,
            top: 0.0,
            css_height: 400.0,
        };

        let request = |name: &str, x, y, w, h| CropRequest {
            file_name: name.into(),
            area: Rect { x, y, w, h },
        };
        let requests = [
            request("inside.png", 50.0, 100.0, 40.0, 20.0),
            request("corner.png", 0.0, 0.0, 10.0, 10.0),
            request("hidden.png", 0.0, 0.0, 0.0, 0.0),
            request("below.png", 10.0, 900.0, 40.0, 20.0),
        ];
        let written = save_crops(&page, &requests, &dir.join("crops")).unwrap();
        assert_eq!(written, [true, true, false, false]);

        let size = |name: &str| image::image_dimensions(dir.join("crops").join(name)).unwrap();
        assert_eq!(size("inside.png"), (96, 56)); // (40 + 2*4) * 2 by (20 + 2*4) * 2
        assert_eq!(size("corner.png"), (28, 28)); // padding is cut off at the page edge
        fs::remove_dir_all(dir).unwrap();
    }
}
