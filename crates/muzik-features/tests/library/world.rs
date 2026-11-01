use crate::support::fixture;
use cucumber::World;
use muzik_library::{Album, Error, Item, Library};
use muzik_tags::TagsError;
use std::fmt;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

#[derive(World)]
#[world(init = Self::new)]
pub struct LibraryWorld {
    pub dir: TempDir,
    pub database: PathBuf,
    pub library: Option<Library>,
    pub items: Vec<Item>,
    pub albums: Vec<Album>,
    pub album_id: Option<i64>,
    pub kept_item: Option<(i64, PathBuf)>,
    pub written: Option<Result<(), Error>>,
    pub pruned: Option<Result<usize, Error>>,
    pub image: Vec<u8>,
    pub cover: Option<Result<PathBuf, TagsError>>,
}

impl fmt::Debug for LibraryWorld {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LibraryWorld")
            .field("dir", &self.dir)
            .field("database", &self.database)
            .field("items", &self.items)
            .field("albums", &self.albums)
            .field("album_id", &self.album_id)
            .field("written", &self.written)
            .field("pruned", &self.pruned)
            .field("cover", &self.cover)
            .finish_non_exhaustive()
    }
}

impl LibraryWorld {
    fn new() -> anyhow::Result<Self> {
        let dir = tempfile::tempdir()?;
        std::fs::create_dir(dir.path().join("album"))?;
        Ok(Self {
            database: dir.path().join("library.db"),
            dir,
            library: None,
            items: Vec::new(),
            albums: Vec::new(),
            album_id: None,
            kept_item: None,
            written: None,
            pruned: None,
            image: Vec::new(),
            cover: None,
        })
    }

    pub fn sample() -> PathBuf {
        fixture("muzik-library/tests/fixtures/library.db")
    }

    pub fn root(&self) -> &Path {
        self.dir.path()
    }

    pub fn album_dir(&self) -> PathBuf {
        self.root().join("album")
    }

    pub fn library(&self) -> anyhow::Result<&Library> {
        self.library
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("no library is open"))
    }

    pub fn library_mut(&mut self) -> anyhow::Result<&mut Library> {
        self.library
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("no library is open"))
    }

    pub fn album_id(&self) -> anyhow::Result<i64> {
        self.album_id
            .ok_or_else(|| anyhow::anyhow!("no album was added"))
    }

    pub fn item(&self, id: i64) -> anyhow::Result<&Item> {
        self.items
            .iter()
            .find(|item| item.id == id)
            .ok_or_else(|| anyhow::anyhow!("item {id} is not in {:?}", self.items))
    }

    pub fn album(&self, id: i64) -> anyhow::Result<&Album> {
        self.albums
            .iter()
            .find(|album| album.id == id)
            .ok_or_else(|| anyhow::anyhow!("album {id} is not in {:?}", self.albums))
    }
}

pub fn ids(text: &str) -> anyhow::Result<Vec<i64>> {
    if text == "none" {
        return Ok(Vec::new());
    }
    text.split(',').map(|id| Ok(id.trim().parse()?)).collect()
}
