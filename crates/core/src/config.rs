//! Configuration loader (spec 01 §6, docs/config.md registry).
//!
//! - `config.toml` at repo root; env overrides prefixed `LOCALAI_`
//!   (e.g. `LOCALAI_MEM_CEILING_GB=20` → `mem.ceiling_gb`).
//! - Secrets are environment-ONLY (CON-9): the config FILE is rejected at
//!   load if any string value matches a key-like pattern — defense against
//!   an accidentally committed key.
//! - `config_hash()` is deterministic and logged in every `SessionStart`
//!   ledger event (the learning loop must know its config context, spec 10).
//!
//! Pure module: parses strings, never touches the filesystem (spec 01 T1 —
//! `core` has no I/O). The caller reads the file and captures the env.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

pub const ENV_PREFIX: &str = "LOCALAI_";

/// Key-like patterns rejected in config-file values (CON-9).
/// Kept intentionally broad — a false positive costs a rename; a false
/// negative costs a leaked credential.
const SECRET_PATTERNS: &[&str] = &[
    "sk-",            // OpenAI / Anthropic style
    "AIza",           // Google API key
    "ghp_", "gho_",   // GitHub tokens
    "xoxb-", "xoxp-", // Slack tokens
    "AKIA",           // AWS access key id
    "-----BEGIN",     // PEM private key blocks
];

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("config parse error: {0}")]
    Parse(#[from] toml::de::Error),

    #[error("config value at '{key}' looks like a secret ({pattern}…) — secrets are environment-only (CON-9), never in config.toml")]
    SecretLikeValue { key: String, pattern: String },

    #[error("invalid env override '{0}' — unknown field or wrong value type (check docs/config.md)")]
    UnknownOverride(String),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct Config {
    pub mem: MemCfg,
    pub paths: PathsCfg,
    pub inference: InferenceCfg,
    pub queue: QueueCfg,
    pub ledger: LedgerCfg,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            mem: MemCfg::default(),
            paths: PathsCfg::default(),
            inference: InferenceCfg::default(),
            queue: QueueCfg::default(),
            ledger: LedgerCfg::default(),
        }
    }
}

/// MemoryGuard watermarks (spec 01 §4, CON-1).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct MemCfg {
    pub soft_gb: f64,
    pub hard_gb: f64,
    pub ceiling_gb: f64,
}

impl Default for MemCfg {
    fn default() -> Self {
        Self { soft_gb: 19.0, hard_gb: 21.0, ceiling_gb: 22.0 }
    }
}

/// Data locations. MUST be Linux-filesystem paths, never /mnt/c (CON-4);
/// enforced at startup by the Brain (spec 01 T3), not here (core = no I/O).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct PathsCfg {
    pub db_path: String,
    pub kb_dir: String,
    pub artifacts_dir: String,
    pub spill_path: String,
}

impl Default for PathsCfg {
    fn default() -> Self {
        Self {
            db_path: "data/localai.db".into(),
            kb_dir: "kb".into(),
            artifacts_dir: "artifacts".into(),
            spill_path: "data/ledger.spill.jsonl".into(),
        }
    }
}

/// llama-server client (spec 03).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct InferenceCfg {
    pub port: u16,
    pub ctx: u32,
    pub health_timeout_s: u64,
}

impl Default for InferenceCfg {
    fn default() -> Self {
        Self { port: 8080, ctx: 32_768, health_timeout_s: 120 }
    }
}

/// Job queue / supervisor (spec 04).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct QueueCfg {
    pub permits: u32,
    pub lease_secs: u64,
    pub max_attempts: u32,
}

impl Default for QueueCfg {
    fn default() -> Self {
        Self { permits: 3, lease_secs: 600, max_attempts: 3 }
    }
}

/// Ledger writer (spec 01 R9, spec 04 O14).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct LedgerCfg {
    pub channel_capacity: usize,
    pub batch_max: usize,
    pub flush_interval_ms: u64,
    pub send_timeout_ms: u64,
}

impl Default for LedgerCfg {
    fn default() -> Self {
        Self { channel_capacity: 1024, batch_max: 50, flush_interval_ms: 100, send_timeout_ms: 50 }
    }
}

const SECTIONS: &[&str] = &["mem", "paths", "inference", "queue", "ledger"];

impl Config {
    /// Load from a TOML string plus env-var pairs. Pure — no I/O.
    pub fn load(
        toml_str: &str,
        env: impl IntoIterator<Item = (String, String)>,
    ) -> Result<Config, ConfigError> {
        // Root of a TOML document is always a table — parse as one directly.
        let mut root: toml::Table = toml_str.parse()?;

        // CON-9: reject key-like strings anywhere in the FILE (env is the
        // sanctioned secret channel; the file is not).
        scan_for_secrets(&toml::Value::Table(root.clone()), "")?;

        // Validate the file alone first — file typos surface as Parse
        // errors via deny_unknown_fields, attributed to the file.
        let _: Config = toml::Value::Table(root.clone()).try_into()?;

        // Apply LOCALAI_* env overrides one at a time, re-validating after
        // each so a bad override is attributed to its exact env var (a
        // deserialize error after an insert can only be that insert's fault).
        for (key, val) in env {
            let Some(rest) = key.strip_prefix(ENV_PREFIX) else { continue };
            let (section, field) = match_section(rest).ok_or_else(|| {
                ConfigError::UnknownOverride(key.clone())
            })?;
            let entry = root
                .entry(section)
                .or_insert_with(|| toml::Value::Table(Default::default()));
            let Some(t) = entry.as_table_mut() else {
                return Err(ConfigError::UnknownOverride(key));
            };
            t.insert(field, parse_scalar(&val));

            let check: Result<Config, _> = toml::Value::Table(root.clone()).try_into();
            if check.is_err() {
                return Err(ConfigError::UnknownOverride(key));
            }
        }

        let config: Config = toml::Value::Table(root).try_into()?;
        Ok(config)
    }

