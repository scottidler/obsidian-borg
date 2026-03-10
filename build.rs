use std::path::Path;
use std::process::Command;

fn main() {
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

    println!("cargo:rustc-env=GIT_DESCRIBE={}", git_describe);
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/refs/");

    // Sync clients/extension/manifest.json version with Cargo.toml
    sync_manifest_version();
    println!("cargo:rerun-if-changed=Cargo.toml");
}

fn sync_manifest_version() {
    let version = env!("CARGO_PKG_VERSION");
    let manifest_path = Path::new("clients/extension/manifest.json");
    if !manifest_path.exists() {
        return;
    }
    let Ok(content) = std::fs::read_to_string(manifest_path) else {
        return;
    };
    let Ok(mut json) = serde_json::from_str::<serde_json::Value>(&content) else {
        return;
    };
    let old = json["version"].as_str().unwrap_or("");
    if old == version {
        return;
    }
    json["version"] = serde_json::Value::String(version.to_string());
    if let Ok(out) = serde_json::to_string_pretty(&json) {
        let _ = std::fs::write(manifest_path, out + "\n");
    }
}
