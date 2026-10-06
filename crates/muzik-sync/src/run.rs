use crate::{
    available_bytes, encodings, plan, record, remove_empty_folders, run as transfer_all,
    stale_files, Plan, Target, Transfer,
};
use muzik_library::{path_from_sql, Item, Library};
use muzik_media::quality::MeasuredQuality;
use rusqlite::Connection;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, PoisonError};

pub struct Selection {
    pub tracks: Vec<PathBuf>,
    pub covers: Vec<PathBuf>,
}

#[derive(Clone, Copy)]
pub struct Options {
    pub delete: bool,
    pub jobs: usize,
}

pub struct Prepared {
    pub plan: Plan,
    pub delete: bool,
    pub delete_blocked: bool,
    pub stale: Vec<PathBuf>,
    pub freed: u64,
    pub needed: u64,
    pub available: Option<u64>,
}

pub struct Done<'a> {
    pub index: usize,
    pub total: usize,
    pub transfer: &'a Transfer,
    pub result: &'a Result<(), String>,
    pub record_error: Option<String>,
}

pub struct Report {
    pub written: usize,
    pub failed: usize,
}

impl Prepared {
    pub fn space(&self) -> Option<u64> {
        self.available
            .map(|available| available.saturating_add(self.freed))
    }

    pub fn fits(&self) -> bool {
        self.space().is_none_or(|space| self.needed <= space)
    }
}

pub fn select(
    library: &Library,
    directory: &Path,
    query: &str,
    covers: bool,
) -> Result<Selection, String> {
    let items = library
        .query_items(query)
        .map_err(|error| error.to_string())?;
    let tracks: Vec<PathBuf> = items
        .iter()
        .filter_map(|item| item.field("path").and_then(path_from_sql))
        .map(|path| absolute(directory, path))
        .collect();
    let album_ids: BTreeSet<i64> = if covers {
        items.iter().filter_map(Item::album_id).collect()
    } else {
        BTreeSet::new()
    };
    let mut cover_files = Vec::new();
    for id in album_ids {
        let album = library.album(id).map_err(|error| error.to_string())?;
        if let Some(path) = album
            .as_ref()
            .and_then(|album| album.field("artpath"))
            .and_then(path_from_sql)
            .map(|path| absolute(directory, path))
            .filter(|path| path.is_file())
        {
            cover_files.push(path);
        }
    }
    Ok(Selection {
        tracks,
        covers: cover_files,
    })
}

pub fn prepare(
    target: &Target,
    directory: &Path,
    selection: &Selection,
    connection: &Connection,
    options: Options,
    probe: &(dyn Fn(&Path) -> Result<Option<MeasuredQuality>, String> + Sync),
) -> Result<Prepared, String> {
    let known = encodings(connection, &target.path)?;
    let plan = plan(
        target,
        directory,
        &selection.tracks,
        &selection.covers,
        &known,
        options.jobs,
        probe,
    );
    let delete = options.delete && plan.unreadable.is_empty();
    let stale = if delete {
        stale_files(&target.path, &plan.planned).map_err(|error| error.to_string())?
    } else {
        Vec::new()
    };
    let freed: u64 = stale
        .iter()
        .filter_map(|path| fs::metadata(path).ok())
        .map(|meta| meta.len())
        .sum();
    let needed = plan.bytes_needed();
    Ok(Prepared {
        plan,
        delete,
        delete_blocked: options.delete && !delete,
        stale,
        freed,
        needed,
        available: available_bytes(&target.path),
    })
}

pub fn apply(
    prepared: &Prepared,
    target: &Target,
    connection: Connection,
    jobs: usize,
    done: &(dyn Fn(Done<'_>) + Sync),
) -> Result<Report, String> {
    for path in &prepared.stale {
        fs::remove_file(path).map_err(|error| format!("{}: {error}", path.display()))?;
    }
    if prepared.delete {
        remove_empty_folders(&target.path).map_err(|error| error.to_string())?;
    }
    let total = prepared.plan.pending.len();
    let count = AtomicUsize::new(0);
    let connection = Mutex::new(connection);
    let finished = |transfer: &Transfer, result: &Result<(), String>| {
        let index = count.fetch_add(1, Ordering::Relaxed) + 1;
        let record_error = match result {
            Ok(()) => record(
                &connection.lock().unwrap_or_else(PoisonError::into_inner),
                transfer,
            )
            .err(),
            Err(_) => None,
        };
        done(Done {
            index,
            total,
            transfer,
            result,
            record_error,
        });
    };
    let failed = transfer_all(&prepared.plan.pending, jobs, &finished);
    Ok(Report {
        written: total,
        failed,
    })
}

fn absolute(directory: &Path, path: PathBuf) -> PathBuf {
    if path.is_absolute() {
        path
    } else {
        directory.join(path)
    }
}
