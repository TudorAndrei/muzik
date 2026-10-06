//! Copy library tracks to a device folder in formats that the device plays.

use muzik_core::audio::Codec;
use muzik_core::{app_config, paths, SyncPreset};
use muzik_media::ffmpeg::{Convert, Ffmpeg};
use muzik_media::quality::MeasuredQuality;
use muzik_store::{sync_files, Connection};
use rayon::prelude::*;
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs::{self, File};
use std::io;
use std::path::{Component, Path, PathBuf};
use strum_macros::EnumString;

mod error;
mod run;

pub use error::{Error, Result};
pub use muzik_media::ffmpeg::Encoding;
pub use run::{apply, prepare, select, Done, Options, Prepared, Report, Selection, Shortfall};

const SECTION: &str = "sync";
const PARTIAL: &str = "muzik-part";

#[derive(Clone, Copy, Debug, PartialEq, Eq, EnumString)]
#[strum(serialize_all = "lowercase", ascii_case_insensitive)]
enum DeviceFile {
    Aac,
    #[strum(serialize = "aif", serialize = "aiff")]
    Aiff,
    Ape,
    Dff,
    Dsf,
    Flac,
    #[strum(serialize = "jpeg", serialize = "jpg")]
    Jpeg,
    M4a,
    Mp3,
    Mp4,
    Ogg,
    Opus,
    Png,
    Wav,
    Wma,
}

impl DeviceFile {
    fn from_path(path: &Path) -> Option<Self> {
        path.extension()?.to_str()?.parse().ok()
    }

