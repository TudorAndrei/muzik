//! Build the macOS application bundle for the native desktop app.

use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

const BUNDLE_NAME: &str = "Muzik.app";
const LAUNCHER: &str = "muzik-launcher";
const LOGO: &[u8] = include_bytes!("../../../assets/muzik-logo-v2.png");

pub fn install(user: bool) -> io::Result<()> {
    if !cfg!(target_os = "macos") {
        return Err(io::Error::other("install-app is only supported on macOS"));
    }

    let executable = env::current_exe()?;
    let gpui = env::var_os("MUZIK_GPUI_BIN")
        .map(PathBuf::from)
        .or_else(|| executable.parent().map(|parent| parent.join("muzik-gpui")))
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "GPUI binary is missing"))?;
    if !gpui.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("GPUI binary is missing: {}", gpui.display()),
        ));
    }
    let target = target_dir(user)?;
    let icon = prepare_icon();
    let app = build_app_bundle(
        &target,
        &executable,
        &gpui,
        icon.as_ref().map(|(_, path)| path.as_path()),
        env!("CARGO_PKG_VERSION"),
    )?;

    println!("Installed {}", app.display());
    println!("  Bundled Rust CLI and desktop programs.");
    if icon.is_none() {
        println!("  No icon set (sips or logo unavailable).");
    }
    println!(
        "  Open it from Launchpad, Spotlight, or Finder. On first launch macOS may ask you to confirm an app from an unidentified developer."
    );
    Ok(())
}

fn target_dir(user: bool) -> io::Result<PathBuf> {
    let home = env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "HOME is not set"))?;
    let fallback = home.join("Applications");
    if user {
        fs::create_dir_all(&fallback)?;
        return Ok(fallback);
    }

    let system = Path::new("/Applications");
    if tempfile::Builder::new()
        .prefix(".muzik-write-check-")
        .tempfile_in(system)
        .is_ok()
    {
        return Ok(system.to_path_buf());
    }
    println!(
        "/Applications is not writable; using {} instead.",
        fallback.display()
    );
    fs::create_dir_all(&fallback)?;
    Ok(fallback)
}

fn prepare_icon() -> Option<(tempfile::TempDir, PathBuf)> {
    let directory = tempfile::Builder::new()
        .prefix("muzik-icon-")
        .tempdir()
        .ok()?;
    let source = directory.path().join("source.png");
    let png = directory.path().join("icon.png");
    let icns = directory.path().join("muzik.icns");
    fs::write(&source, LOGO).ok()?;

    let png_status = Command::new("sips")
        .args(["-s", "format", "png", "--resampleHeightWidth", "512", "512"])
        .arg(&source)
        .arg("--out")
        .arg(&png)
        .output()
        .ok()?;
    if !png_status.status.success() {
        return None;
    }
    let icns_status = Command::new("sips")
        .args(["-s", "format", "icns"])
        .arg(&png)
        .arg("--out")
        .arg(&icns)
        .output()
        .ok()?;
    if !icns_status.status.success() || !icns.is_file() {
        return None;
    }
    Some((directory, icns))
}

fn build_app_bundle(
    target_dir: &Path,
    executable: &Path,
    gpui: &Path,
    icns: Option<&Path>,
    version: &str,
) -> io::Result<PathBuf> {
    let app = target_dir.join(BUNDLE_NAME);
    let staging = tempfile::Builder::new()
        .prefix(".muzik-install-")
        .tempdir_in(target_dir)?;
    let staged_app = staging.path().join(BUNDLE_NAME);
    let contents = staged_app.join("Contents");
    let macos = contents.join("MacOS");
    let resources = contents.join("Resources");
    fs::create_dir_all(&macos)?;
    fs::create_dir_all(&resources)?;
    fs::copy(executable, macos.join("muzik"))?;
    fs::copy(gpui, macos.join("muzik-gpui"))?;

    let launcher = macos.join(LAUNCHER);
    fs::write(
        &launcher,
        "#!/bin/sh\nexec \"$(dirname \"$0\")/muzik\" gui\n",
    )?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&launcher, fs::Permissions::from_mode(0o755))?;
    }

    let has_icon = icns.is_some_and(Path::is_file);
    if let Some(icon) = icns.filter(|path| path.is_file()) {
        fs::copy(icon, resources.join("muzik.icns"))?;
    }
    fs::write(contents.join("Info.plist"), info_plist(version, has_icon))?;
    let backup = staging.path().join("previous.app");
    if app.exists() {
        fs::rename(&app, &backup)?;
    }
    if let Err(error) = fs::rename(&staged_app, &app) {
        if backup.exists() {
            let _ = fs::rename(&backup, &app);
        }
        return Err(error);
    }
    Ok(app)
}

