use cucumber::World;
use cucumber::gherkin::Step;
use muzik_core::SyncPreset;
use muzik_core::audio::Codec;
use muzik_media::quality::MeasuredQuality;
use muzik_store::Connection;
use muzik_sync::{Action, Error, Plan, Prepared, Target, Transfer};
use std::collections::BTreeSet;
use std::fmt;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

pub struct Preparation(pub Prepared);

impl fmt::Debug for Preparation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Prepared")
            .field("plan", self.0.plan())
            .field("delete", &self.0.delete())
            .field("delete_blocked", &self.0.delete_blocked())
            .field("stale", &self.0.stale())
            .field("freed", &self.0.freed())
            .finish_non_exhaustive()
    }
}

#[derive(Debug)]
pub struct Applied {
    pub total: usize,
    pub failed: usize,
    pub unrecorded: usize,
    pub calls: usize,
    pub transfer_failures: usize,
    pub record_failures: usize,
}

#[derive(Debug, World)]
#[world(init = Self::new)]
pub struct SyncWorld {
    pub dir: TempDir,
    pub target: Target,
    pub tracks: Vec<PathBuf>,
    pub covers: Vec<PathBuf>,
    pub unreadable: Vec<String>,
    pub records: Option<Connection>,
    pub records_fail: bool,
    pub plan: Option<Plan>,
    pub prepared: Option<Preparation>,
    pub destinations: Vec<PathBuf>,
    pub applied: Option<Result<Applied, Error>>,
    pub planned: BTreeSet<PathBuf>,
    pub stale: Option<Vec<PathBuf>>,
    pub action: Option<Action>,
    pub transfer: Option<Transfer>,
    pub transfer_result: Option<Result<(), Error>>,
    pub saved: Option<Target>,
    pub save_result: Option<Result<(), Error>>,
    pub loaded: Option<Result<Target, Error>>,
    pub cover: PathBuf,
    pub selected_tracks: Vec<PathBuf>,
    pub selected_covers: Vec<PathBuf>,
}

impl SyncWorld {
    fn new() -> anyhow::Result<Self> {
        let dir = tempfile::tempdir()?;
        let library = dir.path().join("library");
        let card = dir.path().join("card");
        std::fs::create_dir_all(&library)?;
        std::fs::create_dir_all(&card)?;
        Ok(Self {
            target: Target {
                path: card,
                preset: SyncPreset::EchoMini,
                bitrate: None,
                covers: true,
            },
            cover: library.join("cover.jpg"),
            dir,
            tracks: Vec::new(),
            covers: Vec::new(),
            unreadable: Vec::new(),
            records: Some(muzik_store::db::open_in_memory()?),
            records_fail: false,
            plan: None,
            prepared: None,
            destinations: Vec::new(),
            applied: None,
            planned: BTreeSet::new(),
            stale: None,
            action: None,
            transfer: None,
            transfer_result: None,
            saved: None,
            save_result: None,
            loaded: None,
            selected_tracks: Vec::new(),
            selected_covers: Vec::new(),
        })
    }

    pub fn root(&self) -> &Path {
        self.dir.path()
    }

    pub fn library(&self) -> PathBuf {
        self.root().join("library")
    }

    pub fn card(&self) -> PathBuf {
        self.root().join("card")
    }

    pub fn unplugged(&self) -> PathBuf {
        self.root().join("unplugged")
    }

    pub fn config(&self) -> PathBuf {
        self.root().join("config.yaml")
    }

    pub fn records(&self) -> anyhow::Result<&Connection> {
        self.records
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("the sync records are closed"))
    }

    pub fn current_plan(&self) -> anyhow::Result<&Plan> {
        if let Some(plan) = &self.plan {
            return Ok(plan);
        }
        if let Some(prepared) = &self.prepared {
            return Ok(prepared.0.plan());
        }
        anyhow::bail!("muzik did not plan the sync")
    }

    pub fn preparation(&self) -> anyhow::Result<&Prepared> {
        self.prepared
            .as_ref()
            .map(|prepared| &prepared.0)
            .ok_or_else(|| anyhow::anyhow!("muzik did not prepare the sync"))
    }

    pub fn report(&self) -> anyhow::Result<&Applied> {
        match &self.applied {
            Some(Ok(applied)) => Ok(applied),
            Some(Err(error)) => anyhow::bail!("the sync failed: {error}"),
            None => anyhow::bail!("muzik did not apply the sync"),
        }
    }
}

pub fn probe(unreadable: &[String], path: &Path) -> Option<MeasuredQuality> {
    let name = path.file_name()?.to_string_lossy();
    if unreadable.iter().any(|unreadable| *unreadable == name) {
        return None;
    }
    let format = path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default();
    Some(audio(format, 44_100, Some(16)))
}

pub fn audio(format: &str, sample_rate: u32, bit_depth: Option<u32>) -> MeasuredQuality {
    MeasuredQuality {
        format: Codec::from_ffprobe(format),
        lossless: false,
        bitrate_kbps: Some(128),
        sample_rate: Some(sample_rate),
        bit_depth,
        channels: Some(2),
        size: Some(1_000),
    }
}

pub fn rows(step: &Step) -> anyhow::Result<Vec<String>> {
    let table = step
        .table
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("the step has no table"))?;
    table
        .rows
        .iter()
        .map(|row| {
            row.first()
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("a table row is empty"))
        })
        .collect()
}
