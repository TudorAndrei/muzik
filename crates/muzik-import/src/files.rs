//! File operations used when an import places tracks in the library.

use std::fs;
use std::io;
use std::path::Path;

use tracing::debug;

pub use crate::Error as FileError;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Placement {
    Move,
    Copy,
    Symlink,
    Hardlink,
    Reflink,
}

/// Place one regular file without replacing an existing destination.
pub(crate) fn place(source: &Path, destination: &Path, mode: Placement) -> Result<(), FileError> {
    if !source.is_file() {
        return Err(FileError::InvalidSource(source.to_owned()));
    }
    if destination.exists() || destination.symlink_metadata().is_ok() {
        return Err(FileError::DestinationExists(destination.to_owned()));
    }
    let parent = destination_parent(destination);
    fs::create_dir_all(parent)?;
    debug!(?mode, ?source, ?destination, "place import file");
    match mode {
        Placement::Move => move_file(source, destination)?,
        Placement::Copy => {
            fs::copy(source, destination)?;
        }
        Placement::Symlink => symlink_file(source, destination)?,
        Placement::Hardlink => fs::hard_link(source, destination)?,
        Placement::Reflink => {
            reflink_copy::reflink(source, destination)?;
        }
    }
    Ok(())
}

fn destination_parent(destination: &Path) -> &Path {
    destination
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

fn move_file(source: &Path, destination: &Path) -> io::Result<()> {
    match fs::rename(source, destination) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::CrossesDevices => {
            copy_then_remove(source, destination)
        }
        Err(error) => Err(error),
    }
}

fn copy_then_remove(source: &Path, destination: &Path) -> io::Result<()> {
    // Copy to the destination filesystem before deleting the source.
    let parent = destination
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "destination has no parent"))?;
    let temporary = tempfile::NamedTempFile::new_in(parent)?;
    fs::copy(source, temporary.path())?;
    temporary.persist_noclobber(destination)?;
    fs::remove_file(source)
}

#[cfg(unix)]
fn symlink_file(source: &Path, destination: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(source.canonicalize()?, destination)
}

#[cfg(windows)]
fn symlink_file(source: &Path, destination: &Path) -> io::Result<()> {
    std::os::windows::fs::symlink_file(source.canonicalize()?, destination)
}

/// Move a file to the operating system trash.
pub(crate) fn move_to_trash(path: &Path) -> Result<(), FileError> {
    trash::delete(path)?;
    Ok(())
}

/// Remove empty parent directories up to, but not including, `root`.
/// Returns the number of directories removed.
///
/// # Errors
/// Returns an error when `path` is outside `root` or a directory cannot be removed.
pub fn prune_empty_parents(path: &Path, root: &Path) -> Result<usize, FileError> {
    let root = root.canonicalize()?;
    let mut current = path.canonicalize()?;
    if !current.starts_with(&root) {
        return Err(FileError::OutsideRoot(path.to_owned()));
    }
    let mut removed = 0;
    while current != root {
        match fs::remove_dir(&current) {
            Ok(()) => removed = usize::saturating_add(removed, 1),
            Err(error) if error.kind() == io::ErrorKind::DirectoryNotEmpty => break,
            Err(error) => return Err(error.into()),
        }
        current = current
            .parent()
            .ok_or_else(|| FileError::OutsideRoot(path.to_owned()))?
            .to_owned();
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copy_and_move_preserve_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.flac");
        fs::write(&source, b"audio").unwrap();
        let copied = dir.path().join("album/copied.flac");
        place(&source, &copied, Placement::Copy).unwrap();
        assert_eq!(fs::read(&copied).unwrap(), b"audio");
        assert!(source.exists());
        let moved = dir.path().join("album/moved.flac");
        place(&source, &moved, Placement::Move).unwrap();
        assert_eq!(fs::read(&moved).unwrap(), b"audio");
        assert!(!source.exists());
    }

    #[test]
    fn hardlink_and_symlink_share_source() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.flac");
        fs::write(&source, b"before").unwrap();
        let hardlink = dir.path().join("hardlink.flac");
        let symlink = dir.path().join("symlink.flac");
        place(&source, &hardlink, Placement::Hardlink).unwrap();
        place(&source, &symlink, Placement::Symlink).unwrap();
        fs::write(&source, b"after").unwrap();
        assert_eq!(fs::read(&hardlink).unwrap(), b"after");
        assert_eq!(fs::read(&symlink).unwrap(), b"after");
    }

    #[test]
    fn never_replace_an_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.flac");
        let destination = dir.path().join("destination.flac");
        fs::write(&source, b"source").unwrap();
        fs::write(&destination, b"destination").unwrap();
        assert!(matches!(
            place(&source, &destination, Placement::Move),
            Err(FileError::DestinationExists(_))
        ));
        assert_eq!(fs::read(&destination).unwrap(), b"destination");
        assert!(source.exists());
    }

    #[test]
    fn cross_device_fallback_copies_before_removing_source() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.flac");
        let destination = dir.path().join("album/destination.flac");
        fs::write(&source, b"audio").unwrap();
        fs::create_dir_all(destination.parent().unwrap()).unwrap();
        copy_then_remove(&source, &destination).unwrap();
        assert_eq!(fs::read(&destination).unwrap(), b"audio");
        assert!(!source.exists());
    }

    #[test]
    fn prune_stops_at_root_and_nonempty_directory() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("library");
        let album = root.join("artist/album");
        fs::create_dir_all(&album).unwrap();
        assert_eq!(prune_empty_parents(&album, &root).unwrap(), 2);
        assert!(root.exists());
        assert!(matches!(
            prune_empty_parents(dir.path(), &root),
            Err(FileError::OutsideRoot(_))
        ));
    }
}
