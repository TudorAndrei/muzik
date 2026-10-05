use muzik_core::quality::MeasuredQuality;
use muzik_core::sync::{self, Action, Encoding, Target};
use muzik_core::SyncPreset;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

fn audio(format: &str, sample_rate: u32, bit_depth: Option<u32>) -> MeasuredQuality {
    MeasuredQuality {
        format: format.into(),
        lossless: false,
        bitrate_kbps: Some(128),
        sample_rate: Some(sample_rate),
        bit_depth,
        channels: Some(2),
        size: Some(1_000),
    }
}

fn target(path: &Path, preset: SyncPreset) -> Target {
    Target {
        path: path.to_path_buf(),
        preset,
        bitrate: None,
        covers: true,
    }
}

#[test]
fn echo_mini_keeps_playable_audio_and_converts_the_rest() {
    let echo = target(Path::new("/card"), SyncPreset::EchoMini);
    assert_eq!(echo.action(&audio("flac", 44_100, Some(24))), Action::Copy);
    assert_eq!(echo.action(&audio("mp3", 44_100, None)), Action::Copy);
    assert_eq!(echo.action(&audio("vorbis", 44_100, None)), Action::Copy);
    assert_eq!(
        echo.action(&audio("opus", 48_000, None)),
        Action::Convert(Encoding::Mp3 { kbps: 320 })
    );
    assert_eq!(
        echo.action(&audio("flac", 352_800, Some(32))),
        Action::Convert(Encoding::Flac {
            sample_rate: Some(176_400),
            bit_depth: Some(24),
        })
    );
}

#[test]
fn opus_converts_lossless_and_keeps_common_lossy_audio() {
    let phone = Target {
        bitrate: Some(128),
        ..target(Path::new("/phone"), SyncPreset::Opus)
    };
    assert_eq!(
        phone.action(&audio("flac", 44_100, Some(16))),
        Action::Convert(Encoding::Opus { kbps: 128 })
    );
    assert_eq!(phone.action(&audio("aac", 44_100, None)), Action::Copy);
    assert_eq!(phone.action(&audio("opus", 48_000, None)), Action::Copy);
}

#[test]
fn mp3_converts_everything_except_mp3() {
    let player = target(Path::new("/card"), SyncPreset::Mp3);
    for format in ["flac", "opus", "aac", "vorbis"] {
        assert_eq!(
            player.action(&audio(format, 44_100, Some(16))),
            Action::Convert(Encoding::Mp3 { kbps: 320 })
        );
    }
    assert_eq!(player.action(&audio("mp3", 44_100, None)), Action::Copy);
}

#[test]
fn targets_round_trip_through_the_config_file() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let config = dir.path().join("config.yaml");
    fs::write(&config, "spotify:\n  client_id: kept\n")?;
    let phone = Target {
        bitrate: Some(160),
        covers: false,
        ..target(&dir.path().join("phone"), SyncPreset::Opus)
    };
    phone.save(&config, "phone")?;
    let loaded = muzik_core::app_config::load(&config)?;
    assert_eq!(Target::load(&loaded, "phone")?, phone);
    assert_eq!(loaded["spotify"]["client_id"], "kept");
    assert!(Target::load(&loaded, "snowsky").is_err_and(|error| error.contains("phone")));
    let loud = Target {
        bitrate: Some(999),
        ..phone
    };
    assert!(loud.save(&config, "phone").is_err());
    Ok(())
}

#[test]
fn plan_skips_fresh_files_and_names_safe_destinations() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let library = dir.path().join("library");
    let card = dir.path().join("card");
    let album = library.join("Artist/Album: Live?");
    fs::create_dir_all(&album)?;
    fs::create_dir_all(&card)?;
    let flac = album.join("01 Song.flac");
    let opus = album.join("02 Song.opus");
    let cover = album.join("cover.jpg");
    for path in [&flac, &opus, &cover] {
        fs::write(path, b"audio")?;
    }
    let outside = dir.path().join("elsewhere.mp3");
    let echo = target(&card, SyncPreset::EchoMini);
    let probe = |path: &Path| -> Result<Option<MeasuredQuality>, String> {
        let format = path
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or("");
        Ok(Some(MeasuredQuality {
            size: Some(1_000),
            ..audio(format, 44_100, Some(16))
        }))
    };
    let tracks = vec![flac.clone(), opus.clone(), outside.clone()];
    let none = BTreeMap::new();
    let plan = sync::plan(&echo, &library, &tracks, &[cover], &none, 2, &probe);
    let destinations: Vec<PathBuf> = plan
        .pending
        .iter()
        .map(|transfer| transfer.destination.clone())
        .collect();
    let folder = card.join("Artist/Album_ Live_");
    assert_eq!(
        destinations,
        vec![
            folder.join("01 Song.flac"),
            folder.join("02 Song.mp3"),
            folder.join("cover.jpg"),
        ]
    );
    assert_eq!(plan.outside, vec![outside]);
    assert_eq!(plan.pending[1].bytes, 2_500);

    for transfer in &plan.pending {
        if transfer.action == Action::Copy {
            sync::transfer(transfer)?;
        }
    }
    let again = sync::plan(&echo, &library, &tracks, &[], &none, 2, &probe);
    assert_eq!(again.fresh, 1);
    assert_eq!(again.pending.len(), 1);
    Ok(())
}

