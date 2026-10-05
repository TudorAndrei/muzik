//! Copy library tracks to a device folder in formats that the device plays.

use crate::app_config;
use crate::config_choices::SyncPreset;
use crate::ffmpeg::{Convert, Ffmpeg};
use crate::paths;
use crate::quality::MeasuredQuality;
use serde_json::{json, Map, Value};
use std::collections::{BTreeSet, HashSet};
use std::fs::{self, File};
use std::io;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, PoisonError};

pub use crate::ffmpeg::Encoding;

const SECTION: &str = "sync";
const PARTIAL: &str = "muzik-part";
const MEDIA_EXTENSIONS: &[&str] = &[
    "aac", "aif", "aiff", "ape", "dff", "dsf", "flac", "jpeg", "jpg", "m4a", "mp3", "mp4", "ogg",
    "opus", "png", "wav", "wma",
];

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
    pub fn load(config: &Value, name: &str) -> Result<Self, String> {
        let targets = config.get(SECTION).and_then(Value::as_object);
        let Some(entry) = targets.and_then(|targets| targets.get(name)) else {
            let names = targets
                .map(|targets| targets.keys().cloned().collect::<Vec<_>>().join(", "))
                .unwrap_or_default();
            return Err(if names.is_empty() {
                format!("no sync target named {name}; add one with `muzik config set-sync-target`")
            } else {
                format!("no sync target named {name}; configured targets: {names}")
            });
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
                .parse()
                .map_err(|error: crate::ChoiceError| error.to_string())?,
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

    pub fn save(&self, config_path: &Path, name: &str) -> Result<(), String> {
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
        app_config::save_section_value(config_path, SECTION, name, Value::Object(entry))
    }

    fn validate(&self) -> Result<(), String> {
        let range = match self.preset {
            SyncPreset::EchoMini | SyncPreset::Mp3 => 32..=320,
            SyncPreset::Opus => 6..=512,
        };
        match self.bitrate {
            Some(bitrate) if !range.contains(&bitrate) => Err(format!(
                "bitrate for {} must be from {} to {} kbps",
                self.preset,
                range.start(),
                range.end()
            )),
            _ => Ok(()),
        }
    }

    pub fn action(&self, audio: &MeasuredQuality) -> Action {
        let codec = audio.format.as_str();
        match self.preset {
            SyncPreset::EchoMini => {
                let plays = lossless(codec)
                    || matches!(codec, "aac" | "mp3" | "vorbis" | "wmav1" | "wmav2");
                if !plays {
                    return Action::Convert(Encoding::Mp3 {
                        kbps: self.bitrate.unwrap_or(320),
                    });
                }
                if codec.starts_with("dsd_") {
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
                if codec == "mp3" {
                    Action::Copy
                } else {
                    Action::Convert(Encoding::Mp3 {
                        kbps: self.bitrate.unwrap_or(320),
                    })
                }
            }
            SyncPreset::Opus => {
                if matches!(codec, "aac" | "mp3" | "opus" | "vorbis") {
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
    jobs: usize,
    probe: &(dyn Fn(&Path) -> Result<Option<MeasuredQuality>, String> + Sync),
) -> Plan {
    let mut steps = parallel(tracks, jobs, |source| {
        plan_track(target, directory, source, probe)
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
    probe: &(dyn Fn(&Path) -> Result<Option<MeasuredQuality>, String> + Sync),
) -> Step {
    if source.strip_prefix(directory).is_err() {
        return Step::Outside(source.to_path_buf());
    }
    if let Some(action) = guess(source).map(|audio| target.action(&audio)) {
        if let Some(destination) = target.destination(directory, source, &action) {
            if is_fresh(source, &destination, action == Action::Copy) {
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
    if is_fresh(source, &destination, action == Action::Copy) {
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
        tags_in_stream: matches!(audio.format.as_str(), "opus" | "vorbis"),
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
    let extension = source.extension()?.to_str()?.to_ascii_lowercase();
    let format = match extension.as_str() {
        "flac" => "flac",
        "mp3" => "mp3",
        "opus" => "opus",
        "wav" => "pcm_s16le",
        "aif" | "aiff" => "pcm_s16be",
        "ape" => "ape",
        "dsf" => "dsd_lsbf_planar",
        "dff" => "dsd_msbf",
        "wma" => "wmav2",
        _ => return None,
    };
    Some(MeasuredQuality {
        format: format.to_owned(),
        lossless: lossless(format),
        bitrate_kbps: None,
        sample_rate: None,
        bit_depth: None,
        channels: None,
        size: None,
    })
}

fn lossless(codec: &str) -> bool {
    matches!(codec, "flac" | "alac" | "ape" | "wavpack" | "tta")
        || codec.starts_with("pcm_")
        || codec.starts_with("dsd_")
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

pub fn transfer(transfer: &Transfer) -> Result<(), String> {
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
            .map_err(|error| error.to_string())
            .and_then(|()| {
                if transfer.cover {
                    copy_cover(&transfer.source, &partial)
                } else {
                    Ok(())
                }
            }),
    }
    .and_then(|()| fs::rename(&partial, &transfer.destination).map_err(|error| error.to_string()));
    if result.is_err() {
        fs::remove_file(&partial).ok();
    }
    result
}

fn copy_cover(source: &Path, destination: &Path) -> Result<(), String> {
    let cover = muzik_tags::front_cover(source).map_err(|error| error.to_string())?;
    match cover {
        Some((image, mime)) => muzik_tags::embed_cover(destination, &image, &mime)
            .map_err(|error| format!("cannot embed the cover: {error}")),
        None => Ok(()),
    }
}

fn copy(source: &Path, destination: &Path) -> Result<(), String> {
    let mut reader = File::open(source).map_err(|error| error.to_string())?;
    let mut writer = File::create(destination).map_err(|error| error.to_string())?;
    io::copy(&mut reader, &mut writer).map_err(|error| error.to_string())?;
    Ok(())
}

pub fn run(
    transfers: &[Transfer],
    jobs: usize,
    done: &(dyn Fn(&Transfer, &Result<(), String>) + Sync),
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
            let media = !name.starts_with('.')
                && path
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .is_some_and(|extension| {
                        MEDIA_EXTENSIONS.contains(&extension.to_ascii_lowercase().as_str())
                    });
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

fn parallel<T: Sync, R: Send>(items: &[T], jobs: usize, work: impl Fn(&T) -> R + Sync) -> Vec<R> {
    let next = AtomicUsize::new(0);
    let results = Mutex::new(Vec::with_capacity(items.len()));
    let workers = if jobs == 0 {
        std::thread::available_parallelism().map_or(4, usize::from)
    } else {
        jobs
    };
    std::thread::scope(|scope| {
        for _ in 0..workers.min(items.len()) {
            scope.spawn(|| loop {
                let index = next.fetch_add(1, Ordering::Relaxed);
                let Some(item) = items.get(index) else {
                    break;
                };
                let result = work(item);
                results
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push((index, result));
            });
        }
    });
    let mut results = results.into_inner().unwrap_or_else(PoisonError::into_inner);
    results.sort_by_key(|(index, _)| *index);
    results.into_iter().map(|(_, result)| result).collect()
}
