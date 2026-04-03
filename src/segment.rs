use image::{GrayImage, ImageBuffer, Luma};
use imageproc::contrast::adaptive_threshold;
use imageproc::distance_transform::Norm;
use imageproc::morphology::dilate;
use imageproc::region_labelling::{connected_components, Connectivity};
use std::collections::HashMap;

/// Label image type: one u32 label per pixel from connected-component analysis.
pub type LabelImage = ImageBuffer<Luma<u32>, Vec<u32>>;

/// Bounding box of a segmented glyph, with reading-order coordinates.
#[derive(Debug, Clone)]
pub struct GlyphBbox {
    /// Left edge in source image pixels
    pub x: u32,
    /// Top edge in source image pixels
    pub y: u32,
    /// Width in pixels
    pub w: u32,
    /// Height in pixels
    pub h: u32,
    /// Number of foreground pixels (ink area)
    pub area: u32,
    /// Row index assigned by reading-order sort
    pub row: u32,
    /// Column index within row, assigned by reading-order sort
    pub col: u32,
    /// Connected-component label in the label image.
    pub label: u32,
}

/// Find all glyph bounding boxes in a grayscale image.
///
/// Assumes dark ink on a light background. The pipeline is:
/// adaptive threshold → connected components → area filter → reading-order sort.
pub fn find_glyphs(
    gray: &GrayImage,
    min_area: u32,
    max_area: u32,
    block_radius: u32,
) -> (Vec<GlyphBbox>, LabelImage) {
    // Binary image: paper → 255 (background), ink → 0 (foreground).
    // c=0: threshold equals the local mean (standard adaptive binarization).
    let binary = adaptive_threshold(gray, block_radius, 0);

    // Label connected ink components; white pixels are background.
    let labels = connected_components(&binary, Connectivity::Eight, Luma([255u8]));

    // Accumulate per-label: (x_min, y_min, x_max, y_max, pixel_count)
    let mut bounds: HashMap<u32, (u32, u32, u32, u32, u32)> = HashMap::new();

    for y in 0..labels.height() {
        for x in 0..labels.width() {
            let label = labels.get_pixel(x, y)[0];
            if label == 0 {
                continue; // background
            }
            let e = bounds.entry(label).or_insert((x, y, x, y, 0));
            if x < e.0 { e.0 = x; }
            if y < e.1 { e.1 = y; }
            if x > e.2 { e.2 = x; }
            if y > e.3 { e.3 = y; }
            e.4 += 1;
        }
    }

    let mut glyphs: Vec<GlyphBbox> = bounds
        .iter()
        .filter(|(_, (_, _, _, _, area))| *area >= min_area && *area <= max_area)
        .map(|(label, (x0, y0, x1, y1, area))| GlyphBbox {
            x: *x0,
            y: *y0,
            w: x1 - x0 + 1,
            h: y1 - y0 + 1,
            area: *area,
            row: 0,
            col: 0,
            label: *label,
        })
        .collect();

    assign_reading_order(&mut glyphs);
    (glyphs, labels)
}

/// Crop a single glyph from the source image, adding `padding` pixels on each side.
///
/// Pixels that belong to other connected components (neighbouring glyphs)
/// are set to white (255) so only the target glyph's ink remains.
pub fn extract_glyph(
    gray: &GrayImage,
    bbox: &GlyphBbox,
    padding: u32,
    labels: &LabelImage,
) -> GrayImage {
    let (img_w, img_h) = gray.dimensions();
    let x0 = bbox.x.saturating_sub(padding);
    let y0 = bbox.y.saturating_sub(padding);
    let x1 = (bbox.x + bbox.w + padding).min(img_w);
    let y1 = (bbox.y + bbox.h + padding).min(img_h);

    let crop_w = x1 - x0;
    let crop_h = y1 - y0;
    let mut out = GrayImage::new(crop_w, crop_h);

    // Build a binary mask of the target component, then dilate it by a few
    // pixels to capture anti-aliased edges that the adaptive threshold missed.
    let mut mask = GrayImage::new(crop_w, crop_h);
    for cy in 0..crop_h {
        for cx in 0..crop_w {
            let src_x = x0 + cx;
            let src_y = y0 + cy;
            if labels.get_pixel(src_x, src_y)[0] == bbox.label {
                mask.put_pixel(cx, cy, Luma([255u8]));
            }
        }
    }
    let mask = dilate(&mask, Norm::LInf, 8);

    // Output original grayscale where the dilated mask covers (glyph + edges),
    // pure white everywhere else (clean background for downstream thresholding).
    for cy in 0..crop_h {
        for cx in 0..crop_w {
            if mask.get_pixel(cx, cy)[0] > 0 {
                let src_x = x0 + cx;
                let src_y = y0 + cy;
                out.put_pixel(cx, cy, *gray.get_pixel(src_x, src_y));
            } else {
                out.put_pixel(cx, cy, Luma([255u8]));
            }
        }
    }

    out
}

/// Sort glyphs into reading order (top→bottom, left→right) and assign row/col indices.
fn assign_reading_order(glyphs: &mut Vec<GlyphBbox>) {
    if glyphs.is_empty() {
        return;
    }

    // Sort by vertical center for initial row grouping.
    glyphs.sort_by_key(|g| g.y + g.h / 2);

    // Use 2/3 of the median glyph height as the row-change threshold.
    let mut heights: Vec<u32> = glyphs.iter().map(|g| g.h).collect();
    heights.sort_unstable();
    let median_h = heights[heights.len() / 2].max(1);
    let tolerance = median_h * 2 / 3;

    // Walk down and bump the row counter whenever y-center jumps by more than tolerance.
    let mut row = 0u32;
    let mut last_cy = glyphs[0].y + glyphs[0].h / 2;

    for glyph in glyphs.iter_mut() {
        let cy = glyph.y + glyph.h / 2;
        if cy.abs_diff(last_cy) > tolerance {
            row += 1;
            last_cy = cy;
        }
        glyph.row = row;
    }

    // Final sort: by row then by x for left-to-right column order.
    glyphs.sort_by(|a, b| a.row.cmp(&b.row).then(a.x.cmp(&b.x)));

    // Assign column indices within each row.
    let mut col = 0u32;
    let mut prev_row = 0u32;
    for glyph in glyphs.iter_mut() {
        if glyph.row != prev_row {
            col = 0;
            prev_row = glyph.row;
        }
        glyph.col = col;
        col += 1;
    }
}
