//! Pictures: screenshots and the crop of each component.

use std::path::Path;
use std::sync::atomic::AtomicBool;

use traceqa_protocol::types::{Component, PageScreenshotParams, Warning};
use traceqa_recorder::{save_crops, CropRequest, PageImage};

use super::frames::element_key;
use super::walk::Screen;
use super::warning;
use crate::driver::Driver;
use crate::EngineError;

/// Takes a screenshot and says whether it worked. Pictures are evidence, not data, so a
/// screenshot the browser could not take is noted once and the extraction goes on. A driver
/// that has stopped answering is a different matter and ends the extraction.
pub(super) fn screenshot(
    driver: &mut Driver,
    cancel: &AtomicBool,
    session_id: &str,
    path: &Path,
    full_page: bool,
    failure: &mut Option<Warning>,
) -> Result<bool, EngineError> {
    let taken = driver.until(cancel).screenshot(&PageScreenshotParams {
        session_id: session_id.to_string(),
        path: path.to_string_lossy().into_owned(),
        full_page: Some(full_page),
        clip: None,
    });
    match taken {
        Ok(_) => Ok(true),
        Err(error) if driver.is_lost() => Err(error.into()),
        Err(error) => {
            failure.get_or_insert_with(|| {
                warning(
                    "screenshot_failed",
                    format!(
                        "A screenshot could not be taken, so some crops are missing: {}",
                        error.message()
                    ),
                )
            });
            Ok(false)
        }
    }
}

/// Saves one crop per component and returns how many were saved.
///
/// The crop comes from the screen that showed the most of the component's first instance that
/// was ever on screen. A component that never appeared while scrolling gets none.
pub(super) fn save_component_crops(
    components: &mut [Component],
    screens: &[Screen],
    viewport_width: f64,
    dir: &Path,
) -> Result<usize, EngineError> {
    // For each screen, the crops to cut from it and the component each belongs to.
    let mut wanted: Vec<Vec<(usize, CropRequest)>> = screens.iter().map(|_| Vec::new()).collect();
    for (index, component) in components.iter().enumerate() {
        let best = component.instances.iter().find_map(|instance| {
            let key = element_key(&instance.frame_id, &instance.selectors);
            screens
                .iter()
                .enumerate()
                .filter_map(|(number, screen)| {
                    let area = screen.shown.get(&key)?;
                    let coverage = screen.coverage(area, viewport_width);
                    (coverage > 0.0).then(|| (number, area.clone(), coverage))
                })
                // The earliest of the screens that show the most of it.
                .reduce(|best, next| if next.2 > best.2 { next } else { best })
        });
        if let Some((number, area, _)) = best {
            wanted[number].push((
                index,
                CropRequest {
                    file_name: format!("{}.png", component.id),
                    area,
                },
            ));
        }
    }

    let mut saved = 0;
    for (screen, wanted) in screens.iter().zip(wanted) {
        let (owners, requests): (Vec<usize>, Vec<CropRequest>) = wanted.into_iter().unzip();
        let image = PageImage {
            path: &screen.image,
            top: screen.top,
            css_height: screen.height,
        };
        for ((index, request), written) in
            owners
                .into_iter()
                .zip(&requests)
                .zip(save_crops(&image, &requests, &dir.join("crops"))?)
        {
            if written {
                components[index].crop = Some(format!("crops/{}", request.file_name));
                saved += 1;
            }
        }
    }
    Ok(saved)
}
