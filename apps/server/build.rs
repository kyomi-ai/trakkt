fn main() {
    let sha = std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .map_err(|e| {
            eprintln!("cargo:warning=build.rs: git command failed: {e}");
            e
        })
        .ok()
        .and_then(|o| {
            if o.status.success() {
                String::from_utf8(o.stdout).ok().map(|s| s.trim().to_string())
            } else {
                eprintln!("cargo:warning=build.rs: git rev-parse failed (exit {:?})", o.status);
                None
            }
        })
        .unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=TRAKKT_GIT_SHA={sha}");
}
