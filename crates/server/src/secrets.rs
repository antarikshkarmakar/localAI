//! Local secret store for the config UI (spec 12 U5, CON-9/CON-13 softened).
//!
//! The operator's cloud API keys are entered in the loopback-only config page
//! and written here — `~/.localai/secrets.env`, permissions 0600, OUTSIDE the
//! repo. This is a deliberate, single-user softening of "secrets never
//! persisted" (CON-13): the file is owner-only, gitignored by location, keys
//! are masked on display, and the SecretFilter still scrubs every egress/log
//! path (CON-13's egress half is untouched). Keys load into process env at
//! boot, so downstream code still reads them via env only (CON-9 spirit).
//!
//! Hardening:
//! - **Name allowlist** — the web form can only set KNOWN key names, never
//!   arbitrary env vars (a form that could set any env var is an injection
//!   hole). Unknown names are refused.
//! - **0600 on every write** — re-applied each time, so a permissive umask
//!   can't widen it.
//! - **Masked reads** — the API never returns a raw key, only `sk-…last4`.

use std::io::Write;
use std::path::{Path, PathBuf};
use thiserror::Error;

/// Env-var names the config UI is allowed to set. Anything else is refused —
/// the UI must never be able to write an arbitrary environment variable.
pub const ALLOWED_KEYS: &[&str] = &[
    "ANTHROPIC_API_KEY",
    "OPENAI_API_KEY",
    "GEMINI_API_KEY",
    "OPENCODE_API_KEY",
];

#[derive(Debug, Error)]
pub enum SecretError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error("'{0}' is not a settable key (allowlist: {ALLOWED_KEYS:?})")]
    NotAllowed(String),

    #[error("value contains a newline or NUL — refused")]
    BadValue,
}

/// A masked view of one stored key for the config page. Never carries the raw value.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct MaskedKey {
    pub name: String,
    /// `true` if a value is present.
    pub set: bool,
    /// e.g. `"sk-…a1b2"`, or empty when unset.
    pub masked: String,
}

pub struct SecretStore {
    path: PathBuf,
}

impl SecretStore {
    /// Store at `<dir>/secrets.env`. The Brain passes `~/.localai`.
    pub fn new(dir: PathBuf) -> Self {
        Self {
            path: dir.join("secrets.env"),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Set one allowlisted key. Upserts the line, rewrites the whole file at 0600.
    pub fn set_key(&self, name: &str, value: &str) -> Result<(), SecretError> {
        if !ALLOWED_KEYS.contains(&name) {
            return Err(SecretError::NotAllowed(name.to_string()));
        }
        if value.contains('\n') || value.contains('\0') {
            return Err(SecretError::BadValue);
        }
        let mut kv = self.read_pairs()?;
        // upsert
        if let Some(slot) = kv.iter_mut().find(|(k, _)| k == name) {
            slot.1 = value.to_string();
        } else {
            kv.push((name.to_string(), value.to_string()));
        }
        self.write_pairs(&kv)
    }

    /// Masked view of every allowlisted key (set or not) — for the config page.
    pub fn list_masked(&self) -> Result<Vec<MaskedKey>, SecretError> {
        let kv = self.read_pairs()?;
        Ok(ALLOWED_KEYS
            .iter()
            .map(|name| {
                let val = kv.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str());
                MaskedKey {
                    name: (*name).to_string(),
                    set: val.is_some_and(|v| !v.is_empty()),
                    masked: val.map(mask).unwrap_or_default(),
                }
            })
            .collect())
    }

    /// Load stored keys into the process environment (boot step, CON-9).
    /// Only sets a var if it isn't already present — an explicit env override
    /// (e.g. from the shell) wins over the file.
    pub fn load_into_env(&self) -> Result<usize, SecretError> {
        let kv = self.read_pairs()?;
        let mut n = 0;
        for (k, v) in kv {
            if ALLOWED_KEYS.contains(&k.as_str()) && !v.is_empty() && std::env::var(&k).is_err() {
                // SAFETY: single-threaded boot step, before any worker spawns.
                std::env::set_var(&k, &v);
                n += 1;
            }
        }
        Ok(n)
    }

    fn read_pairs(&self) -> Result<Vec<(String, String)>, SecretError> {
        if !self.path.exists() {
            return Ok(Vec::new());
        }
        let content = std::fs::read_to_string(&self.path)?;
        Ok(content
            .lines()
            .filter_map(|l| {
                let l = l.trim();
                if l.is_empty() || l.starts_with('#') {
                    return None;
                }
                l.split_once('=')
                    .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
            })
            .collect())
    }

    fn write_pairs(&self, kv: &[(String, String)]) -> Result<(), SecretError> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let body: String = kv.iter().map(|(k, v)| format!("{k}={v}\n")).collect();

        // Write to a temp sibling then rename — never leave a half-written
        // secrets file, and apply 0600 before the value lands at the path.
        let tmp = self.path.with_extension("env.tmp");
        {
            let mut f = std::fs::File::create(&tmp)?;
            set_owner_only(&f)?;
            f.write_all(body.as_bytes())?;
            f.sync_data()?;
        }
        set_owner_only_path(&tmp)?;
        std::fs::rename(&tmp, &self.path)?;
        set_owner_only_path(&self.path)?;
        Ok(())
    }
}

