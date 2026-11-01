//! Blocking Cover Art Archive front image fetch.

use std::time::Duration;

use backon::{BlockingRetryable, ExponentialBuilder};
use musicbrainz_rs::api_bindium::ureq;

const BASE_URL: &str = "https://coverartarchive.org";
const IMAGE_NAMES: [&str; 2] = ["front-1200", "front"];
const MAX_IMAGE_BYTES: u64 = 64 * 1024 * 1024;
const TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug, thiserror::Error)]
pub enum CoverArtError {
    #[error("invalid MusicBrainz release ID: {0}")]
    InvalidReleaseId(String),
    #[error("Cover Art Archive request failed: {0}")]
    Request(#[from] Box<ureq::Error>),
}

pub trait FrontCoverSource {
    /// Return the front image of a release, or `None` when the release has no front image.
    ///
    /// # Errors
    /// Returns an error when the release ID is invalid or the request fails.
    fn front_cover(&self, release_id: &str) -> Result<Option<Vec<u8>>, CoverArtError>;
}

/// A blocking client. It shares the `MusicBrainz` request limiter.
pub struct CoverArtArchive {
    agent: ureq::Agent,
    base_url: String,
}

impl CoverArtArchive {
    #[must_use]
    pub fn new(user_agent: &str) -> Self {
        Self::with_base_url(user_agent, BASE_URL)
    }

    #[must_use]
    pub fn with_base_url(user_agent: &str, base_url: &str) -> Self {
        let agent = ureq::Agent::config_builder()
            .user_agent(user_agent)
            .timeout_global(Some(TIMEOUT))
            .build()
            .into();
        Self {
            agent,
            base_url: base_url.trim_end_matches('/').to_owned(),
        }
    }

    fn image(&self, url: &str) -> Result<Option<Vec<u8>>, CoverArtError> {
        let operation = || {
            super::wait_for_permit();
            match self.agent.get(url).call() {
                Ok(response) => response
                    .into_body()
                    .with_config()
                    .limit(MAX_IMAGE_BYTES)
                    .read_to_vec()
                    .map(Some),
                Err(ureq::Error::StatusCode(404)) => Ok(None),
                Err(error) => Err(error),
            }
        };
        operation
            .retry(ExponentialBuilder::default().with_max_times(3))
            .when(is_temporary)
            .notify(
                |error, delay| tracing::warn!(%error, ?delay, "retry Cover Art Archive request"),
            )
            .call()
            .map_err(|error| CoverArtError::Request(Box::new(error)))
    }
}

impl FrontCoverSource for CoverArtArchive {
    fn front_cover(&self, release_id: &str) -> Result<Option<Vec<u8>>, CoverArtError> {
        let id = release_id.trim();
        if !is_release_id(id) {
            return Err(CoverArtError::InvalidReleaseId(release_id.to_owned()));
        }
        for name in IMAGE_NAMES {
            let url = format!("{}/release/{id}/{name}", self.base_url);
            tracing::debug!(url, "fetch Cover Art Archive image");
            if let Some(bytes) = self.image(&url)? {
                return Ok(Some(bytes));
            }
        }
        Ok(None)
    }
}

fn is_release_id(id: &str) -> bool {
    id.len() == 36
        && id.char_indices().all(|(index, character)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                character == '-'
            } else {
                character.is_ascii_hexdigit()
            }
        })
}

const fn is_temporary(error: &ureq::Error) -> bool {
    match error {
        ureq::Error::StatusCode(code) => *code == 429 || *code >= 500,
        ureq::Error::Io(_)
        | ureq::Error::Timeout(_)
        | ureq::Error::ConnectionFailed
        | ureq::Error::HostNotFound
        | ureq::Error::BodyStalled => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::{CoverArtArchive, CoverArtError, FrontCoverSource};
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    use std::thread;

    const ID: &str = "76df3287-6cda-33eb-8e9a-044b5e15ffdd";

    fn serve(responses: Vec<(u16, &'static [u8])>) -> (String, thread::JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let handle = thread::spawn(move || {
            let mut requests = Vec::new();
            for (status, body) in responses {
                let (mut stream, _) = listener.accept().unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut head = Vec::new();
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    if line == "\r\n" || line.is_empty() {
                        break;
                    }
                    head.push(line.trim_end().to_owned());
                }
                requests.push(head.join("\n"));
                write!(
                    stream,
                    "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                )
                .unwrap();
                stream.write_all(body).unwrap();
            }
            requests
        });
        (base, handle)
    }

    #[test]
    fn the_large_front_image_comes_first_and_the_full_image_is_the_fallback() {
        let (base, server) = serve(vec![(404, b""), (200, b"image")]);
        let client = CoverArtArchive::with_base_url("muzik-test/1", &base);
        assert_eq!(client.front_cover(ID).unwrap(), Some(b"image".to_vec()));
        let requests = server.join().unwrap();
        assert_eq!(requests.len(), 2);
        assert!(requests[0].starts_with(&format!("GET /release/{ID}/front-1200 ")));
        assert!(requests[1].starts_with(&format!("GET /release/{ID}/front ")));
        assert!(
            requests[0]
                .to_ascii_lowercase()
                .contains("user-agent: muzik-test/1")
        );
    }

    #[test]
    fn a_release_without_art_has_no_front_image() {
        let (base, server) = serve(vec![(404, b""), (404, b"")]);
        let client = CoverArtArchive::with_base_url("muzik-test/1", &base);
        assert_eq!(client.front_cover(ID).unwrap(), None);
        assert_eq!(server.join().unwrap().len(), 2);
    }

    #[test]
    fn an_invalid_release_id_makes_no_request() {
        let client = CoverArtArchive::with_base_url("muzik-test/1", "http://127.0.0.1:9");
        for id in ["", "../../etc", "76df3287-6cda-33eb-8e9a-044b5e15ffdz"] {
            assert!(matches!(
                client.front_cover(id),
                Err(CoverArtError::InvalidReleaseId(_))
            ));
        }
    }
}
