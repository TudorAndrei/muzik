use crate::catalog::FakeCatalog;
use crate::support;
use cucumber::World;
use muzik_core::BeetsConfig;
use muzik_import::apply::{ApplyError, ApplyResult};
use muzik_import::files::Placement;
use muzik_import::history::IncrementalHistory;
use muzik_import::mbid_fill::{AlbumMatch, MissingAlbum};
use muzik_import::plan::ImportPlan;
use muzik_import::sync::SyncResult;
use muzik_library::{Item, Library};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use tempfile::TempDir;

pub struct Import {
    pub outcome: Result<ApplyResult, ApplyError>,
    pub items_before: usize,
    pub albums_before: usize,
    pub bytes: Option<Vec<u8>>,
}

#[derive(World)]
#[world(init = Self::new)]
pub struct ImportWorld {
    pub dir: TempDir,
    pub incoming: PathBuf,
    pub root: PathBuf,
    pub database: PathBuf,
    pub statefile: PathBuf,
    pub sources: Vec<PathBuf>,
    pub copy: Option<PathBuf>,
    pub config: BeetsConfig,
    pub catalog: FakeCatalog,
    pub library: Option<Library>,
    pub placement: Option<Placement>,
    pub dry_run: bool,
    pub singleton_path: Option<String>,
    pub default_path: Option<String>,
    pub autotag: bool,
    pub history: Option<IncrementalHistory>,
    pub skip_later: bool,
    pub cancelled: Arc<AtomicBool>,
    pub plan: Option<Result<ImportPlan, muzik_import::Error>>,
    pub imports: Vec<Import>,
    pub old_album: Option<i64>,
    pub old_item: Option<i64>,
    pub song: Option<i64>,
    pub audio: Option<PathBuf>,
    pub synced: Option<SyncResult>,
    pub album_files: Vec<(PathBuf, String)>,
    pub missing: Vec<MissingAlbum>,
    pub found: Option<AlbumMatch>,
}

impl fmt::Debug for ImportWorld {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ImportWorld")
            .field("dir", &self.dir)
            .field("sources", &self.sources)
            .field("catalog", &self.catalog)
            .field("plan", &self.plan)
            .field("synced", &self.synced)
            .finish_non_exhaustive()
    }
}

impl ImportWorld {
    fn new() -> anyhow::Result<Self> {
        let dir = tempfile::tempdir()?;
        let incoming = dir.path().join("incoming");
        std::fs::create_dir(&incoming)?;
        let root = dir.path().join("music");
        std::fs::create_dir(&root)?;
        let database = dir.path().join("library.db");
        std::fs::copy(
            support::fixture("muzik-library/tests/fixtures/library.db"),
            &database,
        )?;
        Ok(Self {
            statefile: dir.path().join("state.pickle"),
            incoming,
            root,
            database,
            dir,
            sources: Vec::new(),
            copy: None,
            config: BeetsConfig::from_layers("", serde_json::json!({}))?,
            catalog: FakeCatalog::default(),
            library: None,
            placement: None,
            dry_run: false,
            singleton_path: None,
            default_path: None,
            autotag: true,
            history: None,
            skip_later: false,
            cancelled: Arc::new(AtomicBool::new(false)),
            plan: None,
            imports: Vec::new(),
            old_album: None,
            old_item: None,
            song: None,
            audio: None,
            synced: None,
            album_files: Vec::new(),
            missing: Vec::new(),
            found: None,
        })
    }

    pub fn open_library(&mut self) -> anyhow::Result<()> {
        if self.library.is_none() {
            self.library = Some(Library::open_read_write(&self.database)?);
        }
        Ok(())
    }

    pub fn library(&mut self) -> anyhow::Result<&mut Library> {
        self.open_library()?;
        self.library
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("the library is not open"))
    }

    pub fn source(&self) -> anyhow::Result<&Path> {
        self.sources
            .last()
            .map(PathBuf::as_path)
            .ok_or_else(|| anyhow::anyhow!("there is no incoming song"))
    }

    pub fn current_plan(&self) -> anyhow::Result<&ImportPlan> {
        match &self.plan {
            Some(Ok(plan)) => Ok(plan),
            Some(Err(error)) => anyhow::bail!("planning failed: {error}"),
            None => anyhow::bail!("muzik did not plan the import"),
        }
    }

    pub fn current_plan_mut(&mut self) -> anyhow::Result<&mut ImportPlan> {
        match &mut self.plan {
            Some(Ok(plan)) => Ok(plan),
            Some(Err(error)) => anyhow::bail!("planning failed: {error}"),
            None => anyhow::bail!("muzik did not plan the import"),
        }
    }

    pub fn last_import(&self) -> anyhow::Result<&Import> {
        self.imports
            .last()
            .ok_or_else(|| anyhow::anyhow!("muzik did not import"))
    }

    pub fn result(&self) -> anyhow::Result<&ApplyResult> {
        match &self.last_import()?.outcome {
            Ok(result) => Ok(result),
            Err(error) => anyhow::bail!("the import failed: {error}"),
        }
    }

    pub fn first_result(&self) -> anyhow::Result<&ApplyResult> {
        match self.imports.first().map(|import| &import.outcome) {
            Some(Ok(result)) => Ok(result),
            Some(Err(error)) => anyhow::bail!("the first import failed: {error}"),
            None => anyhow::bail!("muzik did not import"),
        }
    }

    pub fn destination(&self) -> anyhow::Result<&Path> {
        self.result()?
            .destinations
            .first()
            .map(PathBuf::as_path)
            .ok_or_else(|| anyhow::anyhow!("the import has no destination"))
    }

    pub fn imported_item(&mut self) -> anyhow::Result<Item> {
        let id = *self
            .result()?
            .item_ids
            .first()
            .ok_or_else(|| anyhow::anyhow!("the import added no songs"))?;
        self.library()?
            .item(id)?
            .ok_or_else(|| anyhow::anyhow!("song {id} is not in the library"))
    }
}
