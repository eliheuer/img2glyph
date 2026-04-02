use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub source: PathBuf,
    pub version: String,
    pub glyphs: Vec<GlyphEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GlyphEntry {
    /// Sequential ID assigned at segmentation time, e.g. "glyph_0001"
    pub id: String,
    /// Current filename (changes after labeling), e.g. "U+0041_A.png"
    pub file: String,
    /// Bounding box in the source image (before padding)
    pub bbox: BboxRecord,
    /// Number of foreground pixels in this glyph
    pub area_px: u32,
    /// Row index in reading order (0-based)
    pub row: u32,
    /// Column index within the row (0-based)
    pub col: u32,
    /// Unicode codepoint string, e.g. "U+0041"
    pub unicode: Option<String>,
    /// Unicode character name, e.g. "LATIN CAPITAL LETTER A"
    pub name: Option<String>,
    /// Labeling confidence 0.0–1.0 (set when using --llm)
    pub confidence: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BboxRecord {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}
