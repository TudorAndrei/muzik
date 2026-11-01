use muzik_core::{ReleaseCandidate, TrackCandidate};
use muzik_import::plan::ReleaseProvider;
use muzik_metadata::{ReleaseArtistIds, ReleaseSearch, ReleaseSearchHit};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

#[derive(Debug, Default)]
pub struct FakeCatalog {
    pub releases: Vec<ReleaseCandidate>,
    pub recordings: Vec<TrackCandidate>,
    pub artist_ids: Option<ReleaseArtistIds>,
    pub cancel_on_search: Option<Arc<AtomicBool>>,
    pub requests: AtomicUsize,
}

impl FakeCatalog {
    fn request(&self) {
        self.requests.fetch_add(1, Ordering::SeqCst);
    }

    pub fn requested(&self) -> bool {
        self.requests.load(Ordering::SeqCst) > 0
    }

    pub fn last_release_mut(&mut self) -> anyhow::Result<&mut ReleaseCandidate> {
        self.releases
            .last_mut()
            .ok_or_else(|| anyhow::anyhow!("MusicBrainz has no release"))
    }
}

impl ReleaseProvider for FakeCatalog {
    fn search_releases(
        &self,
        criteria: &ReleaseSearch,
        _limit: u8,
    ) -> Result<Vec<ReleaseSearchHit>, muzik_metadata::Error> {
        self.request();
        if let Some(cancelled) = &self.cancel_on_search {
            cancelled.store(true, Ordering::SeqCst);
            return Ok(Vec::new());
        }
        let hits: Vec<ReleaseSearchHit> = self
            .releases
            .iter()
            .filter(|release| {
                criteria.release == release.title
                    && criteria.artist.as_deref() == Some(release.artist.as_str())
                    && criteria
                        .tracks
                        .is_none_or(|tracks| usize::try_from(tracks) == Ok(release.tracks.len()))
            })
            .map(|release| ReleaseSearchHit {
                id: release.id.clone(),
                title: release.title.clone(),
                artist: release.artist.clone(),
                score: Some(100),
            })
            .collect();
        if hits.is_empty() {
            return Err(muzik_metadata::Error::EmptyReleaseTitle);
        }
        Ok(hits)
    }

    fn lookup_release(&self, id: &str) -> Result<ReleaseCandidate, muzik_metadata::Error> {
        self.request();
        self.releases
            .iter()
            .find(|release| release.id.0 == id)
            .cloned()
            .ok_or(muzik_metadata::Error::EmptyReleaseTitle)
    }

    fn lookup_recording(&self, id: &str) -> Result<TrackCandidate, muzik_metadata::Error> {
        self.request();
        self.recordings
            .iter()
            .find(|recording| {
                recording
                    .recording_id
                    .as_ref()
                    .is_some_and(|recording| recording.0 == id)
            })
            .cloned()
            .ok_or(muzik_metadata::Error::EmptyReleaseTitle)
    }

    fn lookup_release_artist_ids(
        &self,
        _id: &str,
    ) -> Result<ReleaseArtistIds, muzik_metadata::Error> {
        self.request();
        Ok(self.artist_ids.clone().unwrap_or_default())
    }
}
