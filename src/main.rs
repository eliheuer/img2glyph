mod llm;
mod manifest;
mod segment;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use manifest::{BboxRecord, GlyphEntry, Manifest};
use std::path::{Path, PathBuf};

#[derive(Parser)]
#[command(
    name = "img2glyph",
    about = "Convert scanned type specimens into individual named glyph PNGs",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Segment a type specimen and extract individual glyph PNGs
    Process {
        /// Input image (PNG, JPEG, TIFF, BMP, …)
        image: PathBuf,

        /// Output directory for glyph PNGs and manifest.json
        #[arg(short, long, default_value = "glyphs")]
        output: PathBuf,

        /// Call the Claude API to label every glyph with its Unicode codepoint.
        /// Requires ANTHROPIC_API_KEY to be set in the environment.
        #[arg(long)]
        llm: bool,

        /// Padding in pixels added around each cropped glyph
        #[arg(long, default_value_t = 10)]
        padding: u32,

        /// Minimum glyph area in pixels — raise this to filter scan noise
        #[arg(long, default_value_t = 200)]
        min_area: u32,

        /// Maximum glyph area in pixels — lower this to exclude large page elements
        #[arg(long, default_value_t = 50_000)]
        max_area: u32,

        /// Adaptive threshold block radius — larger values handle more uneven lighting
        #[arg(long, default_value_t = 15)]
        block_radius: u32,
    },

    /// Apply Unicode labels to an existing manifest produced by `process`
    Label {
        /// Path to manifest.json
        manifest: PathBuf,

        /// JSON assignments file mapping glyph IDs to Unicode codepoints.
        ///
        /// Two supported formats:
        ///
        ///   Sequence (assigns by reading order):
        ///   {"sequence": "ABCabc…"}
        ///
        ///   Per-glyph (explicit mapping):
        ///   {"glyph_0001": {"unicode": "U+0041", "name": "LATIN CAPITAL LETTER A"}}
        #[arg(long)]
        assignments: Option<PathBuf>,

        /// Call the Claude API to label all unlabeled glyphs.
        /// Requires ANTHROPIC_API_KEY to be set in the environment.
        #[arg(long)]
        llm: bool,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Process {
            image,
            output,
            llm,
            padding,
            min_area,
            max_area,
            block_radius,
        } => cmd_process(image, output, llm, padding, min_area, max_area, block_radius).await,

        Commands::Label {
            manifest,
            assignments,
            llm,
        } => cmd_label(manifest, assignments, llm).await,
    }
}

// ---------------------------------------------------------------------------
// process
// ---------------------------------------------------------------------------

