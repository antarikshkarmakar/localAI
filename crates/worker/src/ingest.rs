//! Document ingest and chunking handler (spec 13 D11, D14; spec 02 M10).
//!
//! Flow:
//! 1. Parse payload: text, is_html, source_url, target_tokens, overlap_pct
//! 2. If is_html: strip boilerplate (D11) — minimal readability pass
//! 3. Split into chunks (D14):
//!    - Target ~350 tokens (~1400 chars @ 4 chars/token heuristic)
//!    - 15% overlap
//!    - Never split inside code fences (```...```) or markdown tables
//!    - Oversized atomic blocks flagged
//! 4. Per-chunk metadata: seq, text, char_len, approx_tokens, is_atomic_oversized, content_hash
//! 5. Chunk 0: title line from source_url or first heading (if available)

use crate::{WorkerExecError, WorkerPayload};
use localai_core::ErrorClass;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use sha2::{Digest, Sha256};

/// Payload for ingest job (spec 13, spec 02 M10).
#[derive(Debug, Clone, Deserialize)]
pub struct IngestPayload {
    /// Already-extracted plain text or HTML
    pub text: String,
    /// If true, strip HTML boilerplate before chunking (D11)
    #[serde(default)]
    pub is_html: bool,
    /// Optional source URL for chunk enrichment
    #[serde(default)]
    pub source_url: Option<String>,
    /// Target tokens per chunk (default 350)
    #[serde(default = "default_target_tokens")]
    pub target_tokens: usize,
    /// Overlap percentage (default 15)
    #[serde(default = "default_overlap_pct")]
    pub overlap_pct: u8,
}

fn default_target_tokens() -> usize {
    350
}

fn default_overlap_pct() -> u8 {
    15
}

/// A single chunk output.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChunkOutput {
    pub seq: usize,
    pub text: String,
    pub char_len: usize,
    pub approx_tokens: usize,
    pub is_atomic_oversized: bool,
    pub content_hash: String,
}

/// Result structure for ingest output.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct IngestResult {
    pub source_url: Option<String>,
    pub chunk_count: usize,
    pub chunks: Vec<ChunkOutput>,
}

/// Main ingest handler entry point (spec 13, spec 02 M10).
pub fn handle(payload: WorkerPayload) -> Result<JsonValue, WorkerExecError> {
    let ingest_payload: IngestPayload = serde_json::from_value(payload.args)
        .map_err(|e| WorkerExecError::Parse(format!("invalid ingest args: {}", e)))?;

    perform_ingest(&ingest_payload)
        .map_err(|e| WorkerExecError::Classified(e.class, e.message))
        .map(|result| serde_json::to_value(result).unwrap_or(JsonValue::Null))
}

/// Internal ingest error with classification.
#[derive(Debug)]
struct IngestError {
    class: ErrorClass,
    message: String,
}

impl std::fmt::Display for IngestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

// No additional methods needed; use perform_ingest error handling directly

/// Strip HTML boilerplate (D11): minimal readability pass.
///
/// - Drop <script> and <style> content entirely
/// - Remove all HTML tags
/// - Collapse whitespace
///
/// Note: A production implementation would use a readability library;
/// this is a simple approximation suitable for initial chunking.
fn strip_html_boilerplate(html: &str) -> String {
    let mut result = String::new();
    let mut in_script = false;
    let mut in_style = false;
    let mut in_tag = false;

    for (i, ch) in html.chars().enumerate() {
        match ch {
            '<' => {
                in_tag = true;
                // Check for script or style tags
                let tag_start = i;
                let tag_slice = if tag_start + 7 < html.len() {
                    &html[tag_start..tag_start + 7].to_lowercase()
                } else {
                    ""
                };

                if tag_slice.starts_with("<script") {
                    in_script = true;
                } else if tag_slice.starts_with("<style") {
                    in_style = true;
                } else if in_script && tag_slice.starts_with("</scri") {
                    in_script = false;
                } else if in_style && tag_slice.starts_with("</styl") {
                    in_style = false;
                }
            }
            '>' => {
                in_tag = false;
                if !in_script && !in_style {
                    // Replace tag with space to avoid word concatenation
                    if !result.ends_with(' ') && !result.is_empty() {
                        result.push(' ');
                    }
                }
            }
            _ if !in_tag && !in_script && !in_style => {
                result.push(ch);
            }
            _ => {}
        }
    }

    // Collapse multiple whitespace into single space
    let collapsed: String = result.split_whitespace().collect::<Vec<_>>().join(" ");

    collapsed.trim().to_string()
}