    fn codec(self) -> Option<Codec> {
        match self {
            Self::Flac => Some(Codec::Flac),
            Self::Mp3 => Some(Codec::Mp3),
            Self::Opus => Some(Codec::Opus),
            Self::Wav => Some(Codec::Pcm("pcm_s16le".into())),
            Self::Aiff => Some(Codec::Pcm("pcm_s16be".into())),
            Self::Ape => Some(Codec::Ape),
            Self::Dsf => Some(Codec::Dsd("dsd_lsbf_planar".into())),
            Self::Dff => Some(Codec::Dsd("dsd_msbf".into())),
            Self::Wma => Some(Codec::WmaV2),
            Self::Aac | Self::Jpeg | Self::M4a | Self::Mp4 | Self::Ogg | Self::Png => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub path: PathBuf,
    pub preset: SyncPreset,
    pub bitrate: Option<u32>,
    pub covers: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    Copy,
    Convert(Encoding),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Transfer {
    pub source: PathBuf,
    pub destination: PathBuf,
    pub action: Action,
    pub tags_in_stream: bool,
    pub cover: bool,
    pub bytes: u64,
}

#[derive(Debug, Default)]
pub struct Plan {
    pub fresh: usize,
    pub pending: Vec<Transfer>,
    pub outside: Vec<PathBuf>,
    pub unreadable: Vec<PathBuf>,
    pub duplicates: Vec<PathBuf>,
    pub planned: BTreeSet<PathBuf>,
}

enum Step {
    Fresh(PathBuf),
    Pending(Transfer),
    Outside(PathBuf),
    Unreadable(PathBuf),
}

impl Target {
    pub fn load(config: &Value, name: &str) -> Result<Self> {
        let targets = config.get(SECTION).and_then(Value::as_object);
        let Some(entry) = targets.and_then(|targets| targets.get(name)) else {
            let names = targets
                .map(|targets| targets.keys().cloned().collect::<Vec<_>>().join(", "))
                .unwrap_or_default();
            return Err(Error::Message(if names.is_empty() {
                format!("no sync target named {name}; add one with `muzik config set-sync-target`")
            } else {
                format!("no sync target named {name}; configured targets: {names}")
            }));
        };
        let path = entry
            .get("path")
            .and_then(Value::as_str)
            .filter(|path| !path.trim().is_empty())
            .ok_or_else(|| format!("sync target {name} has no path"))?;
        let preset = match entry.get("preset") {
            None | Some(Value::Null) => SyncPreset::default(),
            Some(value) => value
                .as_str()
                .ok_or_else(|| format!("sync target {name}: preset must be a string"))?
                .parse::<SyncPreset>()?,
        };
        let bitrate = match entry.get("bitrate") {
            None | Some(Value::Null) => None,
            Some(value) => Some(
                value
                    .as_u64()
                    .and_then(|value| u32::try_from(value).ok())
                    .ok_or_else(|| format!("sync target {name}: bitrate must be kbps"))?,
            ),
        };
        let covers = match entry.get("covers") {
            None | Some(Value::Null) => true,
            Some(value) => value
                .as_bool()
                .ok_or_else(|| format!("sync target {name}: covers must be true or false"))?,
        };
        let target = Self {
            path: paths::expand_home(Path::new(path)),
            preset,
            bitrate,
            covers,
        };
        target.validate()?;
        Ok(target)
    }

    pub fn save(&self, config_path: &Path, name: &str) -> Result<()> {
        self.validate()?;
        let name = name.trim();
        if name.is_empty() {
            return Err("sync target name must not be empty".into());
        }
        let mut entry = Map::new();
        entry.insert("path".into(), json!(self.path));
        entry.insert("preset".into(), json!(self.preset));
        if let Some(bitrate) = self.bitrate {
            entry.insert("bitrate".into(), json!(bitrate));
        }
        entry.insert("covers".into(), json!(self.covers));
        app_config::save_section_value(config_path, SECTION, name, Value::Object(entry))?;
        Ok(())
    }

    fn validate(&self) -> Result<()> {
        let range = match self.preset {
            SyncPreset::EchoMini | SyncPreset::Mp3 => 32..=320,
            SyncPreset::Opus => 6..=512,
        };
        match self.bitrate {
            Some(bitrate) if !range.contains(&bitrate) => Err(Error::Message(format!(
                "bitrate for {} must be from {} to {} kbps",
                self.preset,
                range.start(),
                range.end()
            ))),
            _ => Ok(()),
        }
    }

    pub fn action(&self, audio: &MeasuredQuality) -> Action {
        let codec = &audio.format;
        match self.preset {
            SyncPreset::EchoMini => {
                let plays = codec.is_lossless()
                    || matches!(
                        codec,
                        Codec::Aac | Codec::Mp3 | Codec::Vorbis | Codec::WmaV1 | Codec::WmaV2
                    );
                if !plays {
                    return Action::Convert(Encoding::Mp3 {
                        kbps: self.bitrate.unwrap_or(320),
                    });
                }
                if matches!(codec, Codec::Dsd(_)) {
                    return Action::Copy;
                }
                let sample_rate = audio
                    .sample_rate
                    .filter(|rate| *rate > 192_000)
                    .map(|rate| if rate % 44_100 == 0 { 176_400 } else { 192_000 });
                let bit_depth = audio.bit_depth.filter(|depth| *depth > 24).map(|_| 24);
                if sample_rate.is_none() && bit_depth.is_none() {
                    Action::Copy
                } else {
                    Action::Convert(Encoding::Flac {
                        sample_rate,
                        bit_depth,
                    })
                }
            }
            SyncPreset::Mp3 => {
                if *codec == Codec::Mp3 {
                    Action::Copy
                } else {
                    Action::Convert(Encoding::Mp3 {
                        kbps: self.bitrate.unwrap_or(320),
                    })
                }
            }
            SyncPreset::Opus => {
                if matches!(codec, Codec::Aac | Codec::Mp3 | Codec::Opus | Codec::Vorbis) {
                    Action::Copy
                } else {
                    Action::Convert(Encoding::Opus {
                        kbps: self.bitrate.unwrap_or(192),
                    })
                }
            }
        }
    }

    fn destination(&self, directory: &Path, source: &Path, action: &Action) -> Option<PathBuf> {
        let relative = sanitize(source.strip_prefix(directory).ok()?)?;
        Some(match action {
            Action::Copy => self.path.join(relative),
            Action::Convert(encoding) => self
                .path
                .join(relative)
                .with_extension(encoding.extension()),
        })
    }
}

impl Plan {
    pub fn bytes_needed(&self) -> u64 {
        self.pending
            .iter()
            .map(|transfer| {
                let existing = fs::metadata(&transfer.destination).map_or(0, |meta| meta.len());
                transfer.bytes.saturating_sub(existing)
            })
            .sum()
    }
}

pub fn plan(
    target: &Target,
    directory: &Path,
    tracks: &[PathBuf],
    covers: &[PathBuf],
    encodings: &BTreeMap<PathBuf, Encoding>,
    jobs: usize,
    probe: &(dyn Fn(&Path) -> Result<Option<MeasuredQuality>, String> + Sync),
) -> Plan {
    let mut steps = parallel(tracks, jobs, |source| {
        plan_track(target, directory, source, encodings, probe)
    });
    steps.extend(
        covers
            .iter()
            .map(|source| plan_cover(target, directory, source)),
    );
    let mut plan = Plan::default();
    let mut taken = HashSet::new();
    let mut claim = |destination: &Path| taken.insert(destination.to_string_lossy().to_lowercase());
    for (source, step) in tracks.iter().chain(covers).zip(steps) {
        match step {
            Step::Fresh(destination) if claim(&destination) => {
                plan.fresh += 1;
                plan.planned.insert(destination);
            }
            Step::Pending(transfer) if claim(&transfer.destination) => {
                plan.planned.insert(transfer.destination.clone());
                plan.pending.push(transfer);
            }
            Step::Fresh(_) | Step::Pending(_) => plan.duplicates.push(source.clone()),
            Step::Outside(source) => plan.outside.push(source),
            Step::Unreadable(source) => plan.unreadable.push(source),
        }
    }
    plan
}

fn plan_track(
    target: &Target,
    directory: &Path,
    source: &Path,
    encodings: &BTreeMap<PathBuf, Encoding>,
    probe: &(dyn Fn(&Path) -> Result<Option<MeasuredQuality>, String> + Sync),
) -> Step {
    if source.strip_prefix(directory).is_err() {
        return Step::Outside(source.to_path_buf());
    }
    if let Some(action) = guess(source).map(|audio| target.action(&audio)) {
        if let Some(destination) = target.destination(directory, source, &action) {
            if is_current(source, &destination, &action, encodings) {
                return Step::Fresh(destination);
            }
        }
    }
    let Ok(Some(audio)) = probe(source) else {
        return Step::Unreadable(source.to_path_buf());
    };
    let action = target.action(&audio);
    let Some(destination) = target.destination(directory, source, &action) else {
        return Step::Outside(source.to_path_buf());
    };
    if is_current(source, &destination, &action, encodings) {
        return Step::Fresh(destination);
    }
    let size = audio.size.unwrap_or(0);
    let bytes = match &action {
        Action::Convert(Encoding::Mp3 { kbps } | Encoding::Opus { kbps }) => {
            match audio.bitrate_kbps.filter(|bitrate| *bitrate > 0) {
                Some(bitrate) => size.saturating_mul(u64::from(*kbps)) / u64::from(bitrate),
                None => size,
            }
        }
        Action::Copy | Action::Convert(Encoding::Flac { .. }) => size,
    };
    Step::Pending(Transfer {
        source: source.to_path_buf(),
        destination,
        tags_in_stream: matches!(audio.format, Codec::Opus | Codec::Vorbis),
        cover: target.covers && action != Action::Copy,
        action,
        bytes,
    })
}

fn plan_cover(target: &Target, directory: &Path, source: &Path) -> Step {
    let Some(destination) = target.destination(directory, source, &Action::Copy) else {
        return Step::Outside(source.to_path_buf());
    };
    if is_fresh(source, &destination, true) {
        return Step::Fresh(destination);
    }
    let Ok(meta) = fs::metadata(source) else {
        return Step::Unreadable(source.to_path_buf());
    };
    Step::Pending(Transfer {
        source: source.to_path_buf(),
        destination,
        action: Action::Copy,
        tags_in_stream: false,
        cover: false,
        bytes: meta.len(),
    })
}

fn guess(source: &Path) -> Option<MeasuredQuality> {
    let format = DeviceFile::from_path(source)?.codec()?;
    Some(MeasuredQuality {
        lossless: format.is_lossless(),
        format,
        bitrate_kbps: None,
        sample_rate: None,
        bit_depth: None,
        channels: None,
        size: None,
    })
}

fn is_current(
    source: &Path,
    destination: &Path,
    action: &Action,
    encodings: &BTreeMap<PathBuf, Encoding>,
) -> bool {
    match action {
        Action::Copy => is_fresh(source, destination, true),
        Action::Convert(encoding) => {
            encodings.get(destination) == Some(encoding) && is_fresh(source, destination, false)
        }
    }
}

fn is_fresh(source: &Path, destination: &Path, same_size: bool) -> bool {
    let (Ok(source), Ok(destination)) = (fs::metadata(source), fs::metadata(destination)) else {
        return false;
    };
    if destination.len() == 0 || (same_size && destination.len() != source.len()) {
        return false;
    }
    match (source.modified(), destination.modified()) {
        (Ok(source), Ok(destination)) => destination >= source,
        _ => false,
    }
}

fn sanitize(relative: &Path) -> Option<PathBuf> {
    let mut clean = PathBuf::new();
    for component in relative.components() {
        let Component::Normal(name) = component else {
            return None;
        };
        let name: String = name
            .to_string_lossy()
            .chars()
            .map(|character| {
                if character.is_control() || "<>:\"\\|?*".contains(character) {
                    '_'
                } else {
                    character
                }
            })
            .collect();
        let name = name.trim_end_matches([' ', '.']);
        clean.push(if name.is_empty() { "_" } else { name });
    }
    (!clean.as_os_str().is_empty()).then_some(clean)
}

pub fn transfer(transfer: &Transfer) -> Result<()> {
    let parent = transfer
        .destination
        .parent()
        .ok_or("destination has no folder")?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
    let stem = transfer
        .destination
        .file_stem()
        .ok_or("destination has no file name")?;
    let extension = transfer
        .destination
        .extension()
        .map(|extension| extension.to_string_lossy())
        .unwrap_or_default();
    let partial = parent.join(format!(".{}.{PARTIAL}.{extension}", stem.to_string_lossy()));
    let result = match &transfer.action {
        Action::Copy => copy(&transfer.source, &partial),
        Action::Convert(encoding) => Ffmpeg::default()
            .convert(&Convert {
                source: &transfer.source,
                destination: &partial,
                encoding,
                tags_in_stream: transfer.tags_in_stream,
            })
            .map_err(Error::from)
            .and_then(|()| {
                if transfer.cover {
                    copy_cover(&transfer.source, &partial)
                } else {
                    Ok(())
                }
            }),
    }
    .and_then(|()| Ok(fs::rename(&partial, &transfer.destination)?));
    if result.is_err() {
        fs::remove_file(&partial).ok();
    }
    result
}

fn copy_cover(source: &Path, destination: &Path) -> Result<()> {
    match muzik_tags::front_cover(source)? {
        Some((image, mime)) => muzik_tags::embed_cover(destination, &image, &mime)
            .map_err(|error| Error::Message(format!("cannot embed the cover: {error}"))),
        None => Ok(()),
    }
}

fn copy(source: &Path, destination: &Path) -> Result<()> {
    let mut reader = File::open(source)?;
    let mut writer = File::create(destination)?;
    io::copy(&mut reader, &mut writer)?;
    Ok(())
}

pub fn encodings(connection: &Connection, root: &Path) -> Result<BTreeMap<PathBuf, Encoding>> {
    Ok(sync_files::load(connection, root)?
        .into_iter()
        .filter_map(|(destination, text)| {
            serde_json::from_str(&text)
                .ok()
                .map(|encoding| (destination, encoding))
        })
        .collect())
}

pub fn record(connection: &Connection, transfer: &Transfer) -> Result<()> {
    let encoding = match &transfer.action {
        Action::Copy => None,
        Action::Convert(encoding) => Some(serde_json::to_string(encoding)?),
    };
    sync_files::save(connection, &transfer.destination, encoding.as_deref())?;
    Ok(())
}

pub fn run(
    transfers: &[Transfer],
    jobs: usize,
    done: &(dyn Fn(&Transfer, &Result<()>) + Sync),
) -> usize {
    parallel(transfers, jobs, |item| {
        let result = transfer(item);
        done(item, &result);
        result.is_err()
    })
    .into_iter()
    .filter(|failed| *failed)
    .count()
}

pub fn stale_files(root: &Path, planned: &BTreeSet<PathBuf>) -> io::Result<Vec<PathBuf>> {
    let mut stale = Vec::new();
    let mut folders = vec![root.to_path_buf()];
    while let Some(folder) = folders.pop() {
        for entry in fs::read_dir(&folder)? {
            let entry = entry?;
            let path = entry.path();
            let kind = entry.file_type()?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if kind.is_dir() {
                if !name.starts_with('.') {
                    folders.push(path);
                }
                continue;
            }
            let leftover = name.starts_with("._")
                || (name.starts_with('.') && name.contains(&format!(".{PARTIAL}.")));
            let media = !name.starts_with('.') && DeviceFile::from_path(&path).is_some();
            if leftover || (media && !planned.contains(&path)) {
                stale.push(path);
            }
        }
    }
    stale.sort();
    Ok(stale)
}

pub fn remove_empty_folders(root: &Path) -> io::Result<()> {
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() && !entry.file_name().to_string_lossy().starts_with('.') {
            let path = entry.path();
            remove_empty_folders(&path)?;
            if fs::read_dir(&path)?.next().is_none() {
                fs::remove_dir(&path)?;
            }
        }
    }
    Ok(())
}

#[cfg(unix)]
pub fn available_bytes(path: &Path) -> Option<u64> {
    rustix::fs::statvfs(path)
        .ok()
        .map(|stat| stat.f_bavail.saturating_mul(stat.f_frsize))
}

#[cfg(not(unix))]
pub fn available_bytes(_path: &Path) -> Option<u64> {
    None
}

fn parallel<T: Sync, R: Send>(
    items: &[T],
    jobs: usize,
    work: impl Fn(&T) -> R + Sync + Send,
) -> Vec<R> {
    match rayon::ThreadPoolBuilder::new().num_threads(jobs).build() {
        Ok(pool) => pool.install(|| items.par_iter().map(work).collect()),
        Err(_) => items.iter().map(work).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::DeviceFile;
    use std::path::Path;

    #[test]
    fn device_files_cover_the_media_extensions() {
        for extension in [
            "aac", "aif", "aiff", "ape", "dff", "dsf", "flac", "jpeg", "jpg", "m4a", "mp3", "mp4",
            "ogg", "opus", "png", "wav", "wma", "WAV",
        ] {
            let path = Path::new("track").with_extension(extension);
            assert!(DeviceFile::from_path(&path).is_some(), "{extension}");
        }
        for extension in ["wv", "mpc", "txt"] {
            let path = Path::new("track").with_extension(extension);
            assert_eq!(DeviceFile::from_path(&path), None, "{extension}");
        }
    }
}
