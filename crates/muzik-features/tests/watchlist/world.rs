use crate::operations::FakeOperations;
use cucumber::World;
use muzik_core::{JobEvent, QualityPolicy};
use muzik_store::watchlist::jobs::{JobError, JobOptions, PendingItem};
use muzik_store::watchlist::{
    CheckedWrite, LocalCheck, LocalChecks, Playlist, ReconcileOptions, Repository, Stage, Watchlist,
};
use serde_json::Value;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use tempfile::TempDir;

pub struct CheckState {
    pub generation: Arc<AtomicU64>,
    pub checks: LocalChecks,
    pub check: Option<LocalCheck>,
}

impl fmt::Debug for CheckState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CheckState")
            .field("generation", &self.generation)
            .field("prepared", &self.check.is_some())
            .finish_non_exhaustive()
    }
}

#[derive(Debug, World)]
#[world(init = Self::new)]
pub struct WatchlistWorld {
    pub dir: TempDir,
    pub legacy: bool,
    pub config: Option<PathBuf>,
    pub no_organize: bool,
    pub dry_run: bool,
    pub only: Option<String>,
    pub operations: FakeOperations,
    pub events: Vec<JobEvent>,
    pub job: Option<Result<Value, JobError>>,
    pub pending: Option<Vec<PendingItem>>,
    pub checked: Option<Value>,
    pub shown: Option<Value>,
    pub loaded: Option<Watchlist>,
    pub before: Option<(i64, Watchlist)>,
    pub source: Option<Result<Playlist, muzik_store::Error>>,
    pub changed: Option<bool>,
    pub imported: Option<bool>,
    pub read: Option<(Watchlist, i64)>,
    pub write: Option<Result<CheckedWrite, muzik_store::Error>>,
    pub local: CheckState,
    pub prepared: Option<Value>,
    pub published: Option<Result<Value, String>>,
    pub outside_edits: u32,
    pub legacy_content: Option<Vec<u8>>,
    pub decision: Option<Stage>,
}

impl WatchlistWorld {
    fn new() -> anyhow::Result<Self> {
        let dir = tempfile::tempdir()?;
        for folder in ["downloads", "splits", "cache"] {
            std::fs::create_dir(dir.path().join(folder))?;
        }
        let generation = Arc::new(AtomicU64::new(0));
        Ok(Self {
            dir,
            legacy: false,
            config: None,
            no_organize: false,
            dry_run: false,
            only: None,
            operations: FakeOperations::default(),
            events: Vec::new(),
            job: None,
            pending: None,
            checked: None,
            shown: None,
            loaded: None,
            before: None,
            source: None,
            changed: None,
            imported: None,
            read: None,
            write: None,
            local: CheckState {
                checks: LocalChecks::new(Arc::clone(&generation)),
                generation,
                check: None,
            },
            prepared: None,
            published: None,
            outside_edits: 0,
            legacy_content: None,
            decision: None,
        })
    }

    pub fn root(&self) -> &Path {
        self.dir.path()
    }

    pub fn downloads(&self) -> PathBuf {
        self.root().join("downloads")
    }

    pub fn splits(&self) -> PathBuf {
        self.root().join("splits")
    }

    pub fn cache(&self) -> PathBuf {
        self.root().join("cache")
    }

    pub fn legacy_file(&self) -> PathBuf {
        self.root().join("watchlist.json")
    }

    pub fn database(&self) -> PathBuf {
        self.root().join("config").join("muzik.db")
    }

    pub fn repository(&self) -> Repository {
        let repository = Repository::new(self.database());
        if self.legacy {
            repository.with_legacy(self.legacy_file())
        } else {
            repository
        }
    }

    pub fn snapshot(&mut self) -> anyhow::Result<()> {
        let repository = self.repository();
        self.before = Some((repository.revision()?, repository.load()?));
        Ok(())
    }

    pub fn with_options<T>(&self, work: impl FnOnce(ReconcileOptions<'_>) -> T) -> T {
        let output = self.downloads();
        let splits = self.splits();
        let cache = self.cache();
        work(ReconcileOptions {
            output: &output,
            splits: &splits,
            cache: &cache,
            config: self.config.as_deref(),
            no_organize: self.no_organize,
            no_split: false,
            quality_policy: QualityPolicy::Off,
        })
    }

    pub fn with_job_options<T>(&self, work: impl FnOnce(JobOptions<'_>) -> T) -> T {
        self.with_options(|reconcile| {
            work(JobOptions {
                reconcile,
                output: reconcile.output,
                cache: reconcile.cache,
                dry_run: self.dry_run,
                playlist_id: self.only.as_deref(),
            })
        })
    }

    pub fn document(&self, which: &str) -> anyhow::Result<Value> {
        let document = match which {
            "saved" => Some(self.repository().load()?.to_value()?),
            "loaded" => self.loaded.as_ref().map(Watchlist::to_value).transpose()?,
            "checked" => self.checked.clone(),
            "shown" => self.shown.clone(),
            "prepared" => self.prepared.clone(),
            "published" => match &self.published {
                Some(Ok(document)) => Some(document.clone()),
                Some(Err(error)) => anyhow::bail!("the local check failed: {error}"),
                None => None,
            },
            "refreshed" => match &self.job {
                Some(Ok(result)) => result.get("watchlist").cloned(),
                Some(Err(error)) => anyhow::bail!("the job failed: {error}"),
                None => None,
            },
            other => anyhow::bail!("there is no {other} watchlist"),
        };
        document.ok_or_else(|| anyhow::anyhow!("there is no {which} watchlist yet"))
    }
}
