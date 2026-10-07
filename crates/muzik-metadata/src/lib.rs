//! Blocking MusicBrainz release search and lookup.

use std::num::NonZeroU32;
use std::sync::OnceLock;
use std::thread;

use backon::{BlockingRetryable, ExponentialBuilder};
use governor::clock::{Clock, DefaultClock};
use governor::{DefaultDirectRateLimiter, Quota, RateLimiter};
use musicbrainz_rs::api_bindium::ureq;
use musicbrainz_rs::api_bindium::ApiRequestError;
use musicbrainz_rs::client::MusicBrainzClient;
use musicbrainz_rs::entity::artist_credit::ArtistCredit;
use musicbrainz_rs::entity::recording::Recording;
use musicbrainz_rs::entity::release::Release;
use musicbrainz_rs::prelude::*;
use muzik_core::{RecordingId, ReleaseCandidate, ReleaseId, TrackCandidate};

const VARIOUS_ARTISTS_ID: &str = "89ad4ac3-39f7-470e-963a-56509c546377";
static REQUEST_LIMITER: OnceLock<DefaultDirectRateLimiter> = OnceLock::new();

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("release title is empty")]
    EmptyReleaseTitle,
    #[error("MusicBrainz request failed: {0}")]
    Api(#[from] Box<musicbrainz_rs::ApiEndpointError>),
}

/// Search fields supported by the beets MusicBrainz album search.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReleaseSearch {
    pub release: String,
    pub artist: Option<String>,
    pub various_artists: bool,
    pub barcode: Option<String>,
    pub catalog_number: Option<String>,
    pub country: Option<String>,
    pub label: Option<String>,
    pub media: Option<String>,
    pub year: Option<String>,
    pub tracks: Option<u32>,
    pub alias: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReleaseSearchHit {
    pub id: ReleaseId,
    pub title: String,
    pub artist: String,
    pub score: Option<u8>,
}

/// Beets match settings that affect MusicBrainz release mapping.
#[derive(Clone, Debug)]
pub struct ReleaseOptions {
    pub preferred_countries: Vec<String>,
    pub ignored_media: Vec<String>,
    pub ignore_video_tracks: bool,
    pub various_artists_name: String,
}

impl Default for ReleaseOptions {
    fn default() -> Self {
        Self {
            preferred_countries: Vec::new(),
            ignored_media: Vec::new(),
            ignore_video_tracks: true,
            various_artists_name: "Various Artists".to_string(),
        }
    }
}

/// A blocking client. The global limiter applies to all instances and retries.
pub struct MetadataClient {
    client: MusicBrainzClient,
    options: ReleaseOptions,
}

impl MetadataClient {
    #[must_use]
    pub fn new(user_agent: &str) -> Self {
        Self {
            client: MusicBrainzClient::new(user_agent),
            options: ReleaseOptions::default(),
        }
    }

    #[must_use]
    pub fn with_options(user_agent: &str, options: ReleaseOptions) -> Self {
        Self {
            client: MusicBrainzClient::new(user_agent),
            options,
        }
    }

    pub fn search_releases(
        &self,
        criteria: &ReleaseSearch,
        limit: u8,
    ) -> Result<Vec<ReleaseSearchHit>, Error> {
        let query = criteria.query()?;
        tracing::debug!(query, "search MusicBrainz releases");
        let result = request_with_retry(|| {
            let mut search = Release::search(query.clone());
            search.limit(limit.clamp(1, 100));
            search.execute_with_client(&self.client).map_err(Box::new)
        })?;
        Ok(result
            .entities
            .iter()
            .map(|release| ReleaseSearchHit {
                id: ReleaseId(release.id.clone()),
                title: release.title.clone(),
                artist: artist_name(release.artist_credit.as_deref().unwrap_or_default()),
                score: release.score,
            })
            .collect())
    }

    pub fn lookup_release(&self, id: &str) -> Result<ReleaseCandidate, Error> {
        tracing::debug!(id, "look up MusicBrainz release");
        let release = request_with_retry(|| {
            Release::fetch()
                .id(id)
                .with_media()
                .with_recordings()
                .with_artist_credits()
                .with_artists()
                .with_labels()
                .with_release_groups()
                .execute_with_client(&self.client)
                .map_err(Box::new)
        })?;
        Ok(release_candidate_with_options(&release, &self.options))
    }