fn info_plist(version: &str, has_icon: bool) -> String {
    let version = xml_escape(version);
    let icon = if has_icon {
        "  <key>CFBundleIconFile</key><string>muzik</string>\n"
    } else {
        ""
    };
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\">\n<dict>\n\
           <key>CFBundleName</key><string>Muzik</string>\n\
           <key>CFBundleDisplayName</key><string>Muzik</string>\n\
           <key>CFBundleIdentifier</key><string>com.tudorandrei.muzik</string>\n\
           <key>CFBundleExecutable</key><string>muzik-launcher</string>\n\
           <key>CFBundlePackageType</key><string>APPL</string>\n\
           <key>CFBundleVersion</key><string>{version}</string>\n\
           <key>CFBundleShortVersionString</key><string>{version}</string>\n\
           <key>LSMinimumSystemVersion</key><string>13.0</string>\n\
           <key>NSHighResolutionCapable</key><true/>\n{icon}\
         </dict>\n</plist>\n"
    )
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use super::build_app_bundle;
    use std::fs;
    #[cfg(unix)]
    use std::process::Command;

    #[test]
    fn bundle_has_launcher_plist_and_optional_icon() -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempfile::tempdir()?;
        let icon = temp.path().join("source.icns");
        let cli = temp.path().join("muzik-source");
        let gpui = temp.path().join("gpui-source");
        fs::write(&icon, b"icns-bytes")?;
        fs::write(&cli, b"cli-bytes")?;
        fs::write(&gpui, b"gpui-bytes")?;
        let app = build_app_bundle(temp.path(), &cli, &gpui, Some(&icon), "1.2.3")?;
        let launcher = app.join("Contents/MacOS/muzik-launcher");
        assert_eq!(
            fs::read_to_string(&launcher)?,
            "#!/bin/sh\nexec \"$(dirname \"$0\")/muzik\" gui\n"
        );
        assert_eq!(fs::read(app.join("Contents/MacOS/muzik"))?, b"cli-bytes");
        assert_eq!(
            fs::read(app.join("Contents/MacOS/muzik-gpui"))?,
            b"gpui-bytes"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_ne!(fs::metadata(&launcher)?.permissions().mode() & 0o100, 0);
        }
        let plist = fs::read_to_string(app.join("Contents/Info.plist"))?;
        assert!(
            plist.contains("<key>CFBundleIdentifier</key><string>com.tudorandrei.muzik</string>")
        );
        assert!(plist.contains("<key>CFBundleShortVersionString</key><string>1.2.3</string>"));
        assert!(plist.contains("<key>CFBundleIconFile</key><string>muzik</string>"));
        assert_eq!(
            fs::read(app.join("Contents/Resources/muzik.icns"))?,
            b"icns-bytes"
        );
        Ok(())
    }

    #[test]
    fn bundle_replaces_existing_and_can_copy_its_own_cli() -> Result<(), Box<dyn std::error::Error>>
    {
        let temp = tempfile::tempdir()?;
        let cli = temp.path().join("muzik-source");
        let gpui = temp.path().join("gpui-source");
        fs::write(&cli, b"cli-v1")?;
        fs::write(&gpui, b"gpui-v1")?;
        let app = build_app_bundle(temp.path(), &cli, &gpui, None, "0.1.0")?;
        fs::write(app.join("old"), "old")?;
        let old_cli = app.join("Contents/MacOS/muzik");
        let old_gpui = app.join("Contents/MacOS/muzik-gpui");
        let app = build_app_bundle(temp.path(), &old_cli, &old_gpui, None, "0.1.0")?;
        assert!(!app.join("old").exists());
        assert_eq!(fs::read(app.join("Contents/MacOS/muzik"))?, b"cli-v1");
        assert!(!fs::read_to_string(app.join("Contents/Info.plist"))?.contains("CFBundleIconFile"));
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn bundle_launcher_runs_from_a_path_with_spaces() -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir()?;
        let target = temp.path().join("Music App's Folder");
        fs::create_dir(&target)?;
        let cli = temp.path().join("muzik-source");
        let gpui = temp.path().join("gpui-source");
        fs::write(&cli, "#!/bin/sh\nprintf '%s\\n' \"$1\"\n")?;
        fs::set_permissions(&cli, fs::Permissions::from_mode(0o755))?;
        fs::write(&gpui, b"gpui")?;
        let app = build_app_bundle(&target, &cli, &gpui, None, "0.1.0")?;
        let result = Command::new(app.join("Contents/MacOS/muzik-launcher")).output()?;
        assert!(result.status.success());
        assert_eq!(result.stdout, b"gui\n");
        Ok(())
    }
}