async fn cmd_process(
    image_path: PathBuf,
    output_dir: PathBuf,
    use_llm: bool,
    padding: u32,
    min_area: u32,
    max_area: u32,
    block_radius: u32,
) -> Result<()> {
    eprintln!("Loading {}…", image_path.display());
    let img = image::open(&image_path)
        .with_context(|| format!("Cannot open {}", image_path.display()))?;
    let gray = img.into_luma8();

    std::fs::create_dir_all(&output_dir)
        .with_context(|| format!("Cannot create directory {}", output_dir.display()))?;

    eprintln!("Segmenting…");
    let bboxes = segment::find_glyphs(&gray, min_area, max_area, block_radius);
    eprintln!("Found {} glyph candidates", bboxes.len());

    // Extract and save each glyph as a numbered PNG.
    let mut entries: Vec<GlyphEntry> = Vec::with_capacity(bboxes.len());

    for (idx, bbox) in bboxes.iter().enumerate() {
        let id = format!("glyph_{:04}", idx + 1);
        let filename = format!("{}.png", id);
        let out_path = output_dir.join(&filename);

        let cropped = segment::extract_glyph(&gray, bbox, padding);
        cropped
            .save(&out_path)
            .with_context(|| format!("Cannot save {}", out_path.display()))?;

        entries.push(GlyphEntry {
            id,
            file: filename,
            bbox: BboxRecord {
                x: bbox.x,
                y: bbox.y,
                w: bbox.w,
                h: bbox.h,
            },
            area_px: bbox.area,
            row: bbox.row,
            col: bbox.col,
            unicode: None,
            name: None,
            confidence: None,
        });
    }

    eprintln!("Extracted {} glyphs → {}", entries.len(), output_dir.display());

    // Optional LLM labeling pass.
    if use_llm {
        let api_key = std::env::var("ANTHROPIC_API_KEY")
            .context("ANTHROPIC_API_KEY is not set")?;
        eprintln!("Labeling with Claude API…");
        llm::label_all(&mut entries, &output_dir, &api_key).await?;
        rename_labeled(&mut entries, &output_dir)?;
    }

    write_manifest(&output_dir, &image_path, entries)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// label
// ---------------------------------------------------------------------------

async fn cmd_label(
    manifest_path: PathBuf,
    assignments_path: Option<PathBuf>,
    use_llm: bool,
) -> Result<()> {
    let json = std::fs::read_to_string(&manifest_path)
        .with_context(|| format!("Cannot read {}", manifest_path.display()))?;
    let mut manifest: Manifest = serde_json::from_str(&json)
        .context("manifest.json is not valid")?;

    let output_dir = manifest_path
        .parent()
        .unwrap_or(Path::new("."))
        .to_path_buf();

    if let Some(path) = assignments_path {
        apply_assignments(&mut manifest.glyphs, &path, &output_dir)?;
    }

    if use_llm {
        let api_key = std::env::var("ANTHROPIC_API_KEY")
            .context("ANTHROPIC_API_KEY is not set")?;

        let unlabeled: Vec<usize> = manifest
            .glyphs
            .iter()
            .enumerate()
            .filter(|(_, g)| g.unicode.is_none())
            .map(|(i, _)| i)
            .collect();

        if unlabeled.is_empty() {
            eprintln!("All glyphs are already labeled; nothing to do.");
        } else {
            eprintln!("Labeling {} unlabeled glyph(s) with Claude API…", unlabeled.len());
            llm::label_at(&mut manifest.glyphs, &output_dir, &api_key, &unlabeled).await?;
            rename_labeled(&mut manifest.glyphs, &output_dir)?;
        }
    }

    let updated = serde_json::to_string_pretty(&manifest)?;
    std::fs::write(&manifest_path, updated)?;
    eprintln!("Updated {}", manifest_path.display());
    Ok(())
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

/// Read an assignments JSON file and apply the mappings to `glyphs`.
/// Renames files immediately after assigning labels.
fn apply_assignments(
    glyphs: &mut Vec<GlyphEntry>,
    assignments_path: &Path,
    output_dir: &Path,
) -> Result<()> {
    let json = std::fs::read_to_string(assignments_path)
        .with_context(|| format!("Cannot read {}", assignments_path.display()))?;
    let value: serde_json::Value = serde_json::from_str(&json)?;

    if let Some(seq) = value.get("sequence").and_then(|v| v.as_str()) {
        // Assign characters in reading order.
        for (glyph, ch) in glyphs.iter_mut().zip(seq.chars()) {
            let (unicode, name) = char_unicode_info(ch);
            glyph.unicode = Some(unicode);
            glyph.name = Some(name);
        }
    } else if let Some(obj) = value.as_object() {
        // Assign by glyph ID.
        for glyph in glyphs.iter_mut() {
            if let Some(entry) = obj.get(&glyph.id) {
                glyph.unicode = entry
                    .get("unicode")
                    .and_then(|v| v.as_str())
                    .map(str::to_string);
                glyph.name = entry
                    .get("name")
                    .and_then(|v| v.as_str())
                    .map(str::to_string);
            }
        }
    }

    rename_labeled(glyphs, output_dir)
}

/// Rename each labeled glyph file to its Unicode-based name.
fn rename_labeled(glyphs: &mut Vec<GlyphEntry>, output_dir: &Path) -> Result<()> {
    for glyph in glyphs.iter_mut() {
        let Some(unicode) = &glyph.unicode else { continue };
        let name = glyph.name.as_deref().unwrap_or("UNKNOWN");
        let new_file = unicode_filename(unicode, name);
        let old_path = output_dir.join(&glyph.file);
        let new_path = output_dir.join(&new_file);

        if old_path != new_path {
            if old_path.exists() {
                std::fs::rename(&old_path, &new_path)
                    .with_context(|| {
                        format!("Cannot rename {} → {}", old_path.display(), new_path.display())
                    })?;
            }
            glyph.file = new_file;
        }
    }
    Ok(())
}

/// Build a safe filename from a Unicode codepoint string and character name.
/// Examples: "U+0041_A.png", "U+00B6_PILCROW_SIGN.png"
fn unicode_filename(unicode: &str, name: &str) -> String {
    // Attempt a short single-char suffix for printable characters.
    let cp_str = unicode
        .trim_start_matches("U+")
        .trim_start_matches("u+");

    let short = u32::from_str_radix(cp_str, 16)
        .ok()
        .and_then(char::from_u32)
        .filter(|c| c.is_alphanumeric() && !c.is_control())
        .map(|c| c.to_string())
        .unwrap_or_else(|| {
            // Fall back to a sanitized version of the Unicode name.
            name.chars()
                .map(|c| if c.is_alphanumeric() { c } else { '_' })
                .collect()
        });

    format!("{}_{}.png", unicode.to_uppercase(), short)
}

/// Serialize and write the manifest file.
fn write_manifest(
    output_dir: &Path,
    source: &Path,
    glyphs: Vec<GlyphEntry>,
) -> Result<()> {
    let manifest = Manifest {
        source: source.to_path_buf(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        glyphs,
    };
    let json = serde_json::to_string_pretty(&manifest)?;
    let path = output_dir.join("manifest.json");
    std::fs::write(&path, json)?;
    eprintln!("Wrote {}", path.display());
    Ok(())
}

/// Return the Unicode codepoint string and name for a Rust `char`.
fn char_unicode_info(c: char) -> (String, String) {
    let cp = c as u32;
    (format!("U+{:04X}", cp), unicode_char_name(c))
}

/// Return the Unicode character name for common characters.
/// For everything else, returns a generic "CHARACTER U+XXXX" placeholder.
fn unicode_char_name(c: char) -> String {
    match c {
        'A'..='Z' => format!("LATIN CAPITAL LETTER {}", c),
        'a'..='z' => {
            let upper: String = c.to_uppercase().collect();
            format!("LATIN SMALL LETTER {}", upper)
        }
        '0' => "DIGIT ZERO".into(),
        '1' => "DIGIT ONE".into(),
        '2' => "DIGIT TWO".into(),
        '3' => "DIGIT THREE".into(),
        '4' => "DIGIT FOUR".into(),
        '5' => "DIGIT FIVE".into(),
        '6' => "DIGIT SIX".into(),
        '7' => "DIGIT SEVEN".into(),
        '8' => "DIGIT EIGHT".into(),
        '9' => "DIGIT NINE".into(),
        ' ' => "SPACE".into(),
        '!' => "EXCLAMATION MARK".into(),
        '"' => "QUOTATION MARK".into(),
        '#' => "NUMBER SIGN".into(),
        '$' => "DOLLAR SIGN".into(),
        '%' => "PERCENT SIGN".into(),
        '&' => "AMPERSAND".into(),
        '\'' => "APOSTROPHE".into(),
        '(' => "LEFT PARENTHESIS".into(),
        ')' => "RIGHT PARENTHESIS".into(),
        '*' => "ASTERISK".into(),
        '+' => "PLUS SIGN".into(),
        ',' => "COMMA".into(),
        '-' => "HYPHEN-MINUS".into(),
        '.' => "FULL STOP".into(),
        '/' => "SOLIDUS".into(),
        ':' => "COLON".into(),
        ';' => "SEMICOLON".into(),
        '<' => "LESS-THAN SIGN".into(),
        '=' => "EQUALS SIGN".into(),
        '>' => "GREATER-THAN SIGN".into(),
        '?' => "QUESTION MARK".into(),
        '@' => "COMMERCIAL AT".into(),
        '[' => "LEFT SQUARE BRACKET".into(),
        '\\' => "REVERSE SOLIDUS".into(),
        ']' => "RIGHT SQUARE BRACKET".into(),
        '^' => "CIRCUMFLEX ACCENT".into(),
        '_' => "LOW LINE".into(),
        '`' => "GRAVE ACCENT".into(),
        '{' => "LEFT CURLY BRACKET".into(),
        '|' => "VERTICAL LINE".into(),
        '}' => "RIGHT CURLY BRACKET".into(),
        '~' => "TILDE".into(),
        _ => format!("CHARACTER U+{:04X}", c as u32),
    }
}