    pub fn lookup_recording(&self, id: &str) -> Result<TrackCandidate, Error> {
        tracing::debug!(id, "look up MusicBrainz recording");
        let recording = request_with_retry(|| {
            Recording::fetch()
                .id(id)
                .with_artists()
                .execute_with_client(&self.client)
                .map_err(Box::new)
        })?;
        Ok(recording_candidate(&recording))
    }
}

#[must_use]
pub fn recording_candidate(recording: &Recording) -> TrackCandidate {
    TrackCandidate {
        recording_id: Some(RecordingId(recording.id.clone())),
        release_track_id: None,
        title: recording.title.clone(),
        artist: artist_name(recording.artist_credit.as_deref().unwrap_or_default()),
        length_seconds: recording.length.map(|length| f64::from(length) / 1000.0),
        index: 0,
        medium: 0,
        medium_index: 0,
    }
}

impl ReleaseSearch {
    /// Build the Lucene fields used by the beets MusicBrainz album search.
    pub fn query(&self) -> Result<String, Error> {
        if self.release.trim().is_empty() {
            return Err(Error::EmptyReleaseTitle);
        }
        let mut terms = Vec::new();
        add_term(&mut terms, "release", &self.release);
        if self.various_artists {
            add_term(&mut terms, "arid", VARIOUS_ARTISTS_ID);
        } else if let Some(artist) = &self.artist {
            add_term(&mut terms, "artist", artist);
        }
        for (field, value) in [
            ("barcode", self.barcode.as_deref()),
            ("catno", self.catalog_number.as_deref()),
            ("country", self.country.as_deref()),
            ("label", self.label.as_deref()),
            ("format", self.media.as_deref()),
            ("date", self.year.as_deref()),
            ("alias", self.alias.as_deref()),
        ] {
            if let Some(value) = value {
                add_term(&mut terms, field, value);
            }
        }
        if let Some(tracks) = self.tracks {
            add_term(&mut terms, "tracks", &tracks.to_string());
        }
        Ok(terms.join(" "))
    }
}

fn add_term(terms: &mut Vec<String>, field: &str, value: &str) {
    let value = value.trim().to_lowercase();
    if value.is_empty() {
        return;
    }
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        if "-+&|!(){}[]^\"~*?:\\/".contains(character) {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    terms.push(format!("{field}:({escaped})"));
}

fn limiter() -> &'static DefaultDirectRateLimiter {
    REQUEST_LIMITER.get_or_init(|| RateLimiter::direct(Quota::per_second(NonZeroU32::MIN)))
}

fn wait_for_permit() {
    let limiter = limiter();
    let clock = DefaultClock::default();
    while let Err(not_until) = limiter.check() {
        thread::sleep(not_until.wait_time_from(clock.now()));
    }
}

fn request_with_retry<T>(
    mut request: impl FnMut() -> Result<T, Box<musicbrainz_rs::ApiEndpointError>>,
) -> Result<T, Error> {
    let operation = || {
        wait_for_permit();
        request()
    };
    operation
        .retry(ExponentialBuilder::default().with_max_times(3))
        .when(|error| is_temporary(error))
        .notify(|error, delay| tracing::warn!(%error, ?delay, "retry MusicBrainz request"))
        .call()
        .map_err(Error::from)
}

fn is_temporary(error: &musicbrainz_rs::ApiEndpointError) -> bool {
    let musicbrainz_rs::ApiEndpointError::ApiRequestError { source, .. } = error else {
        return false;
    };
    if source.is_retryable() {
        return true;
    }
    match source {
        ApiRequestError::UreqError { source, .. }
        | ApiRequestError::ParsingError { source, .. } => {
            matches!(source, ureq::Error::StatusCode(code) if *code == 429 || *code >= 500)
        }
        _ => false,
    }
}

fn artist_name(credits: &[ArtistCredit]) -> String {
    let mut name = String::new();
    for credit in credits {
        name.push_str(&credit.artist.name);
        if let Some(join) = &credit.joinphrase {
            name.push_str(join);
        }
    }
    name
}

/// Convert one decoded MusicBrainz response to the shared matching model.
#[must_use]
pub fn release_candidate(release: &Release) -> ReleaseCandidate {
    release_candidate_with_options(release, &ReleaseOptions::default())
}

