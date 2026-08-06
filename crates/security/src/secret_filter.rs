//! SecretFilter — redaction at the persist and egress chokepoints (CON-13).
//!
//! Spec 11 S4-S6 / spec 05 C1 / GAPS G-14: the *model* can emit a secret it
//! read from env, a file, or a scraped page. Keeping secrets out of config
//! (CON-9) does nothing about that. So every string that is about to be
//! **persisted** (ledger payload, OKF body, handoff, artifact) or **sent to a
//! cloud provider** (council prompt) passes through here first.
//!
//! Design rules:
//! - **Fail loud, not silent.** Redaction returns a report (what matched,
//!   where) so the caller can raise an incident + write a `secret_audit` row.
//!   A silent scrub hides that a leak was attempted.
//! - **Never log the secret.** The report carries the *pattern name* and an
//!   offset, never the matched text. A filter that logs what it caught is a
//!   secret-logger.
//! - **Pattern-based, deliberately broad.** False positives cost a redacted
//!   token; false negatives cost a leaked credential. Bias to over-redact.
//! - **No regex dependency.** Hand-rolled scanners keep this crate dep-free
//!   and auditable — this is the code that must never surprise anyone.

use std::fmt;

/// One known secret shape. `prefix` is matched literally; a match extends
/// while the following chars look like a credential body.
struct Pattern {
    name: &'static str,
    prefix: &'static str,
    /// Minimum body length after the prefix to count as a credential
    /// (avoids redacting the bare word "sk-" in prose).
    min_body: usize,
}

const PATTERNS: &[Pattern] = &[
    Pattern {
        name: "openai/anthropic-key",
        prefix: "sk-",
        min_body: 16,
    },
    Pattern {
        name: "google-api-key",
        prefix: "AIza",
        min_body: 20,
    },
    Pattern {
        name: "github-token",
        prefix: "ghp_",
        min_body: 20,
    },
    Pattern {
        name: "github-oauth",
        prefix: "gho_",
        min_body: 20,
    },
    Pattern {
        name: "slack-bot-token",
        prefix: "xoxb-",
        min_body: 10,
    },
    Pattern {
        name: "slack-user-token",
        prefix: "xoxp-",
        min_body: 10,
    },
    Pattern {
        name: "aws-access-key-id",
        prefix: "AKIA",
        min_body: 12,
    },
    Pattern {
        name: "pem-private-key",
        prefix: "-----BEGIN",
        min_body: 8,
    },
];

/// Replacement written in place of a matched secret.
pub const REDACTED: &str = "[REDACTED]";

/// What the filter caught. Carries NO secret material — pattern name and
/// position only, safe to log and to store in `secret_audit`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    pub pattern: &'static str,
    /// Byte offset in the ORIGINAL input where the match started.
    pub offset: usize,
}

/// Result of filtering one string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Filtered {
    /// The text with every match replaced by [`REDACTED`].
    pub text: String,
    /// Every match, in order. Empty = clean.
    pub hits: Vec<Hit>,
}

impl Filtered {
    /// True when something was redacted — caller MUST raise an incident and
    /// write a `secret_audit` row (spec 11 S4).
    pub fn is_dirty(&self) -> bool {
        !self.hits.is_empty()
    }
}

impl fmt::Display for Hit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Deliberately prints no matched text.
        write!(f, "{} at byte {}", self.pattern, self.offset)
    }
}

/// Scan `input`, replacing every credential-shaped run with [`REDACTED`].
///
/// This is the ONLY function callers need: run it on anything crossing the
/// persist or egress boundary. Cheap enough to apply unconditionally.
pub fn filter(input: &str) -> Filtered {
    let bytes = input.as_bytes();
    let mut out = String::with_capacity(input.len());
    let mut hits = Vec::new();
    let mut i = 0usize;

    while i < bytes.len() {
        let mut matched = false;

        for p in PATTERNS {
            if !input.is_char_boundary(i) || !input[i..].starts_with(p.prefix) {
                continue;
            }
            let body_start = i + p.prefix.len();
            let rest = &input[body_start.min(input.len())..];
            // PEM headers contain spaces; every other credential shape must
            // stop at whitespace (see credential_body_len).
            let body_len = if p.name == "pem-private-key" {
                pem_body_len(rest)
            } else {
                credential_body_len(rest)
            };
            if body_len < p.min_body {
                continue;
            }
            out.push_str(REDACTED);
            hits.push(Hit {
                pattern: p.name,
                offset: i,
            });
            i = body_start + body_len;
            matched = true;
            break;
        }

        if !matched {
            // Advance one full char (never split a UTF-8 sequence).
            let ch = input[i..].chars().next().unwrap_or('\u{FFFD}');
            out.push(ch);
            i += ch.len_utf8();
        }
    }

    Filtered { text: out, hits }
}

