#[cfg(test)]
mod item_state {
    use muzik_core::QualityPolicy;
    use muzik_store::jobs::{Ending, Job, Kind, NewJob, Queue, Status, Store};
    use muzik_store::watchlist::jobs::{
        self, JobError, JobOptions, LoadedSource, Operations, RunningJob,
    };
    use muzik_store::watchlist::{
        ItemAction, ItemId, Playlist, ReconcileOptions, Repository, SourceKind, Stage, StageEvent,
        StageStatus, WatchItem,
    };
    use proptest::prelude::*;
    use serde_json::{Value, json};
    use std::collections::HashSet;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, Ordering};

    const KEYS: [&str; 4] = ["aaaaaaaaaaa", "bbbbbbbbbbb", "ccccccccccc", "ddddddddddd"];
    const ASKING: [Stage; 4] = [
        Stage::Download,
        Stage::Quality,
        Stage::Parse,
        Stage::Organize,
    ];

    #[derive(Clone, Copy, Debug)]
    enum Outcome {
        Complete,
        Wait(usize),
        Fail(usize),
        Cancel,
    }

    #[derive(Clone, Debug)]
    enum Step {
        Sync(Vec<usize>),
        Enqueue(usize),
        Claim,
        Finish(Outcome),
        Answer(usize),
        Cancel(usize),
        Crash,
    }

    fn step() -> impl Strategy<Value = Step> {
        let outcome = prop_oneof![
            Just(Outcome::Complete),
            (0..ASKING.len()).prop_map(Outcome::Wait),
            (0..ASKING.len()).prop_map(Outcome::Fail),
            Just(Outcome::Cancel),
        ];
        prop_oneof![
            proptest::collection::vec(0..KEYS.len(), 0..5).prop_map(Step::Sync),
            (0..8_usize).prop_map(Step::Enqueue),
            Just(Step::Claim),
            outcome.prop_map(Step::Finish),
            (0..8_usize).prop_map(Step::Answer),
            (0..8_usize).prop_map(Step::Cancel),
            Just(Step::Crash),
        ]
    }

    fn kind_of(stage: Stage) -> &'static str {
        match stage {
            Stage::Download => "soulseek_candidate",
            Stage::Quality => "quality_replacement",
            Stage::Parse => "chapter_review",
            Stage::Split | Stage::Organize => "import_match",
        }
    }

    struct Fake {
        outcome: Outcome,
        output: PathBuf,
    }

    impl Fake {
        fn reach(item: &mut WatchItem, stage: Stage, output: &Path) {
            for earlier in Stage::ALL
                .iter()
                .copied()
                .take_while(|earlier| *earlier != stage)
            {
                let path = (earlier == Stage::Download)
                    .then(|| output.join(format!("{} [{}].opus", item.title, item.title)));
                item.apply(earlier, StageEvent::Complete(path));
            }
        }
    }

    impl Operations for Fake {
        fn load(&mut self, _: &Playlist) -> Result<LoadedSource, JobError> {
            Err(JobError::Operation("no source in this test".into()))
        }

        fn process(
            &mut self,
            _: &Playlist,
            item: &mut WatchItem,
            action: ItemAction,
            cancelled: &AtomicBool,
        ) -> Result<(), JobError> {
            let order = |stage: Stage| Stage::ALL.iter().position(|known| *known == stage);
            let at = |index: usize| {
                let stage = ASKING[index];
                if order(stage) < order(action.stage()) {
                    action.stage()
                } else {
                    stage
                }
            };
            match self.outcome {
                Outcome::Complete => {
                    Self::reach(item, Stage::Organize, &self.output);
                    item.apply(Stage::Organize, StageEvent::Complete(None));
                    Ok(())
                }
                Outcome::Wait(index) => {
                    let stage = at(index);
                    Self::reach(item, stage, &self.output);
                    Err(JobError::Waiting {
                        stage,
                        question: json!({"kind": kind_of(stage), "payload": {}}),
                    })
                }
                Outcome::Fail(index) => {
                    let stage = at(index);
                    Self::reach(item, stage, &self.output);
                    Err(JobError::Failed {
                        stage,
                        message: format!("{stage} failed"),
                    })
                }
                Outcome::Cancel => {
                    cancelled.store(true, Ordering::SeqCst);
                    Err(JobError::Cancelled)
                }
            }
        }
    }

    struct World {
        _directory: tempfile::TempDir,
        database: PathBuf,
        output: PathBuf,
        running: Vec<Job>,
    }

    impl World {
        fn new() -> Self {
            let directory = tempfile::tempdir().unwrap();
            let database = directory.path().join("muzik.db");
            let output = directory.path().join("downloads");
            std::fs::create_dir_all(&output).unwrap();
            for key in &KEYS[..3] {
                std::fs::write(output.join(format!("{key} [{key}].opus")), b"").unwrap();
            }
            let repository = Repository::new(database.clone());
            repository
                .update(|document| {
                    document
                        .playlists
                        .push(Playlist::new("PL1", "u", SourceKind::Youtube, None));
                    Ok(())
                })
                .unwrap();
            Self {
                _directory: directory,
                database,
                output,
                running: Vec::new(),
            }
        }

        fn repository(&self) -> Repository {
            Repository::new(self.database.clone())
        }

        fn store(&self) -> Store {
            Store::from_connection(muzik_store::db::open(&self.database).unwrap())
        }

        fn jobs(&self, status: Status) -> Vec<Job> {
            self.store().list(status).unwrap()
        }

        fn apply(&mut self, step: &Step) {
            match step {
                Step::Sync(keys) => self.sync(keys),
                Step::Enqueue(index) => self.enqueue(*index),
                Step::Claim => {
                    if let Some(job) = self.store().claim(Queue::Item).unwrap() {
                        self.running.push(job);
                    }
                }
                Step::Finish(outcome) => self.finish(*outcome),
                Step::Answer(index) => {
                    let waiting = self.jobs(Status::Waiting);
                    if let Some(job) = pick(&waiting, *index) {
                        self.store().answer(job.id, &json!("as_is")).unwrap();
                    }
                }
                Step::Cancel(index) => {
                    let mut open = self.jobs(Status::Waiting);
                    open.extend(self.store().list_open().unwrap());
                    if let Some(job) = pick(&open, *index) {
                        self.store().request_cancel(job.id).unwrap();
                    }
                }
                Step::Crash => {
                    self.running.clear();
                    let output = self.output.clone();
                    jobs::repair(&self.repository(), |_| Some(output.clone())).unwrap();
                }
            }
        }

        fn sync(&self, keys: &[usize]) {
            let items: Vec<WatchItem> = keys
                .iter()
                .zip(1_u64..)
                .map(|(key, position)| {
                    let key = KEYS[*key];
                    let mut item = WatchItem::new(position, key, SourceKind::Youtube);
                    item.video_id = Some(key.into());
                    item.video_url = Some(format!("https://www.youtube.com/watch?v={key}"));
                    item
                })
                .collect();
            self.repository()
                .update(|document| {
                    document.playlists[0].merge_items(items);
                    Ok(())
                })
                .unwrap();
        }

        fn enqueue(&self, index: usize) {
            let document = self.repository().load().unwrap();
            let playlist = &document.playlists[0];
            let ids = playlist.item_ids();
            let Some(id) = pick(&ids, index) else {
                return;
            };
            let mut params = json!({"action": "run"});
            id.write(&mut params).unwrap();
            self.store()
                .replace_waiting(&NewJob {
                    kind: Kind::Item,
                    item_key: &id.to_string(),
                    title: "item",
                    params: &params,
                })
                .unwrap();
        }

        fn finish(&mut self, outcome: Outcome) {
            if self.running.is_empty() {
                return;
            }
            let job = self.running.remove(0);
            let id = ItemId::from_params(&job.params).unwrap();
            let action: ItemAction = job.params["action"].as_str().unwrap().parse().unwrap();
            let cancelled =
                AtomicBool::new(self.store().cancel_requests().unwrap().contains(&job.id));
            let splits = self.output.join("splits");
            let options = JobOptions {
                reconcile: ReconcileOptions {
                    output: &self.output,
                    splits: &splits,
                    cache: &splits,
                    config: None,
                    no_organize: false,
                    no_split: false,
                    quality_policy: QualityPolicy::Off,
                },
                output: &self.output,
                cache: &splits,
                dry_run: false,
                playlist_id: None,
                job: Some(RunningJob {
                    id: job.id,
                    params: &job.params,
                }),
            };
            let mut fake = Fake {
                outcome,
                output: self.output.clone(),
            };
            let result = jobs::run_item(
                &self.repository(),
                options,
                &id,
                action,
                &mut fake,
                &cancelled,
            );
            let after = self.store().get(job.id).unwrap().unwrap();
            assert_ne!(after.status, Status::Running, "{job:?} {result:?}");
            let message = match &result {
                Err(error) => error.to_string(),
                Ok(_) => String::new(),
            };
            let ending = match &result {
                Ok(_) => Ending::Done,
                Err(JobError::Cancelled) => Ending::Cancelled,
                Err(_) => Ending::Failed(&message),
            };
            assert!(!self.store().end(job.id, ending).unwrap());
        }

        fn check(&self) {
            let snapshot = self.repository().snapshot().unwrap();
            let playlist = &snapshot.document.playlists[0];
            let ids = playlist.item_ids();
            let mut jobs = Vec::new();
            for status in [Status::Queued, Status::Running, Status::Waiting] {
                jobs.extend(self.jobs(status));
            }
            let count = |id: &ItemId, status: Option<Status>| {
                jobs.iter()
                    .filter(|job| job.kind == Kind::Item && job.item_key == id.to_string())
                    .filter(|job| status.is_none_or(|status| job.status == status))
                    .count()
            };
            for (id, item) in ids.iter().zip(&playlist.items) {
                let hold = snapshot.holds.get(id);
                assert!(count(id, None) <= 1, "{id} has more than one open job");
                assert_eq!(
                    hold.is_some_and(|hold| hold.status() == StageStatus::Waiting),
                    count(id, Some(Status::Waiting)) == 1,
                    "{id}"
                );
                assert_eq!(
                    hold.is_some_and(|hold| hold.status() == StageStatus::Running),
                    count(id, Some(Status::Running)) == 1,
                    "{id}"
                );
                if let Some(hold) = hold {
                    for earlier in Stage::ALL
                        .iter()
                        .take_while(|stage| **stage != hold.stage())
                    {
                        assert!(
                            item.status(*earlier).is_done(),
                            "{id} is held at {} before {earlier} is done",
                            hold.stage()
                        );
                    }
                }
            }
            let known: HashSet<String> = ids.iter().map(ToString::to_string).collect();
            for job in &jobs {
                if job.status != Status::Running {
                    assert!(known.contains(&job.item_key), "{job:?} has no item");
                }
            }
            let connection = muzik_store::db::open(&self.database).unwrap();
            let mut statement = connection
                .prepare("SELECT data FROM watchlist_items")
                .unwrap();
            let rows: Vec<Value> = statement
                .query_map([], |row| row.get(0))
                .unwrap()
                .map(Result::unwrap)
                .collect();
            for row in rows {
                for (stage, record) in row["stages"].as_object().unwrap() {
                    let status = record["status"].as_str().unwrap();
                    assert!(
                        ["not_started", "complete", "failed", "skipped", "stale"].contains(&status),
                        "{stage} is stored as {status}"
                    );
                    assert!(
                        record["path"].is_null() || status == "complete",
                        "{stage}: {record}"
                    );
                    assert_eq!(
                        !record["error"].is_null(),
                        status == "failed",
                        "{stage}: {record}"
                    );
                    assert!(record.get("question").is_none(), "{stage}: {record}");
                }
            }
        }
    }

    fn pick<T>(values: &[T], index: usize) -> Option<&T> {
        values.get(index.checked_rem(values.len())?)
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(64))]

        #[test]
        fn items_and_jobs_agree_after_any_sequence(steps in proptest::collection::vec(step(), 1..24)) {
            let mut world = World::new();
            for step in &steps {
                world.apply(step);
                world.check();
            }
        }
    }
}
