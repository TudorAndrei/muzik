//! Source fallback shared by video, search, and Spotify acquisition.

use crate::{AudioFallback, Result, check_cancelled, operation_error};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AcquisitionSource {
    Youtube,
    Soulseek,
}

#[derive(Debug)]
pub struct AcquiredAudio {
    pub files: Vec<PathBuf>,
    pub source: AcquisitionSource,
}

/// Acquire audio from the selected source, then apply the configured fallback.
/// The caller resolves `Auto` and owns cache and force behavior.
///
/// # Errors
/// Returns an error when acquisition fails or is cancelled.
pub fn acquire(
    first: AcquisitionSource,
    fallback: AudioFallback,
    cancelled: &AtomicBool,
    mut attempt: impl FnMut(AcquisitionSource) -> Result<Vec<PathBuf>, String>,
) -> Result<AcquiredAudio> {
    check_cancelled(cancelled)?;
    let result = attempt(first);
    check_cancelled(cancelled)?;
    let (source, files) = if first == AcquisitionSource::Soulseek
        && fallback == AudioFallback::Youtube
        && result.as_ref().map_or(true, Vec::is_empty)
    {
        let files = attempt(AcquisitionSource::Youtube);
        check_cancelled(cancelled)?;
        (AcquisitionSource::Youtube, files)
    } else {
        (first, result)
    };
    Ok(AcquiredAudio {
        files: files.map_err(|error| operation_error(error, cancelled))?,
        source,
    })
}
