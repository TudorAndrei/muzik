use std::fs;
use std::path::{Path, PathBuf};

use muzik_tags::{embed_cover, find_cover, has_front_cover, probe, read};

const SUFFIXES: &[&str] = &["mp3", "flac", "m4a", "opus", "ogg"];

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

#[test]
fn probes_audio_properties_in_five_formats() {
    for suffix in SUFFIXES {
        let path = fixtures().join(format!("mediafile.{suffix}"));
        let audio = probe(&path).unwrap();
        assert_eq!(audio.format.to_string(), *suffix);
        assert!(
            audio
                .duration_seconds
                .is_some_and(|seconds| (0.25..0.40).contains(&seconds)),
            "{suffix}: duration {audio:?}"
        );
        assert_eq!(audio.sample_rate_hz, Some(48_000), "{suffix}");
        assert_eq!(
            audio.channels,
            Some(if *suffix == "ogg" { 2 } else { 1 }),
            "{suffix}"
        );
        assert_eq!(audio.size_bytes, fs::metadata(path).unwrap().len());
    }
}

#[test]
fn finds_named_cover_in_album_tree() {
    let root = fixtures();
    assert_eq!(find_cover(root), Some(fixtures().join("cover.png")));
}

#[test]
fn embeds_front_cover_and_preserves_tags() {
    let root = std::env::var_os("MUZIK_TAGS_WRITE_FIXTURES")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::temp_dir().join(format!("muzik-cover-{}", std::process::id()))
        });
    fs::create_dir_all(&root).unwrap();
    let image = fs::read(fixtures().join("cover.png")).unwrap();
    for suffix in SUFFIXES {
        let dest = root.join(format!("cover.{suffix}"));
        fs::copy(fixtures().join(format!("mediafile.{suffix}")), &dest).unwrap();
        embed_cover(&dest, &image, "image/png").unwrap();
        assert!(has_front_cover(&dest).unwrap(), "{suffix}");
        let tags = read(&dest, &["MUZIK_MOOD"]).unwrap();
        assert_eq!(
            tags.fields.get("title").map(String::as_str),
            Some("Tide & Stone"),
            "{suffix}"
        );
        assert_eq!(
            tags.custom.get("MUZIK_MOOD").map(String::as_str),
            Some("calm"),
            "{suffix}"
        );
    }
    if std::env::var_os("MUZIK_TAGS_WRITE_FIXTURES").is_none() {
        fs::remove_dir_all(root).unwrap();
    }
}
