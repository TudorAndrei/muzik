use std::error::Error;
use std::fs;
use std::path::Path;
use std::process::Command;

fn main() -> Result<(), Box<dyn Error>> {
    let version = std::env::args()
        .nth(1)
        .ok_or("usage: muzik-release VERSION")?;
    if !valid_version(&version) {
        return Err("version must have MAJOR.MINOR.PATCH format".into());
    }

    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let crates = root.join("rust/crates");
    let mut manifests = Vec::new();
    for entry in fs::read_dir(crates)? {
        let path = entry?.path().join("Cargo.toml");
        if path.is_file() {
            manifests.push(path);
        }
    }
    manifests.push(root.join("rust/gpui_app/Cargo.toml"));
    manifests.sort();
    for manifest in manifests {
        set_version(&manifest, &version)?;
    }

    let status = Command::new("cargo")
        .args(["update", "--workspace", "--offline", "--manifest-path"])
        .arg(root.join("Cargo.toml"))
        .status()?;
    if !status.success() {
        return Err(format!("cargo update failed with {status}").into());
    }
    Ok(())
}

fn valid_version(version: &str) -> bool {
    version.split('.').count() == 3
        && version
            .split('.')
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
}

fn set_version(manifest: &Path, version: &str) -> Result<(), Box<dyn Error>> {
    let source = fs::read_to_string(manifest)?;
    if !source.starts_with("[package]\n") {
        return Err(format!("package table is missing in {}", manifest.display()).into());
    }
    let old = source
        .lines()
        .find(|line| line.starts_with("version = \""))
        .ok_or_else(|| format!("version is missing in {}", manifest.display()))?;
    let updated = source.replacen(old, &format!("version = \"{version}\""), 1);
    if updated != source {
        fs::write(manifest, updated)?;
    }
    Ok(())
}
