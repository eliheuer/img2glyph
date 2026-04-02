mod llm;
mod manifest;
mod segment;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use manifest::{BboxRecord, GlyphEntry, Manifest};
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------

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
        Commands::Process { image, output, llm, padding, min_area, max_area, block_radius } =>
            cmd_process(image, output, llm, padding, min_area, max_area, block_radius).await,
        Commands::Label { manifest, assignments, llm } =>
            cmd_label(manifest, assignments, llm).await,
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

    let mut entries: Vec<GlyphEntry> = Vec::with_capacity(bboxes.len());

    for (idx, bbox) in bboxes.iter().enumerate() {
        let id = format!("glyph_{:04}", idx + 1);
        let filename = format!("{}.png", id);
        let out_path = output_dir.join(&filename);

        let cropped = segment::extract_glyph(&gray, bbox, padding);
        cropped.save(&out_path)
            .with_context(|| format!("Cannot save {}", out_path.display()))?;

        entries.push(GlyphEntry {
            id,
            file: filename,
            bbox: BboxRecord { x: bbox.x, y: bbox.y, w: bbox.w, h: bbox.h },
            area_px: bbox.area,
            row: bbox.row,
            col: bbox.col,
            unicode: None,
            glyph_name: None,
            unicode_name: None,
            confidence: None,
        });
    }

    eprintln!("Extracted {} glyphs → {}", entries.len(), output_dir.display());

    if use_llm {
        let api_key = std::env::var("ANTHROPIC_API_KEY")
            .context("ANTHROPIC_API_KEY is not set")?;
        eprintln!("Labeling with Claude API…");
        llm::label_all(&mut entries, &output_dir, &api_key).await?;
        populate_glyph_names(&mut entries);
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

    let output_dir = manifest_path.parent().unwrap_or(Path::new(".")).to_path_buf();

    if let Some(path) = assignments_path {
        apply_assignments(&mut manifest.glyphs, &path, &output_dir)?;
    }

    if use_llm {
        let api_key = std::env::var("ANTHROPIC_API_KEY")
            .context("ANTHROPIC_API_KEY is not set")?;

        let unlabeled: Vec<usize> = manifest.glyphs.iter().enumerate()
            .filter(|(_, g)| g.unicode.is_none())
            .map(|(i, _)| i)
            .collect();

        if unlabeled.is_empty() {
            eprintln!("All glyphs are already labeled; nothing to do.");
        } else {
            eprintln!("Labeling {} unlabeled glyph(s) with Claude API…", unlabeled.len());
            llm::label_at(&mut manifest.glyphs, &output_dir, &api_key, &unlabeled).await?;
            populate_glyph_names(&mut manifest.glyphs);
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

fn apply_assignments(
    glyphs: &mut Vec<GlyphEntry>,
    assignments_path: &Path,
    output_dir: &Path,
) -> Result<()> {
    let json = std::fs::read_to_string(assignments_path)
        .with_context(|| format!("Cannot read {}", assignments_path.display()))?;
    let value: serde_json::Value = serde_json::from_str(&json)?;

    if let Some(seq) = value.get("sequence").and_then(|v| v.as_str()) {
        for (glyph, ch) in glyphs.iter_mut().zip(seq.chars()) {
            let (unicode, unicode_name) = char_unicode_info(ch);
            glyph.unicode = Some(unicode);
            glyph.unicode_name = Some(unicode_name);
        }
    } else if let Some(obj) = value.as_object() {
        for glyph in glyphs.iter_mut() {
            if let Some(entry) = obj.get(&glyph.id) {
                glyph.unicode = entry.get("unicode").and_then(|v| v.as_str()).map(str::to_string);
                glyph.unicode_name = entry.get("name").and_then(|v| v.as_str()).map(str::to_string);
            }
        }
    }

    populate_glyph_names(glyphs);
    rename_labeled(glyphs, output_dir)
}

/// Compute and store the AGL glyph name for every labeled entry.
fn populate_glyph_names(glyphs: &mut Vec<GlyphEntry>) {
    for glyph in glyphs.iter_mut() {
        if let Some(unicode) = &glyph.unicode {
            glyph.glyph_name = Some(agl_name(unicode));
        }
    }
}

/// Rename each labeled glyph file to its AGL glyph name (`A.png`, `ampersand.png`, …).
fn rename_labeled(glyphs: &mut Vec<GlyphEntry>, output_dir: &Path) -> Result<()> {
    for glyph in glyphs.iter_mut() {
        let Some(unicode) = &glyph.unicode else { continue };
        let new_file = format!("{}.png", agl_name(unicode));
        let old_path = output_dir.join(&glyph.file);
        let new_path = output_dir.join(&new_file);

        if old_path != new_path {
            if old_path.exists() {
                std::fs::rename(&old_path, &new_path).with_context(|| {
                    format!("Cannot rename {} → {}", old_path.display(), new_path.display())
                })?;
            }
            glyph.file = new_file;
        }
    }
    Ok(())
}

fn write_manifest(output_dir: &Path, source: &Path, glyphs: Vec<GlyphEntry>) -> Result<()> {
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

fn char_unicode_info(c: char) -> (String, String) {
    (format!("U+{:04X}", c as u32), unicode_char_name(c))
}

fn unicode_char_name(c: char) -> String {
    match c {
        'A'..='Z' => format!("LATIN CAPITAL LETTER {}", c),
        'a'..='z' => format!("LATIN SMALL LETTER {}", c.to_uppercase().next().unwrap()),
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

// ---------------------------------------------------------------------------
// Adobe Glyph List name lookup
//
// The glyph name is the identifier font editors (Glyphs.app, RoboFont,
// FontLab, UFO) use to map an image into a Unicode slot. The stem of the
// output PNG must be exactly this name for direct import to work.
//
// Covers Basic Latin, Latin-1 Supplement, and common typographic extras.
// Anything not listed falls back to the uniXXXX / uXXXXX format, which
// every major font editor also recognises.
//
// Reference: https://github.com/adobe-type-tools/agl-aglfn
// ---------------------------------------------------------------------------

fn agl_name(unicode: &str) -> String {
    let cp_str = unicode.trim_start_matches("U+").trim_start_matches("u+");
    let cp = match u32::from_str_radix(cp_str, 16) {
        Ok(v) => v,
        Err(_) => return format!("uni{}", cp_str.to_uppercase()),
    };

    match cp {
        0x0020 => "space".into(),
        0x0021 => "exclam".into(),
        0x0022 => "quotedbl".into(),
        0x0023 => "numbersign".into(),
        0x0024 => "dollar".into(),
        0x0025 => "percent".into(),
        0x0026 => "ampersand".into(),
        0x0027 => "quotesingle".into(),
        0x0028 => "parenleft".into(),
        0x0029 => "parenright".into(),
        0x002A => "asterisk".into(),
        0x002B => "plus".into(),
        0x002C => "comma".into(),
        0x002D => "hyphen".into(),
        0x002E => "period".into(),
        0x002F => "slash".into(),
        0x0030 => "zero".into(),
        0x0031 => "one".into(),
        0x0032 => "two".into(),
        0x0033 => "three".into(),
        0x0034 => "four".into(),
        0x0035 => "five".into(),
        0x0036 => "six".into(),
        0x0037 => "seven".into(),
        0x0038 => "eight".into(),
        0x0039 => "nine".into(),
        0x003A => "colon".into(),
        0x003B => "semicolon".into(),
        0x003C => "less".into(),
        0x003D => "equal".into(),
        0x003E => "greater".into(),
        0x003F => "question".into(),
        0x0040 => "at".into(),
        0x0041..=0x005A => char::from_u32(cp).unwrap().to_string(), // A–Z
        0x005B => "bracketleft".into(),
        0x005C => "backslash".into(),
        0x005D => "bracketright".into(),
        0x005E => "asciicircum".into(),
        0x005F => "underscore".into(),
        0x0060 => "grave".into(),
        0x0061..=0x007A => char::from_u32(cp).unwrap().to_string(), // a–z
        0x007B => "braceleft".into(),
        0x007C => "bar".into(),
        0x007D => "braceright".into(),
        0x007E => "asciitilde".into(),
        0x00A1 => "exclamdown".into(),
        0x00A2 => "cent".into(),
        0x00A3 => "sterling".into(),
        0x00A4 => "currency".into(),
        0x00A5 => "yen".into(),
        0x00A6 => "brokenbar".into(),
        0x00A7 => "section".into(),
        0x00A8 => "dieresis".into(),
        0x00A9 => "copyright".into(),
        0x00AA => "ordfeminine".into(),
        0x00AB => "guillemotleft".into(),
        0x00AC => "logicalnot".into(),
        0x00AE => "registered".into(),
        0x00AF => "macron".into(),
        0x00B0 => "degree".into(),
        0x00B1 => "plusminus".into(),
        0x00B2 => "twosuperior".into(),
        0x00B3 => "threesuperior".into(),
        0x00B4 => "acute".into(),
        0x00B5 => "mu".into(),
        0x00B6 => "paragraph".into(),
        0x00B7 => "periodcentered".into(),
        0x00B8 => "cedilla".into(),
        0x00B9 => "onesuperior".into(),
        0x00BA => "ordmasculine".into(),
        0x00BB => "guillemotright".into(),
        0x00BC => "onequarter".into(),
        0x00BD => "onehalf".into(),
        0x00BE => "threequarters".into(),
        0x00BF => "questiondown".into(),
        0x00C0 => "Agrave".into(),
        0x00C1 => "Aacute".into(),
        0x00C2 => "Acircumflex".into(),
        0x00C3 => "Atilde".into(),
        0x00C4 => "Adieresis".into(),
        0x00C5 => "Aring".into(),
        0x00C6 => "AE".into(),
        0x00C7 => "Ccedilla".into(),
        0x00C8 => "Egrave".into(),
        0x00C9 => "Eacute".into(),
        0x00CA => "Ecircumflex".into(),
        0x00CB => "Edieresis".into(),
        0x00CC => "Igrave".into(),
        0x00CD => "Iacute".into(),
        0x00CE => "Icircumflex".into(),
        0x00CF => "Idieresis".into(),
        0x00D0 => "Eth".into(),
        0x00D1 => "Ntilde".into(),
        0x00D2 => "Ograve".into(),
        0x00D3 => "Oacute".into(),
        0x00D4 => "Ocircumflex".into(),
        0x00D5 => "Otilde".into(),
        0x00D6 => "Odieresis".into(),
        0x00D7 => "multiply".into(),
        0x00D8 => "Oslash".into(),
        0x00D9 => "Ugrave".into(),
        0x00DA => "Uacute".into(),
        0x00DB => "Ucircumflex".into(),
        0x00DC => "Udieresis".into(),
        0x00DD => "Yacute".into(),
        0x00DE => "Thorn".into(),
        0x00DF => "germandbls".into(),
        0x00E0 => "agrave".into(),
        0x00E1 => "aacute".into(),
        0x00E2 => "acircumflex".into(),
        0x00E3 => "atilde".into(),
        0x00E4 => "adieresis".into(),
        0x00E5 => "aring".into(),
        0x00E6 => "ae".into(),
        0x00E7 => "ccedilla".into(),
        0x00E8 => "egrave".into(),
        0x00E9 => "eacute".into(),
        0x00EA => "ecircumflex".into(),
        0x00EB => "edieresis".into(),
        0x00EC => "igrave".into(),
        0x00ED => "iacute".into(),
        0x00EE => "icircumflex".into(),
        0x00EF => "idieresis".into(),
        0x00F0 => "eth".into(),
        0x00F1 => "ntilde".into(),
        0x00F2 => "ograve".into(),
        0x00F3 => "oacute".into(),
        0x00F4 => "ocircumflex".into(),
        0x00F5 => "otilde".into(),
        0x00F6 => "odieresis".into(),
        0x00F7 => "divide".into(),
        0x00F8 => "oslash".into(),
        0x00F9 => "ugrave".into(),
        0x00FA => "uacute".into(),
        0x00FB => "ucircumflex".into(),
        0x00FC => "udieresis".into(),
        0x00FD => "yacute".into(),
        0x00FE => "thorn".into(),
        0x00FF => "ydieresis".into(),
        0x0131 => "dotlessi".into(),
        0x0141 => "Lslash".into(),
        0x0142 => "lslash".into(),
        0x0152 => "OE".into(),
        0x0153 => "oe".into(),
        0x0160 => "Scaron".into(),
        0x0161 => "scaron".into(),
        0x0178 => "Ydieresis".into(),
        0x017D => "Zcaron".into(),
        0x017E => "zcaron".into(),
        0x0192 => "florin".into(),
        0x02C6 => "circumflex".into(),
        0x02DC => "tilde".into(),
        0x2013 => "endash".into(),
        0x2014 => "emdash".into(),
        0x2018 => "quoteleft".into(),
        0x2019 => "quoteright".into(),
        0x201A => "quotesinglbase".into(),
        0x201C => "quotedblleft".into(),
        0x201D => "quotedblright".into(),
        0x201E => "quotedblbase".into(),
        0x2020 => "dagger".into(),
        0x2021 => "daggerdbl".into(),
        0x2022 => "bullet".into(),
        0x2026 => "ellipsis".into(),
        0x2030 => "perthousand".into(),
        0x2039 => "guilsinglleft".into(),
        0x203A => "guilsinglright".into(),
        0x20AC => "Euro".into(),
        0x2122 => "trademark".into(),
        0xFB01 => "fi".into(),
        0xFB02 => "fl".into(),
        _ if cp <= 0xFFFF => format!("uni{:04X}", cp),
        _ => format!("u{:05X}", cp),
    }
}
