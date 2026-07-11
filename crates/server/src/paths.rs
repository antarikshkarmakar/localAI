//! Data-path guard (spec 01 R-startup step 1, CON-4, G-24).
//!
//! SQLite `.db` + `kb/` must live on the Linux filesystem, never a `/mnt/*`
//! 9P mount (CON-4 — the 9P layer destroys SQLite lock performance). The guard
//! checks the *resolved absolute* path, not the config string: a relative
//! default like `data/localai.db` launched from a `/mnt/c` checkout would
//! otherwise land on the forbidden mount undetected (G-24).

use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum PathGuardError {
    #[error("data path {0} resolves under /mnt (a 9P mount) — SQLite + kb must live on the Linux filesystem (CON-4). Use a path like ~/.localai or /home/<you>/localai.")]
    OnMountedFs(PathBuf),
}

/// Resolve `path` to an absolute path against `cwd` (no filesystem access, so
/// it works before the file exists), normalizing `.`/`..` lexically.
pub fn to_absolute(path: &Path, cwd: &Path) -> PathBuf {
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    };
    normalize_lexical(&joined)
}

/// Lexically collapse `.` and `..` without touching the filesystem.
fn normalize_lexical(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Reject a resolved path that lives under `/mnt` (CON-4, G-24).
/// `abs` must already be absolute + normalized (via [`to_absolute`]).
pub fn guard_not_on_mount(abs: &Path) -> Result<(), PathGuardError> {
    // WSL2 mounts Windows drives under /mnt/<letter>. Match /mnt and /mnt/*.
    let mut comps = abs.components();
    // Skip the root component.
    let _ = comps.next();
    if let Some(first) = comps.next() {
        if first.as_os_str() == "mnt" {
            return Err(PathGuardError::OnMountedFs(abs.to_path_buf()));
        }
    }
    Ok(())
}

/// Resolve then guard in one call.
pub fn resolve_guarded(path: &Path, cwd: &Path) -> Result<PathBuf, PathGuardError> {
    let abs = to_absolute(path, cwd);
    guard_not_on_mount(&abs)?;
    Ok(abs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_path_under_mnt_cwd_is_rejected() {
        // The G-24 case: relative default + a /mnt working directory.
        let cwd = Path::new("/mnt/c/GitHub/localAI");
        let err = resolve_guarded(Path::new("data/localai.db"), cwd).unwrap_err();
        assert!(matches!(err, PathGuardError::OnMountedFs(_)));
    }

    #[test]
    fn absolute_mnt_path_is_rejected() {
        let cwd = Path::new("/home/user");
        let err = resolve_guarded(Path::new("/mnt/c/data.db"), cwd).unwrap_err();
        assert!(matches!(err, PathGuardError::OnMountedFs(_)));
    }

    #[test]
    fn linux_fs_path_is_allowed() {
        let cwd = Path::new("/home/user/localai");
        let ok = resolve_guarded(Path::new("data/localai.db"), cwd).unwrap();
        assert_eq!(ok, PathBuf::from("/home/user/localai/data/localai.db"));
    }

    #[test]
    fn absolute_home_path_is_allowed() {
        let cwd = Path::new("/anywhere");
        let ok = resolve_guarded(Path::new("/home/user/.localai/db"), cwd).unwrap();
        assert_eq!(ok, PathBuf::from("/home/user/.localai/db"));
    }

    // `..` traversal out of /mnt is honored lexically.
    #[test]
    fn dotdot_escaping_mnt_is_allowed() {
        let cwd = Path::new("/mnt/c/proj");
        // ../../home/user/db → /home/user/db
        let ok = resolve_guarded(Path::new("../../../home/user/db"), cwd).unwrap();
        assert_eq!(ok, PathBuf::from("/home/user/db"));
    }

    // A path named "mnt" deeper in the tree is fine — only the first segment matters.
    #[test]
    fn mnt_as_nonroot_segment_is_allowed() {
        let cwd = Path::new("/home/user");
        let ok = resolve_guarded(Path::new("data/mnt/x.db"), cwd).unwrap();
        assert_eq!(ok, PathBuf::from("/home/user/data/mnt/x.db"));
    }
}
