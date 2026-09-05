use std::process::Command;

fn main() {
    // Do not add `--dirty` here without changing `parse_build_version`
    // (`src/cli/commands/upgrade.rs`) first. It reads the `-<n>-g<sha>` suffix
    // to order a dev build ABOVE the tag it descends from; `--dirty` appends
    // another field, the suffix match fails, and semver falls back to reading
    // the whole thing as a PRERELEASE of that tag, which sorts below it. That
    // is the exact inversion `cffb143` fixed, and it would come back silently:
    // the string still parses, it just orders backwards.
    let git_describe = Command::new("git")
        .args(["describe", "--tags", "--always"])
        .output()
        .and_then(|output| {
            if output.status.success() {
                Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
            } else {
                Err(std::io::Error::other("git describe failed"))
            }
        })
        .unwrap_or_else(|_| {
            // Fallback to Cargo.toml version when git describe fails
            env!("CARGO_PKG_VERSION").to_string()
        });

    println!("cargo:rustc-env=GIT_DESCRIBE={git_describe}");
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/refs/");
}
