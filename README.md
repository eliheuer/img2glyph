# img2glyph

A Rust CLI tool and library for turning scanned images of printed type into individual named glyph PNGs — designed for font production pipelines and AI agent workflows.

This project draws on two earlier tools. [Raph Levien's font scanning scripts](https://levien.com/garden/font/) (2007, C/Python) established the core idea: binarize a scanned page, find connected ink components, and crop each one into its own image file. We follow the same basic pipeline but replace the fixed luminance threshold with adaptive thresholding, which handles uneven lighting and paper tone without manual tuning. [GlyphCollector](https://github.com/krksgbr/glyphcollector) takes a different approach — rather than automatic segmentation, it asks you to manually crop one example of each character and then finds every instance across a set of source pages using normalized cross-correlation. That makes it powerful for multi-page revival work where you want every occurrence of a glyph averaged together. img2glyph is simpler and more automated: one image in, one directory of labeled PNGs out, with no manual template step and no GUI required.

<img width="1512" height="982" alt="Image" src="https://github.com/user-attachments/assets/e1a08bae-c897-479e-b40d-beb3de3eb4a7" />

---

## How it works

Given a scanned type specimen (or any image of printed characters), img2glyph:

1. Binarizes the image with adaptive thresholding (handles uneven lighting)
2. Finds each glyph via connected-component labeling
3. Crops and pads every glyph into its own PNG
4. Sorts results into reading order and writes a `manifest.json`
5. Optionally labels each glyph with its Unicode codepoint

---

## Install

```bash
cargo install --path .
```

Requires Rust (stable). No external system libraries needed.

---

## Development and testing


Clone the repo and run directly with `cargo run`:

```bash
git clone https://github.com/eliheuer/img2glyph
cd img2glyph
cargo run -- process test.png --output tmp/test-output --min-area 2000
```

The `--min-area 2000` flag is appropriate for this specimen because the type is large and bold. For smaller or lighter scans you'd lower it.

Check the output:

```bash
ls tmp/test-output/
# glyph_0001.png  glyph_0002.png  …  manifest.json
```

Apply labels from a sequence string to verify the full labeling pipeline:

```bash
echo '{"sequence":"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789"}' \
  > tmp/assignments.json
cargo run -- label tmp/test-output/manifest.json --assignments tmp/assignments.json
ls tmp/test-output/*.png
# A.png  B.png  C.png  …  z.png  zero.png  one.png  …
```

The `tmp/` directory is gitignored, so test output won't show up in commits.

The repo includes `test.png` — a bold type specimen with uppercase, lowercase, digits, and common punctuation — this file is provided as a specimen to verify the pipeline while working on the code.

![test.png](test.png)

---

## Library usage

img2glyph can also be used as a library in other Rust projects. Add it to `Cargo.toml`:

```toml
# Full library + CLI
img2glyph = { path = "../img2glyph" }

# Library only — no CLI deps (clap)
img2glyph = { path = "../img2glyph", default-features = false }
```

Core API:

```rust
use img2glyph::{SegmentConfig, segment_image, extract_glyph, populate_glyph_names};

let image = image::open("specimen.png")?;
let config = SegmentConfig::default();

let bboxes = img2glyph::segment_image(&image, &config);
for bbox in &bboxes {
    let glyph_png = img2glyph::extract_glyph(&image, bbox, config.padding);
    // glyph_png is a GrayImage — save it, pass it on, or feed it to img2bez
}
```

### Features

| Feature | Default | Description |
|---|---|---|
| `cli` | ✓ | Builds the `img2glyph` binary (requires `clap`). |

---

## Workflow

### Step 1 — Segment

```bash
img2glyph process specimen.png --output ./glyphs
```

This creates one PNG per glyph (`glyph_0001.png` … `glyph_NNNN.png`) and a `manifest.json` in the output directory. Nothing is labeled yet — the files are numbered in reading order (top to bottom, left to right).

### Step 2 — Review

Spot-check a few output images and the manifest to confirm the segmentation looks right. The main things to look for:

- **Too many small blobs?** Raise `--min-area` to filter scan noise.
- **Characters touching each other merged?** Lower `--max-area` or re-scan.
- **Dotted glyphs split** (`i`, `j`, `!`, `;`)? The dot becomes a separate component — see [Known limitations](#known-limitations).
- **Horizontal rules or headers extracted?** Lower `--max-area`.

### Step 3 — Label

Once segmentation looks good, apply Unicode labels. There are three ways to do this.

#### Option A — Sequence string (fastest)

If your specimen has characters in a predictable order, provide them as a string:

```json
{ "sequence": "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789" }
```

```bash
img2glyph label ./glyphs/manifest.json --assignments assignments.json
```

Characters are assigned in reading order. Glyphs with no matching character remain numbered.

#### Option B — Per-glyph mapping

For explicit control, map glyph IDs to Unicode codepoints:

```json
{
  "glyph_0001": { "unicode": "U+0041", "name": "LATIN CAPITAL LETTER A" },
  "glyph_0002": { "unicode": "U+0042", "name": "LATIN CAPITAL LETTER B" },
  "glyph_0034": { "unicode": "U+0026", "name": "AMPERSAND" }
}
```

```bash
img2glyph label ./glyphs/manifest.json --assignments assignments.json
```

After labeling, files are renamed to standard glyph names: `glyph_0001.png` → `A.png`, `ampersand.png`, `germandbls.png`, `uni00B6.png` etc.

---

## LLM workflow

img2glyph is designed to work with any LLM CLI tool — Claude Code, Codex, OpenCode, Hermes, or anything else that can read files and run shell commands. The tool handles the image processing; the LLM handles the part that requires vision.

The workflow context lives in [`context/workflow.md`](context/workflow.md). Point your LLM tool at that file and it has everything it needs: command reference, assignments format, manifest format, and common troubleshooting.

A typical session:

1. Run `img2glyph process` to extract numbered glyph images
2. Ask your LLM to look at the images and write `assignments.json`
3. Run `img2glyph label` to apply the names

This works well for ambiguous or historical type where heuristics fall short — the LLM can make contextual decisions about ligatures, archaic letterforms, and non-Latin scripts.

---

## Options

### Segmentation

| Flag | Default | Description |
|---|---|---|
| `--min-area` | `200` | Minimum glyph area in pixels. Raise to suppress noise. |
| `--max-area` | `50000` | Maximum glyph area in pixels. Lower to exclude large non-glyph elements. |
| `--block-radius` | `15` | Adaptive threshold neighborhood radius. Increase for uneven lighting. |
| `--padding` | `10` | Pixels of whitespace added around each cropped glyph. |
| `--output` | `glyphs` | Output directory. |

---

## Output

Labeled glyph files use standard glyph names: `A.png`, `ampersand.png`, `germandbls.png`, `uni00B6.png`. These are the names Glyphs.app, RoboFont, FontLab, and UFO-based tools use to map an image into a Unicode slot — you can drag the output folder directly into your font editor.

Unlabeled glyphs keep their sequential names: `glyph_0001.png`.

The `manifest.json` is the durable record of the session. It stores bounding boxes, pixel areas, reading-order row/col indices, and the full Unicode metadata for every glyph. You can re-label or re-export at any time without re-segmenting.

```json
{
  "source": "specimen.png",
  "version": "0.1.0",
  "glyphs": [
    {
      "id": "glyph_0001",
      "file": "A.png",
      "bbox": { "x": 120, "y": 45, "w": 86, "h": 112 },
      "area_px": 6420,
      "row": 0,
      "col": 0,
      "unicode": "U+0041",
      "glyph_name": "A",
      "unicode_name": "LATIN CAPITAL LETTER A",
      "confidence": null
    }
  ]
}
```

---

## Known limitations

- **Dark on light only.** The pipeline assumes dark ink on a light background. Inverted images (white on black) need to be flipped before processing.
- **Touching characters.** Glyphs that share any pixels are extracted as a single component. This usually means a re-scan or manual crop is needed.
- **Dotted glyphs split.** The dot on `i`, `j`, `!`, `;`, `:` becomes its own connected component. To handle this: raise `--min-area` to discard the dots entirely, or skip those IDs in your assignments file.