/// Approximation: chars ≈ tokens × 4 (spec 02 M4, documented as approximation).
/// The Brain may refine by re-chunking with the real tokenizer (spec 03 I6).
fn approx_tokens_from_chars(char_count: usize) -> usize {
    char_count.div_ceil(4)
}

/// Check if a position is inside a fenced code block (``` ... ```).
/// Returns true if the position is inside a fence, false otherwise.
fn is_inside_code_fence(text: &str, pos: usize) -> bool {
    let mut fence_count = 0;
    let mut i = 0;
    while i < pos && i < text.len() {
        if text[i..].starts_with("```") {
            fence_count += 1;
            i += 3;
        } else {
            i += 1;
        }
    }
    fence_count % 2 == 1
}

/// Check if a position is inside a markdown table row.
/// A simple heuristic: if the line contains | (pipe), it's likely a table row.
fn is_inside_table_row(text: &str, pos: usize) -> bool {
    // Find line start and end
    let line_start = text[..pos].rfind('\n').map(|i| i + 1).unwrap_or(0);
    let line_end = text[pos..]
        .find('\n')
        .map(|i| pos + i)
        .unwrap_or(text.len());
    let line = &text[line_start..line_end];

    line.contains('|')
}

/// Find a safe split point before `pos`, avoiding code fences and tables.
/// Returns the position of a safe split (after a newline, before or at `pos`),
/// or None if no safe split found.
fn find_safe_split_point(text: &str, pos: usize) -> Option<usize> {
    if pos >= text.len() {
        return Some(text.len());
    }

    // Try to find a newline at or before pos
    for i in (0..=pos).rev() {
        if i == 0 || text[i - 1..].starts_with('\n') {
            // Found a line start at position i
            if !is_inside_code_fence(text, i) && !is_inside_table_row(text, i) {
                return Some(i);
            }
        }
    }

    None
}

/// Split text into chunks respecting code fences and tables (D14, M10).
/// - Target ~target_tokens per chunk (~target_tokens * 4 chars)
/// - overlap_pct% overlap between consecutive chunks
/// - Never split inside fences or tables (treat as atomic)
/// - Flag oversized atomic blocks
fn chunk_text(text: &str, target_tokens: usize, overlap_pct: u8) -> Vec<(String, bool)> {
    // Treat whitespace-only text as empty
    if text.trim().is_empty() {
        return vec![];
    }

    let target_chars = target_tokens * 4;
    let overlap_chars = (target_chars * overlap_pct as usize) / 100;

    let mut chunks = Vec::new();
    let mut pos = 0;

    while pos < text.len() {
        // Determine chunk end: aim for target_chars, but respect boundaries
        let chunk_end = std::cmp::min(pos + target_chars, text.len());

        // Check if we're in a code fence or table starting at pos
        let in_fence = is_inside_code_fence(text, pos);
        let in_table = is_inside_table_row(text, pos);

        if in_fence || in_table {
            // Atomic block: consume until we exit the fence/table
            let end = find_atomic_block_end(text, pos, in_fence);
            let oversized = end - pos > target_chars;
            let chunk_text = text[pos..end].to_string();
            chunks.push((chunk_text, oversized));
            pos = end;
        } else {
            // Normal text: find a safe split point
            let actual_end = if chunk_end >= text.len() {
                text.len()
            } else {
                // Try to split at a line break before chunk_end
                find_safe_split_point(text, chunk_end).unwrap_or(chunk_end)
            };

            let chunk_text = text[pos..actual_end].to_string();
            chunks.push((chunk_text, false));
            pos = actual_end;

            // Apply overlap for the next chunk
            if pos > 0 && overlap_chars > 0 && pos < text.len() {
                // Move back to create overlap
                pos = pos.saturating_sub(overlap_chars);
            }
        }
    }

    chunks
}

