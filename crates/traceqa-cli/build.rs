//! Embeds the commit a release binary was built from, as TRACEQA_COMMIT (40 hex digits, or
//! empty), so `traceqa install` can install the drivers from that exact commit and not from a
//! tag, which can be moved.
//!
//! Where the commit comes from, in order:
//!   1. TRACEQA_RELEASE_COMMIT, when it is set to a full commit id. The release workflow sets it.
//!   2. `git rev-parse HEAD`, for a release-profile build of a clean checkout.
//!   3. Nothing. A debug build, a dirty tree or a missing git binary gets an empty value: the
//!      commit of an unpushed change would not exist on GitHub, and `traceqa install` would
//!      fail on it. The installer then falls back to the release tag and says so.
//!
//! The build never fails because of any of this.

use std::path::Path;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-env-changed=TRACEQA_RELEASE_COMMIT");
    println!("cargo:rerun-if-env-changed=PROFILE");
    println!("cargo:rerun-if-changed=build.rs");
    // A new commit changes HEAD (or the branch it points to, which moves its reflog). In a
    // worktree `.git` is a file, so ask git where these live.
    for name in ["HEAD", "logs/HEAD"] {
        if let Some(path) = git(&["rev-parse", "--git-path", name]) {
            println!("cargo:rerun-if-changed={path}");
        }
    }
    println!(
        "cargo:rustc-env=TRACEQA_COMMIT={}",
        release_commit().unwrap_or_default()
    );
}

fn release_commit() -> Option<String> {
    if let Ok(given) = std::env::var("TRACEQA_RELEASE_COMMIT") {
        let given = given.trim().to_ascii_lowercase();
        if is_commit(&given) {
            return Some(given);
        }
        if !given.is_empty() {
            println!("cargo:warning=TRACEQA_RELEASE_COMMIT is not a full commit id and was ignored");
        }
    }
    if std::env::var("PROFILE").ok()? != "release" {
        return None;
    }
    // Changes to tracked files make the tree dirty; build output and other untracked files do not.
    if !git(&["status", "--porcelain", "--untracked-files=no"])?.is_empty() {
        return None;
    }
    let head = git(&["rev-parse", "HEAD"])?.to_ascii_lowercase();
    is_commit(&head).then_some(head)
}

fn is_commit(text: &str) -> bool {
    text.len() == 40 && text.bytes().all(|b| b.is_ascii_hexdigit())
}

/// What git prints for these arguments in this package's folder, trimmed. None when git is
/// missing, this is not a repository, or git fails.
fn git(args: &[&str]) -> Option<String> {
    let dir = std::env::var_os("CARGO_MANIFEST_DIR")?;
    let output = Command::new("git")
        .args(args)
        .current_dir(Path::new(&dir))
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}