/// How many bytes from the start of `s` look like credential body —
/// base64/hex/token alphabet. **Whitespace terminates the match**: without
/// that, one key's match runs on through the following words, merging two
/// secrets into one hit and false-positiving on ordinary prose ("the sk-
/// prefix denotes…"). PEM blocks, whose header legitimately contains spaces,
/// are handled by [`pem_body_len`] instead.
fn credential_body_len(s: &str) -> usize {
    s.bytes()
        .take_while(|b| {
            b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'/' | b'+' | b'=')
        })
        .count()
}

/// PEM header/footer runs to end of line — `-----BEGIN RSA PRIVATE KEY-----`
/// contains spaces, so it needs its own rule.
fn pem_body_len(s: &str) -> usize {
    s.find('\n').unwrap_or(s.len())
}

/// Convenience: filter and return only the safe text, discarding the report.
/// Prefer [`filter`] — the report is how an attempted leak gets noticed.
pub fn redact(input: &str) -> String {
    filter(input).text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_text_passes_through_unchanged() {
        let f = filter("the sky is blue and nothing here is secret");
        assert!(!f.is_dirty());
        assert_eq!(f.text, "the sky is blue and nothing here is secret");
    }

    #[test]
    fn openai_style_key_is_redacted() {
        let f = filter("use sk-proj-ABCDEFGH12345678IJKL to call the api");
        assert!(f.is_dirty());
        assert!(f.text.contains(REDACTED));
        assert!(!f.text.contains("ABCDEFGH"), "raw key must not survive");
        assert_eq!(f.hits[0].pattern, "openai/anthropic-key");
    }

    #[test]
    fn google_github_slack_aws_pem_all_caught() {
        for (input, want) in [
            ("AIzaSyD1234567890abcdefghij", "google-api-key"),
            ("ghp_1234567890abcdefghijKLMNOP", "github-token"),
            ("xoxb-123456789012-abcdef", "slack-bot-token"),
            ("AKIAIOSFODNN7EXAMPLE", "aws-access-key-id"),
            ("-----BEGIN RSA PRIVATE KEY-----", "pem-private-key"),
        ] {
            let f = filter(input);
            assert!(f.is_dirty(), "missed a secret: {input}");
            assert_eq!(f.hits[0].pattern, want);
        }
    }

    // The report must never carry secret material (spec 11 S4).
    #[test]
    fn hit_report_contains_no_secret_material() {
        let f = filter("sk-proj-SUPERSECRETVALUE123");
        let rendered = format!("{}", f.hits[0]);
        assert!(!rendered.contains("SUPERSECRET"));
        assert!(rendered.contains("openai/anthropic-key"));
    }

    // "sk-" in ordinary prose must not be redacted (min_body guard).
    #[test]
    fn short_prefix_in_prose_is_not_redacted() {
        let f = filter("the sk- prefix denotes a secret key");
        assert!(!f.is_dirty(), "false positive on bare prefix");
    }

    #[test]
    fn multiple_secrets_all_redacted() {
        let f = filter("first sk-AAAAAAAAAAAAAAAAAA then ghp_BBBBBBBBBBBBBBBBBBBB done");
        assert_eq!(f.hits.len(), 2);
        assert!(!f.text.contains("AAAAAAAA"));
        assert!(!f.text.contains("BBBBBBBB"));
    }

    // A secret buried in a scraped page / model output (G-14: the model can
    // emit what it read) is caught the same way.
    #[test]
    fn secret_embedded_in_prose_is_caught() {
        let f =
            filter("The config showed OPENAI_API_KEY=sk-live-9876543210ABCDEFGH in plain text.");
        assert!(f.is_dirty());
        assert!(!f.text.contains("9876543210"));
        assert!(
            f.text.contains("The config showed"),
            "surrounding prose preserved"
        );
    }

    #[test]
    fn unicode_input_does_not_panic_or_corrupt() {
        let f = filter("héllo → wörld 日本語 sk-ABCDEFGHIJKLMNOPQR ✓");
        assert!(f.is_dirty());
        assert!(f.text.contains("日本語"));
        assert!(!f.text.contains("ABCDEFGHIJKLMNOPQR"));
    }

    #[test]
    fn redact_helper_returns_text_only() {
        assert_eq!(redact("plain"), "plain");
        assert!(redact("sk-ABCDEFGHIJKLMNOPQRST").contains(REDACTED));
    }
}
