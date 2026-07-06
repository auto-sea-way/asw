//! Runtime resolution of the workspace source directory uploaded by
//! `asw cloud build`.
//!
//! The pipeline used to embed a `CARGO_MANIFEST_DIR`-derived compile-time
//! constant as the only source of truth, which only makes sense for a
//! binary built and run from the same local checkout. A binary built on a
//! CI runner and distributed as a release asset resolves to a path that
//! doesn't exist on the end user's machine, with no way to override it.

use std::path::{Path, PathBuf};

/// Resolve the workspace source directory to upload for `asw cloud build`.
///
/// Precedence:
/// 1. An explicit `--src` flag, if given — always wins.
/// 2. The current working directory, if it looks like the workspace root
///    (contains a `Cargo.toml` with a `[workspace]` table).
/// 3. `compile_time_fallback` (the `CARGO_MANIFEST_DIR`-derived path baked in
///    at build time) — dev convenience only, and the only option that made
///    sense before this fix. Meaningless for a distributed release binary,
///    which is why it is the last resort rather than the only behavior.
pub fn resolve_src_dir(
    explicit: Option<PathBuf>,
    cwd: &Path,
    compile_time_fallback: &Path,
) -> PathBuf {
    if let Some(p) = explicit {
        return p;
    }
    if is_workspace_root(cwd) {
        return cwd.to_path_buf();
    }
    compile_time_fallback.to_path_buf()
}

/// True if `dir` contains a workspace-root `Cargo.toml` (one with a
/// `[workspace]` table), as opposed to a single-crate member manifest.
fn is_workspace_root(dir: &Path) -> bool {
    match std::fs::read_to_string(dir.join("Cargo.toml")) {
        Ok(contents) => contents.lines().any(|l| l.trim() == "[workspace]"),
        Err(_) => false,
    }
}

/// Warn (non-fatal) if `dir`'s git working tree has uncommitted changes.
/// `step_upload_src` uploads `git archive HEAD`, so uncommitted changes are
/// silently excluded from the remote build unless the user is told.
pub fn warn_if_dirty(dir: &Path) {
    let output = std::process::Command::new("git")
        .args(["-C", &dir.to_string_lossy(), "status", "--porcelain"])
        .output();

    if let Ok(out) = output {
        if out.status.success() && !out.stdout.is_empty() {
            tracing::warn!(
                "Working tree at {:?} has uncommitted changes. `asw cloud build` uploads \
                 `git archive HEAD` (the last commit), so those changes will NOT be included \
                 in the remote build.",
                dir
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unique_tmp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "asw-srcdir-test-{}-{}-{}",
            tag,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn explicit_src_flag_wins_over_everything() {
        let cwd = unique_tmp_dir("cwd-workspace-explicit");
        std::fs::write(cwd.join("Cargo.toml"), "[workspace]\nmembers = []\n").unwrap();
        let explicit = PathBuf::from("/explicit/src/dir");
        let fallback = PathBuf::from("/compile/time/fallback");

        let resolved = resolve_src_dir(Some(explicit.clone()), &cwd, &fallback);
        assert_eq!(resolved, explicit);

        let _ = std::fs::remove_dir_all(&cwd);
    }

    #[test]
    fn cwd_with_workspace_cargo_toml_wins_over_fallback() {
        let cwd = unique_tmp_dir("cwd-workspace");
        std::fs::write(cwd.join("Cargo.toml"), "[workspace]\nmembers = []\n").unwrap();
        let fallback = PathBuf::from("/compile/time/fallback");

        let resolved = resolve_src_dir(None, &cwd, &fallback);
        assert_eq!(resolved, cwd);

        let _ = std::fs::remove_dir_all(&cwd);
    }

    #[test]
    fn non_workspace_cwd_falls_back_to_compile_time_path() {
        let cwd = unique_tmp_dir("cwd-not-workspace");
        // A crate-member manifest (no [workspace] table) must NOT count as
        // the workspace root.
        std::fs::write(cwd.join("Cargo.toml"), "[package]\nname = \"foo\"\n").unwrap();
        let fallback = PathBuf::from("/compile/time/fallback");

        let resolved = resolve_src_dir(None, &cwd, &fallback);
        assert_eq!(resolved, fallback);

        let _ = std::fs::remove_dir_all(&cwd);
    }

    #[test]
    fn missing_cargo_toml_falls_back_to_compile_time_path() {
        let cwd = unique_tmp_dir("cwd-no-manifest");
        let fallback = PathBuf::from("/compile/time/fallback");

        let resolved = resolve_src_dir(None, &cwd, &fallback);
        assert_eq!(resolved, fallback);

        let _ = std::fs::remove_dir_all(&cwd);
    }

    #[test]
    fn explicit_src_flag_wins_even_over_a_workspace_cwd() {
        let cwd = unique_tmp_dir("cwd-workspace-vs-explicit");
        std::fs::write(cwd.join("Cargo.toml"), "[workspace]\nmembers = []\n").unwrap();
        let explicit = PathBuf::from("/some/other/checkout");
        let fallback = PathBuf::from("/compile/time/fallback");

        let resolved = resolve_src_dir(Some(explicit.clone()), &cwd, &fallback);
        assert_eq!(resolved, explicit);
        assert_ne!(resolved, cwd);

        let _ = std::fs::remove_dir_all(&cwd);
    }
}
