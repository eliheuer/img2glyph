# img2glyph — LLM workflow context

This document provides context for LLM-assisted labeling with img2glyph.
It is written to be used with any LLM CLI tool: Claude Code, Codex, OpenCode,
Hermes, or anything else that can read files and run shell commands.

The idea is simple: img2glyph handles the image processing, and the LLM handles
the part that requires vision — identifying which character each extracted glyph is.

---

## The two-step workflow

### Step 1 — Segment

Run img2glyph to find and extract all glyph candidates from a scanned image:

```bash
img2glyph process <IMAGE_PATH> --output ./glyphs
```

This writes:
- `./glyphs/glyph_0001.png` … `./glyphs/glyph_NNNN.png`
- `./glyphs/manifest.json` with bounding boxes and reading-order metadata

Tuning flags:

| Flag | Default | When to change |
|---|---|---|
| `--min-area 200` | 200 px² | Raise to suppress noise blobs |
| `--max-area 50000` | 50 000 px² | Lower to exclude headers, rules, borders |
| `--block-radius 15` | 15 | Increase for poorly-lit or uneven scans |
| `--padding 10` | 10 px | Increase for more whitespace around each glyph |

### Step 2 — Label

Look at the extracted glyph images, identify each character, write an
assignments file, and apply it:

```bash
img2glyph label ./glyphs/manifest.json --assignments assignments.json
```

This renames the files to standard glyph names (`A.png`, `ampersand.png`,
`uni00B6.png`, …) and updates the manifest in place.

---

## Assignments file format

Two formats are supported.

**Sequence string** — fastest when glyphs are in a predictable reading order:

```json
{ "sequence": "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789" }
```

Characters are assigned to glyphs by reading-order index. Glyphs beyond the
end of the string remain unlabeled.

**Per-glyph mapping** — for precise control:

```json
{
  "glyph_0001": { "unicode": "U+0041", "name": "LATIN CAPITAL LETTER A" },
  "glyph_0002": { "unicode": "U+0042", "name": "LATIN CAPITAL LETTER B" },
  "glyph_0034": { "unicode": "U+0026", "name": "AMPERSAND" }
}
```

---

## Output filename format

| State | Example filename |
|---|---|
| Unlabeled | `glyph_0001.png` |
| Labeled, standard character | `A.png`, `ampersand.png` |
| Labeled, extended character | `uni00B6.png` |

These are standard Adobe Glyph List names, which is what font editors
(Glyphs.app, RoboFont, FontLab, UFO-based tools) use for direct image import.

---

## manifest.json format

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

The manifest is the durable record. Re-label or re-export at any time without
re-segmenting.

---

## Common issues

- **Too many small blobs**: raise `--min-area`
- **Dotted glyphs split** (`i`, `j`, `!`): the dot is a separate component —
  raise `--min-area` to discard it, or skip those IDs in assignments
- **Touching characters merged**: lower `--max-area` or re-scan
- **Horizontal rules extracted**: lower `--max-area`
- **Image is light text on dark background**: invert before processing

---

## Suggested LLM-assisted session

```bash
# 1. Segment the image
img2glyph process specimen.png --output ./glyphs

# 2. Review — look at a sample of the glyph images to check reading order
#    and segmentation quality before labeling

# 3. Write assignments.json based on what you see in the glyph images

# 4. Apply labels
img2glyph label ./glyphs/manifest.json --assignments assignments.json

# 5. Verify
ls ./glyphs/*.png
```
