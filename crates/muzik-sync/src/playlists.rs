use crate::{Result, sanitize};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

pub const FOLDER: &str = "Playlists";
const EXTENSIONS: [&str; 2] = ["m3u8", "m3u"];
const MARKER: &str = "#MUZIK-SOURCE:";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DevicePlaylist {
    pub source: PathBuf,
    pub destination: PathBuf,
    pub text: String,
    pub entries: usize,
    pub dropped: usize,
}

impl DevicePlaylist {
    #[must_use]
    pub fn is_current(&self) -> bool {
        fs::read_to_string(&self.destination).ok().as_deref() == Some(self.text.as_str())
    }
}

pub fn is_playlist(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            EXTENSIONS
                .iter()
                .any(|known| extension.eq_ignore_ascii_case(known))
        })
}

pub fn is_written(path: &Path) -> bool {
    is_playlist(path)
        && fs::read_to_string(path).is_ok_and(|text| {
            text.lines()
                .take(2)
                .any(|line| line.trim_start().starts_with(MARKER))
        })
}

/// # Errors
/// Returns an error if the playlist folder of the library cannot be read.
pub fn find(directory: &Path) -> Result<Vec<PathBuf>> {
    let entries = match fs::read_dir(directory.join(FOLDER)) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    let mut found = Vec::new();
    for entry in entries {
        let path = entry?.path();
        let hidden = path
            .file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with('.'));
        if !hidden && path.is_file() && is_playlist(&path) {
            found.push(path);
        }
    }
    found.sort();
    Ok(found)
}

pub fn rewrite(
    source: &Path,
    directory: &Path,
    root: &Path,
    destinations: &BTreeMap<PathBuf, PathBuf>,
) -> Option<DevicePlaylist> {
    let relative = sanitize(source.strip_prefix(directory).ok()?)?;
    let destination = root.join(relative);
    let folder = destination.parent()?;
    let bytes = fs::read(source).ok()?;
    let text = String::from_utf8(bytes)
        .unwrap_or_else(|error| error.into_bytes().iter().copied().map(char::from).collect());
    let base = source.parent()?;
    let mut output = String::new();
    let mut info: Option<&str> = None;
    let (mut entries, mut dropped) = (0_usize, 0_usize);
    for line in text.lines() {
        let line = line.trim_end_matches('\r');
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.starts_with("#EXTINF") {
            info = Some(line);
            continue;
        }
        if trimmed.starts_with('#') {
            let _ = writeln!(output, "{line}");
            continue;
        }
        let device = resolve(base, trimmed)
            .and_then(|track| destinations.get(&track))
            .and_then(|track| relative_to(folder, track));
        if let Some(device) = device {
            if let Some(info) = info.take() {
                let _ = writeln!(output, "{info}");
            }
            let _ = writeln!(output, "{device}");
            entries = entries.saturating_add(1);
        } else {
            info = None;
            dropped = dropped.saturating_add(1);
        }
    }
    Some(DevicePlaylist {
        source: source.to_path_buf(),
        destination,
        text: output,
        entries,
        dropped,
    })
}

fn resolve(base: &Path, entry: &str) -> Option<PathBuf> {
    let entry = entry.strip_prefix("file://").unwrap_or(entry);
    let path = Path::new(entry);
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    };
    let mut clean = PathBuf::new();
    for component in joined.components() {
        match component {
            Component::ParentDir => {
                if !clean.pop() {
                    return None;
                }
            }
            Component::CurDir => {}
            other => clean.push(other),
        }
    }
    Some(clean)
}

fn relative_to(folder: &Path, track: &Path) -> Option<String> {
    let mut common = 0_usize;
    for (left, right) in folder.components().zip(track.components()) {
        if left != right {
            break;
        }
        common = common.saturating_add(1);
    }
    if common == 0 {
        return None;
    }
    let ups = folder.components().count().saturating_sub(common);
    let mut parts: Vec<String> = std::iter::repeat_n("..".to_owned(), ups).collect();
    parts.extend(
        track
            .components()
            .skip(common)
            .map(|part| part.as_os_str().to_string_lossy().into_owned()),
    );
    Some(parts.join("/"))
}

/// # Errors
/// Returns an error if the playlist file cannot be written on the device.
pub fn write(playlist: &DevicePlaylist) -> Result<()> {
    if playlist.is_current() {
        return Ok(());
    }
    if let Some(parent) = playlist.destination.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut temporary = playlist.destination.as_os_str().to_owned();
    temporary.push(".muzik-part");
    let temporary = PathBuf::from(temporary);
    fs::write(&temporary, &playlist.text)?;
    fs::rename(&temporary, &playlist.destination)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{find, relative_to, rewrite};
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};

    #[test]
    fn a_device_playlist_points_to_the_device_files_and_drops_the_rest() {
        let library = tempfile::tempdir().unwrap();
        let directory = library.path();
        let folder = directory.join("Playlists");
        std::fs::create_dir_all(&folder).unwrap();
        let source = folder.join("Liked Songs.m3u8");
        std::fs::write(
            &source,
            "#EXTM3U\n#MUZIK-SOURCE:spotify:liked\n#EXTINF:200,A - One\n../A/One/01.flac\n#EXTINF:100,B - Two\n../B/02.flac\n",
        )
        .unwrap();
        std::fs::write(folder.join("notes.txt"), "").unwrap();
        assert_eq!(find(directory).unwrap(), std::slice::from_ref(&source));
        let root = Path::new("/Volumes/Player/Music");
        let destinations =
            BTreeMap::from([(directory.join("A/One/01.flac"), root.join("A/One/01.mp3"))]);
        let playlist = rewrite(&source, directory, root, &destinations).unwrap();
        assert_eq!(
            playlist.destination,
            root.join("Playlists/Liked Songs.m3u8")
        );
        assert_eq!(
            playlist.text,
            "#EXTM3U\n#MUZIK-SOURCE:spotify:liked\n#EXTINF:200,A - One\n../A/One/01.mp3\n"
        );
        assert_eq!((playlist.entries, playlist.dropped), (1, 1));
    }

    #[test]
    fn device_paths_are_relative_to_the_playlist_folder() {
        assert_eq!(
            relative_to(Path::new("/d/Playlists"), Path::new("/d/A/B/song.mp3")).as_deref(),
            Some("../A/B/song.mp3")
        );
        assert_eq!(
            relative_to(
                Path::new("/d/Playlists"),
                &PathBuf::from("/d/Playlists/x.mp3")
            )
            .as_deref(),
            Some("x.mp3")
        );
    }
}
