//! Build script: embeds the git short hash so every log line block at
//! startup can identify the exact build ("which version produced this log")
//! without shipping metadata files.

use std::process::Command;

fn main() {
    let sha = Command::new("git")
        .args(["rev-parse", "--short=7", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".into());
    // Mark uncommitted builds so a locally-patched binary is not confused
    // with a CI build of the same commit.
    let dirty = Command::new("git")
        .args(["diff", "--quiet"])
        .status()
        .map(|s| !s.success())
        .unwrap_or(false);
    let sha = if dirty { format!("{sha}-dirty") } else { sha };
    println!("cargo:rustc-env=RUSTBLOX_BUILD_SHA={sha}");
    // Re-run when HEAD (or the working tree diff state) changes.
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/index");
}
