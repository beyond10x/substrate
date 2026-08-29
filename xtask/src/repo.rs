//! Finding the repository root.
//!
//! The Python predecessors read it off their own file (`Path(__file__).resolve().parent.parent`).
//! A compiled binary cannot: the path baked in at build time survives a moved checkout and a
//! cached `target/`. So the root is discovered at run time — the nearest ancestor of the working
//! directory whose `Cargo.toml` declares `[workspace]` — and the build-time manifest is only the
//! fallback for an invocation from outside any workspace.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// The workspace root this invocation belongs to, canonicalised.
pub fn root() -> Result<PathBuf> {
    if let Ok(working) = std::env::current_dir() {
        for candidate in working.ancestors() {
            if declares_workspace(&candidate.join("Cargo.toml")) {
                return canonical(candidate);
            }
        }
    }
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let built_from = manifest
        .parent()
        .with_context(|| format!("{} has no parent directory", manifest.display()))?;
    canonical(built_from)
}

fn declares_workspace(manifest: &Path) -> bool {
    fs::read_to_string(manifest)
        .is_ok_and(|text| text.lines().any(|line| line.trim_end() == "[workspace]"))
}

fn canonical(path: &Path) -> Result<PathBuf> {
    path.canonicalize()
        .with_context(|| format!("cannot resolve {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::declares_workspace;
    use std::fs;

    #[test]
    fn a_workspace_manifest_is_recognised_and_a_package_manifest_is_not() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let workspace = directory.path().join("workspace.toml");
        let package = directory.path().join("package.toml");
        fs::write(&workspace, "[workspace]\nmembers = []\n").expect("write");
        fs::write(&package, "[package]\nname = \"x\"\n").expect("write");
        assert!(declares_workspace(&workspace));
        assert!(!declares_workspace(&package));
        assert!(!declares_workspace(&directory.path().join("absent.toml")));
    }
}