    /// Deterministic hash of the effective config (spec 01 §6 — logged in
    /// SessionStart so learning always knows its config context).
    pub fn config_hash(&self) -> String {
        // Invariant: Config is a closed set of plain scalars/strings —
        // serialization cannot fail. Allowed exception to the no-expect rule
        // (docs/standards.md): a Result here would force every SessionStart
        // caller to invent handling for an impossible error.
        #[allow(clippy::expect_used)]
        let canonical = toml::to_string(self).expect("Config is always serializable");
        let digest = Sha256::digest(canonical.as_bytes());
        format!("{digest:x}")
    }
}

/// `MEM_CEILING_GB` → `("mem", "ceiling_gb")`.
fn match_section(rest: &str) -> Option<(String, String)> {
    let lower = rest.to_ascii_lowercase();
    for section in SECTIONS {
        if let Some(field) = lower.strip_prefix(&format!("{section}_")) {
            if !field.is_empty() {
                return Some((section.to_string(), field.to_string()));
            }
        }
    }
    None
}

/// Env values arrive as strings; coerce to the most specific TOML scalar.
fn parse_scalar(v: &str) -> toml::Value {
    if let Ok(i) = v.parse::<i64>() {
        return toml::Value::Integer(i);
    }
    if let Ok(f) = v.parse::<f64>() {
        return toml::Value::Float(f);
    }
    if let Ok(b) = v.parse::<bool>() {
        return toml::Value::Boolean(b);
    }
    toml::Value::String(v.to_string())
}

fn scan_for_secrets(value: &toml::Value, path: &str) -> Result<(), ConfigError> {
    match value {
        toml::Value::String(s) => {
            for pattern in SECRET_PATTERNS {
                if s.contains(pattern) {
                    return Err(ConfigError::SecretLikeValue {
                        key: path.to_string(),
                        pattern: (*pattern).to_string(),
                    });
                }
            }
            Ok(())
        }
        toml::Value::Table(t) => {
            for (k, v) in t {
                let child = if path.is_empty() { k.clone() } else { format!("{path}.{k}") };
                scan_for_secrets(v, &child)?;
            }
            Ok(())
        }
        toml::Value::Array(a) => {
            for (i, v) in a.iter().enumerate() {
                scan_for_secrets(v, &format!("{path}[{i}]"))?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_env() -> Vec<(String, String)> {
        Vec::new()
    }

    // Empty file → all defaults.
    #[test]
    fn empty_toml_yields_defaults() {
        let cfg = Config::load("", no_env()).unwrap();
        assert_eq!(cfg, Config::default());
        assert_eq!(cfg.mem.ceiling_gb, 22.0); // CON-1
        assert_eq!(cfg.queue.permits, 3);     // CON-5
        assert_eq!(cfg.ledger.batch_max, 50); // R9
    }

    // File value overrides default.
    #[test]
    fn file_value_overrides_default() {
        let cfg = Config::load("[queue]\nlease_secs = 300\n", no_env()).unwrap();
        assert_eq!(cfg.queue.lease_secs, 300);
        assert_eq!(cfg.queue.permits, 3); // untouched default
    }

    // Spec 01 §6 example: LOCALAI_MEM_CEILING_GB env override wins over file.
    #[test]
    fn env_override_beats_file() {
        let env = vec![("LOCALAI_MEM_CEILING_GB".to_string(), "20".to_string())];
        let cfg = Config::load("[mem]\nceiling_gb = 22.0\n", env).unwrap();
        assert_eq!(cfg.mem.ceiling_gb, 20.0);
    }

    // CON-9: config file carrying a key-like value is rejected at load.
    #[test]
    fn secret_like_value_rejected() {
        let toml = "[paths]\ndb_path = \"sk-abc123fakekey\"\n";
        let err = Config::load(toml, no_env()).unwrap_err();
        match err {
            ConfigError::SecretLikeValue { key, pattern } => {
                assert_eq!(key, "paths.db_path");
                assert_eq!(pattern, "sk-");
            }
            other => panic!("expected SecretLikeValue, got {other:?}"),
        }
    }

    // Typos in the file fail loudly, not silently.
    #[test]
    fn unknown_file_key_rejected() {
        let err = Config::load("[queue]\npermitz = 5\n", no_env());
        assert!(err.is_err(), "typo'd key must not pass silently");
    }

    // Typos in env overrides fail loudly too.
    #[test]
    fn unknown_env_override_rejected() {
        let env = vec![("LOCALAI_QUEUE_PERMITZ".to_string(), "5".to_string())];
        let err = Config::load("", env).unwrap_err();
        assert!(matches!(err, ConfigError::UnknownOverride(_)));
    }

    // Non-LOCALAI env vars are ignored (the process env is full of noise).
    #[test]
    fn unrelated_env_ignored() {
        let env = vec![("PATH".to_string(), "/usr/bin".to_string())];
        let cfg = Config::load("", env).unwrap();
        assert_eq!(cfg, Config::default());
    }

    // Hash: deterministic; sensitive to any value change (spec 01 §6).
    #[test]
    fn config_hash_deterministic_and_value_sensitive() {
        let a = Config::load("", no_env()).unwrap();
        let b = Config::load("", no_env()).unwrap();
        assert_eq!(a.config_hash(), b.config_hash());

        let c = Config::load("[queue]\npermits = 2\n", no_env()).unwrap();
        assert_ne!(a.config_hash(), c.config_hash());
    }
}