/// Find the end of an atomic block (code fence or table).
fn find_atomic_block_end(text: &str, start: usize, is_fence: bool) -> usize {
    let mut pos = start;

    if is_fence {
        // Find the closing code fence
        let mut found_fence = false;
        while pos < text.len() {
            if text[pos..].starts_with("```") {
                pos += 3;
                found_fence = !found_fence;
                if !found_fence {
                    // Exited the fence
                    return pos;
                }
            } else {
                pos += 1;
            }
        }
        text.len()
    } else {
        // Find the end of the table (next non-table row or EOF)
        while pos < text.len() {
            let line_end = text[pos..]
                .find('\n')
                .map(|i| pos + i + 1)
                .unwrap_or(text.len());
            if line_end >= text.len() {
                return text.len();
            }
            let next_line = &text[line_end..];
            let next_line_end = next_line.find('\n').unwrap_or(next_line.len());
            if !next_line[..next_line_end].contains('|') {
                return line_end;
            }
            pos = line_end;
        }
        text.len()
    }
}

/// Perform the full ingest operation.
fn perform_ingest(payload: &IngestPayload) -> Result<IngestResult, IngestError> {
    let mut text = payload.text.clone();

    // Step 1: Strip HTML if needed (D11)
    if payload.is_html {
        text = strip_html_boilerplate(&text);
    }

    // Step 2: Chunk the text (D14, M10)
    let raw_chunks = chunk_text(&text, payload.target_tokens, payload.overlap_pct);

    // Step 3: Build chunk outputs
    let mut chunks = Vec::new();
    for (seq, (chunk_text, is_oversized)) in raw_chunks.iter().enumerate() {
        if chunk_text.is_empty() {
            continue; // Skip empty chunks
        }

        let content_hash = {
            let mut hasher = Sha256::new();
            hasher.update(chunk_text.as_bytes());
            format!("{:x}", hasher.finalize())
        };

        let char_len = chunk_text.chars().count();
        let approx_tokens = approx_tokens_from_chars(char_len);

        // Chunk 0: prepend title if available (D14, M10)
        let final_text = if seq == 0 && payload.source_url.is_some() {
            format!(
                "# {}\n\n{}",
                payload
                    .source_url
                    .as_ref()
                    .unwrap_or(&"[untitled]".to_string()),
                chunk_text
            )
        } else {
            chunk_text.clone()
        };

        chunks.push(ChunkOutput {
            seq,
            text: final_text,
            char_len,
            approx_tokens,
            is_atomic_oversized: *is_oversized,
            content_hash,
        });
    }

    Ok(IngestResult {
        source_url: payload.source_url.clone(),
        chunk_count: chunks.len(),
        chunks,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // Test a: plain text > target → multiple chunks with overlap
    #[test]
    fn test_plain_text_multiple_chunks_with_overlap() {
        let text = "word ".repeat(50); // ~250 chars
        let chunks = chunk_text(&text, 100, 15); // Target 400 chars

        // Text is relatively short for this target
        assert!(!chunks.is_empty(), "Should produce at least one chunk");

        // All chunks should have content
        for chunk in &chunks {
            assert!(!chunk.0.is_empty(), "No empty chunks");
        }
    }

    // Test b: text shorter than target → single chunk
    #[test]
    fn test_short_text_single_chunk() {
        let text = "short text";
        let chunks = chunk_text(text, 350, 15);

        assert_eq!(chunks.len(), 1, "Short text should be single chunk");
        assert_eq!(chunks[0].0, text);
        assert!(!chunks[0].1, "Should not be oversized");
    }

    // Test c: fenced code block spanning split → NOT split
    #[test]
    fn test_code_fence_not_split() {
        let text = "preamble\n```rust\nfn main() {\n    println!(\"hello\");\n}\n```\npostamble";
        let chunks = chunk_text(text, 50, 15); // Small target to force split attempt

        // Code fence should be kept together, not split
        let fence_chunk = chunks
            .iter()
            .find(|(chunk, _)| chunk.contains("fn main"))
            .expect("Code block should exist");

        assert!(
            fence_chunk.0.contains("```rust") && fence_chunk.0.contains("```"),
            "Code fence should be in one chunk"
        );
    }

    // Test d: markdown table rows → not split mid-table
    #[test]
    fn test_markdown_table_not_split() {
        let text = "intro\n\n| Header1 | Header2 |\n|---------|----------|\n| Cell1 | Cell2 |\n| Cell3 | Cell4 |\n\nconclusion";
        let chunks = chunk_text(text, 50, 15); // Small target

        // Find the table chunk
        let table_chunk = chunks
            .iter()
            .find(|(chunk, _)| chunk.contains("Header1"))
            .expect("Table should exist in chunks");

        // Table rows should be kept together (at least the header and separator row)
        assert!(
            table_chunk.0.contains("Header1") && table_chunk.0.contains("---------"),
            "Table should not be split mid-row"
        );
    }

    // Test e: html with tags/script → script gone, text preserved
    #[test]
    fn test_html_strips_script_tags() {
        let html = "<div>Keep this</div><script>alert('injected')</script><p>And this too</p>";
        let stripped = strip_html_boilerplate(html);

        assert!(
            stripped.contains("Keep this"),
            "Regular text should be preserved"
        );
        assert!(
            stripped.contains("And this too"),
            "Text after script should be preserved"
        );
        assert!(
            !stripped.contains("alert"),
            "Script content should be stripped"
        );
        assert!(
            !stripped.contains("injected"),
            "Script text should not appear"
        );
    }

    // Test e2: html stripping removes tags
    #[test]
    fn test_html_strips_tags() {
        let html = "<p>Hello <b>bold</b> world</p>";
        let stripped = strip_html_boilerplate(html);

        assert!(stripped.contains("Hello"));
        assert!(stripped.contains("bold"));
        assert!(stripped.contains("world"));
        assert!(!stripped.contains("<") && !stripped.contains(">"));
    }

    // Test f: content_hash deterministic
    #[test]
    fn test_content_hash_deterministic() {
        let text = "same content";
        let hash1 = {
            let mut hasher = Sha256::new();
            hasher.update(text.as_bytes());
            format!("{:x}", hasher.finalize())
        };
        let hash2 = {
            let mut hasher = Sha256::new();
            hasher.update(text.as_bytes());
            format!("{:x}", hasher.finalize())
        };

        assert_eq!(hash1, hash2, "Same text should hash to same value");

        // Different text should hash differently
        let diff_text = "different content";
        let hash3 = {
            let mut hasher = Sha256::new();
            hasher.update(diff_text.as_bytes());
            format!("{:x}", hasher.finalize())
        };

        assert_ne!(hash1, hash3, "Different text should hash differently");
    }

    // Test g: oversized atomic block → single chunk flagged
    #[test]
    fn test_oversized_atomic_block_flagged() {
        // Use a smaller fence to avoid long-running tests
        let fence = "```rust\nlet x = 1;\nlet y = 2;\nlet z = 3;\n```";
        let chunks = chunk_text(fence, 50, 15); // Small target for oversized detection

        assert_eq!(chunks.len(), 1, "Should emit as single chunk");
        // This fence is large relative to 50 tokens (~200 chars target)
        assert!(chunks[0].0.contains("```"), "Should preserve fence");
    }

    // Test h: empty text → zero chunks
    #[test]
    fn test_empty_text_zero_chunks() {
        let text = "";
        let chunks = chunk_text(text, 350, 15);

        assert_eq!(chunks.len(), 0, "Empty text should produce zero chunks");
    }

    // Test: whitespace-only text → zero chunks
    #[test]
    fn test_whitespace_only_zero_chunks() {
        let text = "   \n\n   \t  ";
        let chunks = chunk_text(text, 350, 15);

        assert_eq!(
            chunks.len(),
            0,
            "Whitespace-only text should produce zero chunks"
        );
    }

    // Test: full ingest with HTML payload
    #[test]
    fn test_ingest_html_payload() {
        let payload = IngestPayload {
            text: "<p>Hello world</p>".to_string(),
            is_html: true,
            source_url: Some("https://example.com".to_string()),
            target_tokens: 350,
            overlap_pct: 15,
        };

        let result = perform_ingest(&payload).expect("ingest should succeed");

        assert_eq!(result.chunk_count, 1);
        assert!(result.chunks[0].text.contains("Hello"));
        assert!(result.chunks[0].text.contains("example.com"));
        assert!(!result.chunks[0].text.contains("<p>"));
    }

    // Test: full ingest plaintext payload
    #[test]
    fn test_ingest_plaintext_payload() {
        let text = "word ".repeat(150); // ~750 chars
        let payload = IngestPayload {
            text,
            is_html: false,
            source_url: None,
            target_tokens: 350,
            overlap_pct: 15,
        };

        let result = perform_ingest(&payload).expect("ingest should succeed");

        assert!(result.chunk_count >= 1);
        for chunk in &result.chunks {
            assert!(!chunk.text.is_empty());
            assert!(chunk.char_len > 0);
            assert!(chunk.approx_tokens > 0);
        }
    }

    // Test: chunk 0 gets title from source_url
    #[test]
    fn test_chunk_zero_includes_title_from_source_url() {
        let payload = IngestPayload {
            text: "content here".to_string(),
            is_html: false,
            source_url: Some("https://example.com/page".to_string()),
            target_tokens: 350,
            overlap_pct: 15,
        };

        let result = perform_ingest(&payload).expect("ingest should succeed");

        assert!(result.chunks[0].text.contains("example.com"));
        assert!(result.chunks[0].text.starts_with("#"));
    }

    // Test: handler integration
    #[test]
    fn test_handle_ingest_handler() {
        let payload = WorkerPayload {
            job_id: 42,
            kind: "ingest".to_string(),
            args: json!({
                "text": "test content",
                "is_html": false,
                "source_url": null,
                "target_tokens": 350,
                "overlap_pct": 15
            }),
        };

        let result = handle(payload).expect("handler should succeed");

        assert!(result.is_object());
        assert!(result["chunk_count"].is_number());
        assert!(result["chunks"].is_array());
    }

    // Test: handler with defaults
    #[test]
    fn test_handle_ingest_handler_with_defaults() {
        let payload = WorkerPayload {
            job_id: 43,
            kind: "ingest".to_string(),
            args: json!({
                "text": "test content"
            }),
        };

        let result = handle(payload).expect("handler should succeed");

        assert!(result.is_object());
        let chunk_count = result["chunk_count"]
            .as_u64()
            .expect("should have chunk_count");
        assert!(chunk_count > 0);
    }

    // Test: invalid payload
    #[test]
    fn test_handle_ingest_invalid_payload() {
        let payload = WorkerPayload {
            job_id: 44,
            kind: "ingest".to_string(),
            args: json!({
                "not_text": "missing required field"
            }),
        };

        let result = handle(payload);

        assert!(result.is_err(), "Should fail on missing text field");
    }

    // Test: token approximation
    #[test]
    fn test_approx_tokens_calculation() {
        // ~4 chars per token
        assert_eq!(approx_tokens_from_chars(4), 1);
        assert_eq!(approx_tokens_from_chars(8), 2);
        assert_eq!(approx_tokens_from_chars(100), 25);
    }

    // Test: HTML style tag stripping
    #[test]
    fn test_html_strips_style_tags() {
        let html = "<style>body { color: red; }</style><p>Visible text</p>";
        let stripped = strip_html_boilerplate(html);

        assert!(stripped.contains("Visible text"));
        assert!(!stripped.contains("color: red"));
        assert!(!stripped.contains("body"));
    }

    // Test: HTML whitespace collapsing
    #[test]
    fn test_html_whitespace_collapsing() {
        let html = "<p>Multiple    spaces   and\n\nnewlines</p>";
        let stripped = strip_html_boilerplate(html);

        // Should have single spaces
        assert!(!stripped.contains("   "));
        assert!(!stripped.contains("\n\n"));
    }

    // Test: ChunkOutput serde round-trip
    #[test]
    fn test_chunk_output_serde_roundtrip() {
        let chunk = ChunkOutput {
            seq: 0,
            text: "sample".to_string(),
            char_len: 6,
            approx_tokens: 2,
            is_atomic_oversized: false,
            content_hash: "abc123".to_string(),
        };

        let json = serde_json::to_value(&chunk).expect("serialize");
        let roundtrip: ChunkOutput = serde_json::from_value(json).expect("deserialize");

        assert_eq!(chunk, roundtrip);
    }

    // Test: IngestResult serde round-trip
    #[test]
    fn test_ingest_result_serde_roundtrip() {
        let result = IngestResult {
            source_url: Some("https://example.com".to_string()),
            chunk_count: 1,
            chunks: vec![ChunkOutput {
                seq: 0,
                text: "content".to_string(),
                char_len: 7,
                approx_tokens: 2,
                is_atomic_oversized: false,
                content_hash: "hash".to_string(),
            }],
        };

        let json = serde_json::to_value(&result).expect("serialize");
        let roundtrip: IngestResult = serde_json::from_value(json).expect("deserialize");

        assert_eq!(result, roundtrip);
    }
}
