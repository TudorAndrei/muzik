#![allow(
    dead_code,
    reason = "each feature target uses a different part of this module"
)]

use cucumber::World;
use cucumber::cli::{self, Parser as _};
use cucumber::codegen::WorldInventory;
use std::fmt::Debug;
use std::path::{Path, PathBuf};

pub type StepResult = anyhow::Result<()>;

pub fn fixture(path: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(path)
}

pub fn run<W>(domain: &str)
where
    W: World + WorldInventory + Debug,
{
    if std::env::args().any(|argument| argument == "--list") {
        return;
    }
    let opts = cli::Opts::<_, _, _, cli::Empty>::try_parse().unwrap_or_default();
    let features = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../features")
        .join(domain);
    futures::executor::block_on(
        W::cucumber()
            .fail_on_skipped()
            .with_cli(opts)
            .run_and_exit(features),
    );
}