/// `sk-proj-abcd…wxyz` → `sk-…wxyz`. Short values fully masked.
fn mask(v: &str) -> String {
    if v.len() <= 4 {
        return "****".to_string();
    }
    let head: String = v.chars().take(3).collect();
    let tail: String = v
        .chars()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("{head}…{tail}")
}

#[cfg(unix)]
fn set_owner_only(f: &std::fs::File) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    f.set_permissions(std::fs::Permissions::from_mode(0o600))
}
#[cfg(unix)]
fn set_owner_only_path(p: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o600))
}
#[cfg(not(unix))]
fn set_owner_only(_f: &std::fs::File) -> std::io::Result<()> {
    Ok(()) // Windows ACLs not handled here; deploy target is WSL2/Linux (CON-3).
}
#[cfg(not(unix))]
fn set_owner_only_path(_p: &Path) -> std::io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, SecretStore) {
        let dir = tempfile::tempdir().unwrap();
        let s = SecretStore::new(dir.path().to_path_buf());
        (dir, s)
    }

    #[test]
    fn set_then_list_is_masked_never_raw() {
        let (_d, s) = store();
        s.set_key("OPENAI_API_KEY", "sk-proj-abcdef1234wxyz")
            .unwrap();
        let masked = s.list_masked().unwrap();
        let openai = masked.iter().find(|k| k.name == "OPENAI_API_KEY").unwrap();
        assert!(openai.set);
        assert_eq!(openai.masked, "sk-…wxyz");
        // The raw value never appears in the masked view.
        assert!(!openai.masked.contains("abcdef"));
    }

    #[test]
    fn all_allowlisted_keys_listed_even_when_unset() {
        let (_d, s) = store();
        let masked = s.list_masked().unwrap();
        assert_eq!(masked.len(), ALLOWED_KEYS.len());
        assert!(masked.iter().all(|k| !k.set));
    }

    #[test]
    fn non_allowlisted_key_refused() {
        let (_d, s) = store();
        let err = s.set_key("EVIL_ARBITRARY_ENV", "x").unwrap_err();
        assert!(matches!(err, SecretError::NotAllowed(_)));
    }

    #[test]
    fn newline_injection_refused() {
        let (_d, s) = store();
        let err = s.set_key("OPENAI_API_KEY", "sk-x\nPATH=/evil").unwrap_err();
        assert!(matches!(err, SecretError::BadValue));
    }

    #[test]
    fn upsert_overwrites_not_duplicates() {
        let (_d, s) = store();
        s.set_key("GEMINI_API_KEY", "AIzaFIRST0000").unwrap();
        s.set_key("GEMINI_API_KEY", "AIzaSECOND111").unwrap();
        let raw = std::fs::read_to_string(s.path()).unwrap();
        assert_eq!(raw.matches("GEMINI_API_KEY").count(), 1);
        assert!(raw.contains("AIzaSECOND111"));
    }

    #[cfg(unix)]
    #[test]
    fn file_is_owner_only_0600() {
        use std::os::unix::fs::PermissionsExt;
        let (_d, s) = store();
        s.set_key("ANTHROPIC_API_KEY", "sk-ant-xxxxxxxx").unwrap();
        let mode = std::fs::metadata(s.path()).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "secrets file must be 0600");
    }

    #[test]
    fn load_into_env_respects_existing_override() {
        let (_d, s) = store();
        s.set_key("OPENCODE_API_KEY", "sk-file-value").unwrap();
        std::env::set_var("OPENCODE_API_KEY", "sk-shell-wins");
        s.load_into_env().unwrap();
        assert_eq!(std::env::var("OPENCODE_API_KEY").unwrap(), "sk-shell-wins");
        std::env::remove_var("OPENCODE_API_KEY");
    }
}
