use std::env;
use std::io;
use std::path::PathBuf;
use std::process::Command;

pub fn open() -> io::Result<()> {
    let executable = find_binary().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "GPUI desktop binary is missing. Build rust/gpui_app or set MUZIK_GPUI_BIN.",
        )
    })?;
    let status = Command::new(executable).status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "desktop app exited with status {status}"
        )))
    }
}

fn find_binary() -> Option<PathBuf> {
    let name = if cfg!(windows) {
        "muzik-gpui.exe"
    } else {
        "muzik-gpui"
    };
    let mut candidates = Vec::new();
    if let Some(override_path) = env::var_os("MUZIK_GPUI_BIN") {
        candidates.push(PathBuf::from(override_path));
    } else {
        if let Ok(executable) = env::current_exe()
            && let Some(parent) = executable.parent()
        {
            candidates.push(parent.join(name));
        }
        let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..");
        candidates.push(repo.join("muzik/bin").join(name));
        candidates.push(repo.join("target/release").join(name));
        candidates.push(repo.join("target/debug").join(name));
    }
    candidates.into_iter().find(|path| path.is_file())
}