#[test]
fn plan_writes_one_track_per_device_file_name() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let library = dir.path().join("library");
    let album = library.join("Album");
    fs::create_dir_all(&album)?;
    let tracks = vec![
        album.join("01 Song.flac"),
        album.join("01 Song.opus"),
        album.join("01 song.mp3"),
        album.join("02 Other.flac"),
    ];
    for path in &tracks {
        fs::write(path, b"audio")?;
    }
    let probe = |path: &Path| -> Result<Option<MeasuredQuality>, String> {
        let format = path
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or("");
        Ok(Some(audio(format, 44_100, Some(16))))
    };
    let mp3 = target(&dir.path().join("card"), SyncPreset::Mp3);
    let plan = sync::plan(&mp3, &library, &tracks, &[], &BTreeMap::new(), 2, &probe);
    let sources: Vec<&Path> = plan
        .pending
        .iter()
        .map(|transfer| transfer.source.as_path())
        .collect();
    assert_eq!(sources, vec![tracks[0].as_path(), tracks[3].as_path()]);
    assert_eq!(plan.duplicates, vec![tracks[1].clone(), tracks[2].clone()]);
    Ok(())
}

#[test]
fn a_converted_file_is_current_only_with_the_recorded_encoding(
) -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let library = dir.path().join("library");
    let card = dir.path().join("card");
    fs::create_dir_all(library.join("Album"))?;
    let tracks = vec![library.join("Album/01 Song.flac")];
    fs::write(&tracks[0], b"audio")?;
    let probe = |_: &Path| -> Result<Option<MeasuredQuality>, String> {
        Ok(Some(audio("flac", 44_100, Some(16))))
    };
    let connection = muzik_core::db::open_in_memory()?;
    let mp3 = target(&card, SyncPreset::Mp3);
    let plan_with = |target: &Target| -> Result<sync::Plan, String> {
        let encodings = sync::encodings(&connection, &card)?;
        Ok(sync::plan(
            target,
            &library,
            &tracks,
            &[],
            &encodings,
            1,
            &probe,
        ))
    };

    let first = plan_with(&mp3)?;
    let transfer = &first.pending[0];
    fs::create_dir_all(card.join("Album"))?;
    fs::write(&transfer.destination, b"converted")?;
    assert_eq!(plan_with(&mp3)?.fresh, 0);

    sync::record(&connection, transfer)?;
    assert_eq!(plan_with(&mp3)?.fresh, 1);
    let lower = Target {
        bitrate: Some(128),
        ..mp3.clone()
    };
    assert_eq!(
        plan_with(&lower)?.pending[0].action,
        Action::Convert(Encoding::Mp3 { kbps: 128 })
    );

    sync::record(
        &connection,
        &sync::Transfer {
            action: Action::Copy,
            ..transfer.clone()
        },
    )?;
    assert!(sync::encodings(&connection, &card)?.is_empty());
    Ok(())
}

#[test]
fn stale_files_lists_unplanned_media_and_macos_leftovers() -> Result<(), Box<dyn std::error::Error>>
{
    let dir = tempfile::tempdir()?;
    let root = dir.path();
    fs::create_dir_all(root.join("Album"))?;
    let kept = root.join("Album/01.mp3");
    for name in [
        "Album/01.mp3",
        "Album/02.flac",
        "Album/._01.mp3",
        "Album/.03.muzik-part.mp3",
        "Album/notes.txt",
    ] {
        fs::write(root.join(name), b"x")?;
    }
    let planned = BTreeSet::from([kept]);
    assert_eq!(
        sync::stale_files(root, &planned)?,
        vec![
            root.join("Album/.03.muzik-part.mp3"),
            root.join("Album/._01.mp3"),
            root.join("Album/02.flac"),
        ]
    );
    Ok(())
}
