use std::process::Command;

fn main() {
    // Git short hash
    let git_hash = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .unwrap_or_else(|| "unknown".into());
    println!("cargo:rustc-env=MIDINET_GIT_HASH={}", git_hash.trim());

    // Git branch — handle ambiguous refs (tag + branch with same name like "v3.1")
    let raw_branch = Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .unwrap_or_else(|| "unknown".into());
    let raw_branch = raw_branch.trim();

    let git_branch = if raw_branch == "HEAD" {
        // Detached HEAD (e.g. tag checkout) — try symbolic-ref
        Command::new("git")
            .args(["symbolic-ref", "--short", "HEAD"])
            .output()
            .ok()
            .and_then(|o| {
                if o.status.success() {
                    String::from_utf8(o.stdout).ok()
                } else {
                    None
                }
            })
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|| "unknown".into())
    } else {
        raw_branch.to_string()
    };

    // Strip "heads/" prefix that git adds when tag/branch names collide
    // (e.g. "heads/v3.1" → "v3.1")
    let git_branch = git_branch
        .strip_prefix("heads/")
        .unwrap_or(&git_branch)
        .to_string();

    println!("cargo:rustc-env=MIDINET_GIT_BRANCH={}", git_branch);

    // Build timestamp (UTC)
    let build_time = Command::new("date")
        .args(["-u", "+%Y-%m-%d %H:%M UTC"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .unwrap_or_else(|| "unknown".into());
    println!("cargo:rustc-env=MIDINET_BUILD_TIME={}", build_time.trim());

    // Re-run if git HEAD changes
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    println!("cargo:rerun-if-changed=../../.git/refs/");
}