/// Convert a response with the user's beets match settings.
#[must_use]
pub fn release_candidate_with_options(
    release: &Release,
    options: &ReleaseOptions,
) -> ReleaseCandidate {
    let credits = release.artist_credit.as_deref().unwrap_or_default();
    let is_various_artists = credits
        .first()
        .is_some_and(|credit| credit.artist.id == VARIOUS_ARTISTS_ID);
    let artist = if is_various_artists {
        options.various_artists_name.clone()
    } else {
        artist_name(credits)
    };
    let all_media = release.media.as_deref().unwrap_or_default();
    let media: Vec<_> = all_media
        .iter()
        .filter(|medium| {
            !medium
                .format
                .as_ref()
                .is_some_and(|format| options.ignored_media.contains(format))
        })
        .collect();
    let mut tracks = Vec::new();
    let mut medium_number: u32 = 0;
    let mut track_number: u32 = 0;
    for medium in &media {
        medium_number = medium_number.saturating_add(1);
        let medium_index = medium.position.unwrap_or(medium_number);
        for track in medium.tracks.as_deref().unwrap_or_default() {
            if track.recording.as_ref().is_some_and(|recording| {
                recording.title == "[data track]"
                    || (options.ignore_video_tracks && recording.video.unwrap_or(false))
            }) {
                continue;
            }
            let recording = track.recording.as_ref();
            let artist_credits = track
                .artist_credit
                .as_deref()
                .or_else(|| recording.and_then(|item| item.artist_credit.as_deref()))
                .unwrap_or(credits);
            let length_ms = track
                .length
                .or_else(|| recording.and_then(|item| item.length));
            track_number = track_number.saturating_add(1);
            tracks.push(TrackCandidate {
                recording_id: recording.map(|item| RecordingId(item.id.clone())),
                release_track_id: Some(track.id.clone()),
                title: track.title.clone(),
                artist: artist_name(artist_credits),
                length_seconds: length_ms.map(|length| f64::from(length) / 1000.0),
                index: track_number,
                medium: medium_index,
                medium_index: track.position,
            });
        }
    }
    let media_formats: std::collections::HashSet<&str> = media
        .iter()
        .filter_map(|medium| medium.format.as_deref())
        .collect();
    let media_name = if media_formats.len() == 1 && !tracks.is_empty() {
        media_formats
            .iter()
            .next()
            .map(|value| (*value).to_string())
    } else {
        Some("Media".to_string())
    };
    let first_label = release
        .label_info
        .as_deref()
        .and_then(|labels| labels.first());
    let preferred_event = options.preferred_countries.iter().find_map(|country| {
        release.release_events.as_deref()?.iter().find_map(|event| {
            let codes = event.area.as_ref()?.iso_3166_1_codes.as_ref()?;
            codes
                .contains(country)
                .then_some((country.clone(), event.date.as_ref()))
        })
    });
    let event_date = preferred_event
        .as_ref()
        .map(|(_, date)| *date)
        .unwrap_or(release.date.as_ref());
    let year = event_date.and_then(|date| date.year()).or_else(|| {
        release
            .release_group
            .as_ref()
            .and_then(|group| group.first_release_date.as_ref())
            .and_then(|date| date.year())
    });
    ReleaseCandidate {
        id: ReleaseId(release.id.clone()),
        title: release.title.clone(),
        artist,
        tracks,
        release_group_id: release.release_group.as_ref().map(|group| group.id.clone()),
        year,
        country: preferred_event
            .map(|(country, _)| country)
            .or_else(|| release.country.clone()),
        media: media_name,
        label: first_label
            .and_then(|info| info.label.as_ref())
            .map(|label| label.name.clone())
            .filter(|name| name != "[no label]"),
        catalog_number: first_label.and_then(|info| info.catalog_number.clone()),
        disambiguation: release
            .disambiguation
            .clone()
            .filter(|value| !value.is_empty()),
        is_various_artists,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_query_uses_beets_fields_and_escapes_values() {
        let criteria = ReleaseSearch {
            release: " Song (Live) ".to_string(),
            artist: Some("AC/DC".to_string()),
            catalog_number: Some(" ABC-42 ".to_string()),
            tracks: Some(12),
            ..ReleaseSearch::default()
        };
        assert_eq!(
            criteria.query().unwrap(),
            "release:(song \\(live\\)) artist:(ac\\/dc) catno:(abc\\-42) tracks:(12)"
        );
    }

    #[test]
    fn various_artists_search_uses_musicbrainz_artist_id() {
        let criteria = ReleaseSearch {
            release: "Compilation".to_string(),
            artist: Some("Wrong Artist".to_string()),
            various_artists: true,
            ..ReleaseSearch::default()
        };
        assert_eq!(
            criteria.query().unwrap(),
            "release:(compilation) arid:(89ad4ac3\\-39f7\\-470e\\-963a\\-56509c546377)"
        );
    }
}
