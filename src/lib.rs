//! img2glyph — Convert scanned type specimens into individual named glyph PNGs.
//!
//! # Library usage
//!
//! ```no_run
//! use img2glyph::{SegmentConfig, segment_image, extract_glyph};
//!
//! let image = image::open("specimen.png").unwrap();
//! let config = SegmentConfig::default();
//!
//! let bboxes = segment_image(&image, &config);
//! for bbox in &bboxes {
//!     let glyph_img = extract_glyph(&image, bbox, config.padding);
//!     // glyph_img is a GrayImage cropped from the source
//! }
//! ```

pub mod agl;
pub mod manifest;
pub mod segment;

pub use agl::agl_name;
pub use manifest::{BboxRecord, GlyphEntry, Manifest};
pub use segment::{GlyphBbox, LabelImage};

/// Configuration for the segmentation pipeline.
///
/// All parameters have sensible defaults via [`SegmentConfig::default`].
#[derive(Debug, Clone)]
pub struct SegmentConfig {
    /// Minimum glyph area in pixels. Components smaller than this are discarded as noise.
    pub min_area: u32,
    /// Maximum glyph area in pixels. Components larger than this are discarded (borders, rules).
    pub max_area: u32,
    /// Adaptive threshold block radius. Larger values handle more uneven lighting.
    pub block_radius: u32,
    /// Padding in pixels added around each cropped glyph.
    pub padding: u32,
}

impl Default for SegmentConfig {
    fn default() -> Self {
        Self {
            min_area: 200,
            max_area: 50_000,
            block_radius: 15,
            padding: 10,
        }
    }
}

/// Segment a type specimen image into individual glyph bounding boxes.
///
/// Returns bounding boxes sorted in reading order (top→bottom, left→right)
/// and the connected-component label image (needed by [`extract_glyph`]).
pub fn segment_image(image: &image::DynamicImage, config: &SegmentConfig) -> (Vec<GlyphBbox>, LabelImage) {
    let gray = image.to_luma8();
    segment::find_glyphs(&gray, config.min_area, config.max_area, config.block_radius)
}

/// Crop a single glyph from a source image, adding `padding` pixels on each side.
///
/// Uses the label image from [`segment_image`] to mask out neighbouring glyphs,
/// so only the target connected component's ink appears in the output.
pub fn extract_glyph(
    image: &image::DynamicImage,
    bbox: &GlyphBbox,
    padding: u32,
    labels: &LabelImage,
) -> image::GrayImage {
    let gray = image.to_luma8();
    segment::extract_glyph(&gray, bbox, padding, labels)
}

/// Fill in the `glyph_name` field (AGL name) for every entry that has a Unicode codepoint.
///
/// Call this after assigning `unicode` values to entries, before writing files or a manifest.
pub fn populate_glyph_names(glyphs: &mut Vec<GlyphEntry>) {
    for glyph in glyphs.iter_mut() {
        if let Some(unicode) = &glyph.unicode {
            glyph.glyph_name = Some(agl_name(unicode));
        }
    }
}
