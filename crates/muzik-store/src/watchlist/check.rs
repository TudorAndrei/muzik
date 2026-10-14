//! Local checks own revision retries and suppress results after a newer edit or load.

use super::{CheckedWrite, ReconcileOptions, Repository, import_cache, reconcile, view};
use crate::Result;
use muzik_core::QualityPolicy;
use parking_lot::Mutex;
use serde_json::Value;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// Coordinates local checks with source edits and runner activity.
pub struct LocalChecks {
    latest: Arc<AtomicU64>,
    gate: Arc<Mutex<()>>,
}

impl LocalChecks {
    #[must_use]
    pub fn new(latest: Arc<AtomicU64>) -> Self {
        Self {
            latest,
            gate: Arc::new(Mutex::new(())),
        }
    }

    /// Applies an edit and invalidates earlier checks before they can save or publish.
    /// # Errors
    /// Returns the error from the edit.
    pub fn edit<T, E>(
        &self,
        change: impl FnOnce() -> std::result::Result<T, E>,
    ) -> std::result::Result<T, E> {
        let _gate = self.gate.lock();
        let result = change()?;
        self.latest.fetch_add(1, Ordering::SeqCst);
        Ok(result)
    }

    /// Returns the saved view and a check to run on a background thread.
    /// Invalidates older checks before loading settings or saved data.
    /// # Errors
    /// Returns an error if settings or the saved watchlist cannot be read.
    pub fn prepare<E: From<crate::Error>>(
        &self,
        repository: Repository,
        options: impl FnOnce() -> Result<LocalCheckOptions, E>,
    ) -> Result<(Value, LocalCheck), E> {
        let generation = {
            let _gate = self.gate.lock();
            self.latest.fetch_add(1, Ordering::SeqCst).wrapping_add(1)
        };
        let options = options()?;
        let saved = view(&repository.load()?, &options.output, &options.cache)?;
        Ok((
            saved,
            LocalCheck {
                repository,
                options,
                latest: Arc::clone(&self.latest),
                gate: Arc::clone(&self.gate),
                generation,
            },
        ))
    }
}

/// A single local check. Its result is published only while it is current and idle.
pub struct LocalCheck {
    repository: Repository,
    options: LocalCheckOptions,
    latest: Arc<AtomicU64>,
    gate: Arc<Mutex<()>>,
    generation: u64,
}

impl LocalCheck {
    fn current(&self) -> bool {
        self.generation == self.latest.load(Ordering::SeqCst)
    }

    /// Runs the check and publishes its view or error if it is still current.
    /// The callbacks can run under the edit gate and must not call `LocalChecks`.
    pub fn run(self, busy: impl Fn() -> bool, publish: impl FnOnce(Result<Value>)) {
        let result = self.check(&busy);
        let _gate = self.gate.lock();
        if !busy() && self.current() {
            match result {
                Ok(Some(visible)) => publish(Ok(visible)),
                Ok(None) => {}
                Err(error) => publish(Err(error)),
            }
        }
    }

    fn check(&self, busy: &impl Fn() -> bool) -> Result<Option<Value>> {
        let options = self.options.borrowed();
        {
            let _gate = self.gate.lock();
            if busy() || !self.current() {
                return Ok(None);
            }
            import_cache(&self.repository, options)?;
        }
        for _ in 0..3 {
            if busy() || !self.current() {
                return Ok(None);
            }
            let (mut checked, revision) = self.repository.load_revision()?;
            reconcile(&mut checked, options)?;
            let written = {
                let _gate = self.gate.lock();
                if busy() || !self.current() {
                    return Ok(None);
                }
                self.repository.save_at(revision, &checked)?
            };
            if written == CheckedWrite::Conflict {
                continue;
            }
            return view(&checked, options.output, options.cache).map(Some);
        }
        Err("The watchlist changed during the local check. Reload it.".into())
    }
}

/// Owned settings for a saved view and its background check.
/// Build these from `ReconcileOptions`.
pub struct LocalCheckOptions {
    output: PathBuf,
    splits: PathBuf,
    cache: PathBuf,
    config: Option<PathBuf>,
    no_organize: bool,
    no_split: bool,
    quality_policy: QualityPolicy,
}
impl From<ReconcileOptions<'_>> for LocalCheckOptions {
    fn from(options: ReconcileOptions<'_>) -> Self {
        Self {
            output: options.output.to_owned(),
            splits: options.splits.to_owned(),
            cache: options.cache.to_owned(),
            config: options.config.map(std::path::Path::to_owned),
            no_organize: options.no_organize,
            no_split: options.no_split,
            quality_policy: options.quality_policy,
        }
    }
}
impl LocalCheckOptions {
    fn borrowed(&self) -> ReconcileOptions<'_> {
        ReconcileOptions {
            output: &self.output,
            splits: &self.splits,
            cache: &self.cache,
            config: self.config.as_deref(),
            no_organize: self.no_organize,
            no_split: self.no_split,
            quality_policy: self.quality_policy,
        }
    }
}
