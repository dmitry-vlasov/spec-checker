use std::process::Command;

fn main() {
    // Embed git commit hash at build time
    let output = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output();

    let git_hash = match output {
        Ok(o) if o.status.success() => {
            String::from_utf8_lossy(&o.stdout).trim().to_string()
        }
        _ => "unknown".to_string(),
    };

    println!("cargo:rustc-env=GIT_HASH={}", git_hash);
    // Always rerun build script so git hash stays current
    println!("cargo:rerun-if-changed=FORCE_REBUILD");
}
