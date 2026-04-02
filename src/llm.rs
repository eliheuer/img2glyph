use anyhow::{Context, Result};
use base64::{engine::general_purpose, Engine as _};
use reqwest::Client;
use serde_json::{json, Value};
use std::path::Path;

use crate::manifest::GlyphEntry;

/// Label every entry in `glyphs` using the Claude vision API.
pub async fn label_all(
    glyphs: &mut Vec<GlyphEntry>,
    output_dir: &Path,
    api_key: &str,
) -> Result<()> {
    let indices: Vec<usize> = (0..glyphs.len()).collect();
    label_at(glyphs, output_dir, api_key, &indices).await
}

/// Label only the entries at the given indices (useful for re-labeling unlabeled glyphs).
pub async fn label_at(
    glyphs: &mut Vec<GlyphEntry>,
    output_dir: &Path,
    api_key: &str,
    indices: &[usize],
) -> Result<()> {
    let client = Client::new();
    let total = indices.len();

    for (progress, &idx) in indices.iter().enumerate() {
        let id = glyphs[idx].id.clone();
        let img_path = output_dir.join(&glyphs[idx].file);

        let png_bytes = std::fs::read(&img_path)
            .with_context(|| format!("Cannot read {}", img_path.display()))?;

        eprint!("  [{}/{}] {}... ", progress + 1, total, id);

        match identify_glyph(&client, api_key, &png_bytes).await {
            Ok((unicode, name, confidence)) => {
                eprintln!("{} ({})", unicode, confidence);
                glyphs[idx].unicode = Some(unicode);
                glyphs[idx].name = Some(name);
                glyphs[idx].confidence = Some(confidence);
            }
            Err(e) => {
                eprintln!("failed: {}", e);
            }
        }
    }

    Ok(())
}

/// Send one glyph PNG to Claude and return (unicode, name, confidence).
async fn identify_glyph(
    client: &Client,
    api_key: &str,
    png_bytes: &[u8],
) -> Result<(String, String, f32)> {
    let b64 = general_purpose::STANDARD.encode(png_bytes);

    let body = json!({
        "model": "claude-opus-4-6",
        "max_tokens": 128,
        "messages": [{
            "role": "user",
            "content": [
                {
                    "type": "image",
                    "source": {
                        "type": "base64",
                        "media_type": "image/png",
                        "data": b64
                    }
                },
                {
                    "type": "text",
                    "text": "This is a single typographic glyph cropped from a scanned font specimen (grayscale PNG). Identify the Unicode codepoint for this character.\n\nReply with ONLY a JSON object and nothing else:\n{\"unicode\": \"U+XXXX\", \"name\": \"UNICODE CHARACTER NAME\", \"confidence\": 0.95}"
                }
            ]
        }]
    });

    let response = client
        .post("https://api.anthropic.com/v1/messages")
        .header("x-api-key", api_key)
        .header("anthropic-version", "2023-06-01")
        .header("content-type", "application/json")
        .json(&body)
        .send()
        .await
        .context("API request failed")?;

    let resp_json: Value = response
        .json()
        .await
        .context("Failed to parse API response as JSON")?;

    let text = resp_json["content"][0]["text"]
        .as_str()
        .context("No text content in API response")?;

    // Strip any markdown code fences the model might add.
    let cleaned = text
        .trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();

    let parsed: Value = serde_json::from_str(cleaned)
        .with_context(|| format!("Could not parse model response as JSON: {}", text))?;

    let unicode = parsed["unicode"]
        .as_str()
        .unwrap_or("U+FFFD")
        .to_uppercase();

    let name = parsed["name"]
        .as_str()
        .unwrap_or("UNKNOWN")
        .to_string();

    let confidence = parsed["confidence"].as_f64().unwrap_or(0.5) as f32;

    Ok((unicode, name, confidence))
}
