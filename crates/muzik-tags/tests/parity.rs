use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use muzik_tags::{read, write, TagData};
use serde_json::Value;

const SUFFIXES: &[&str] = &["mp3", "flac", "m4a", "opus", "ogg"];

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn expected() -> Result<Value, Box<dyn std::error::Error>> {
    Ok(serde_json::from_slice(&fs::read(
        fixtures().join("mediafile_tags.json"),
    )?)?)
}

fn expected_fields() -> Result<BTreeMap<String, String>, Box<dyn std::error::Error>> {
    let expected = expected()?;
    let values = expected
        .get("files")
        .and_then(|files| files.get("mp3"))
        .ok_or("missing files.mp3")?;
    let mut fields = BTreeMap::new();
    for key in [
        "title",
        "artist",
        "album",
        "albumartist",
        "mb_trackid",
        "mb_releasetrackid",
        "mb_workid",
        "mb_albumid",
        "mb_releasegroupid",
        "mb_artistid",
        "mb_albumartistid",
        "label",
        "catalognum",
        "country",
        "media",
        "albumdisambig",
    ] {
        let value = values
            .get(key)
            .and_then(Value::as_str)
            .ok_or_else(|| format!("missing {key}"))?;
        fields.insert(key.into(), value.into());
    }
    for key in ["track", "tracktotal", "disc", "disctotal"] {
        let value = values.get(key).ok_or_else(|| format!("missing {key}"))?;
        fields.insert(key.into(), value.to_string());
    }
    fields.insert("date".into(), "2021-04-07".into());
    fields.insert("original_date".into(), "2019-11-03".into());
    fields.insert("comp".into(), "1".into());
    fields.insert("rg_track_gain".into(), "-5.25 dB".into());
    fields.insert("rg_album_gain".into(), "-4.50 dB".into());
    fields.insert("rg_track_peak".into(), "0.912345".into());
    fields.insert("rg_album_peak".into(), "0.987654".into());
    Ok(fields)
}

#[test]
fn reads_mediafile_tags_in_five_formats() {
    let expected = expected_fields().unwrap();
    for suffix in SUFFIXES {
        let path = fixtures().join(format!("mediafile.{suffix}"));
        let actual = read(&path, &["MUZIK_MOOD"]).unwrap();
        assert_eq!(
            actual.custom.get("MUZIK_MOOD").map(String::as_str),
            Some("calm"),
            "{suffix}: custom"
        );
        assert_eq!(
            actual.lists.get("artists"),
            Some(&vec!["Mara Vale".into(), "Lio Hart".into()]),
            "{suffix}: artists"
        );
        assert_eq!(
            actual.lists.get("albumartists"),
            Some(&vec!["Mara Vale".into(), "Lio Hart".into()]),
            "{suffix}: albumartists"
        );
        for (key, value) in &expected {
            assert_eq!(actual.fields.get(key), Some(value), "{suffix}: {key}");
        }
    }
}

#[test]
fn writes_tags_for_mediafile_to_read() {
    let root = std::env::var_os("MUZIK_TAGS_WRITE_FIXTURES")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join(format!("muzik-tags-{}", std::process::id())));
    fs::create_dir_all(&root).unwrap();
    let data = TagData {
        fields: expected_fields().unwrap(),
        lists: BTreeMap::from([
            (
                "artists".into(),
                vec!["Mara Vale".into(), "Lio Hart".into()],
            ),
            (
                "albumartists".into(),
                vec!["Mara Vale".into(), "Lio Hart".into()],
            ),
        ]),
        custom: BTreeMap::from([("MUZIK_MOOD".into(), "calm".into())]),
    };
    for suffix in SUFFIXES {
        let source = fixtures().join(format!("blank.{suffix}"));
        let dest = root.join(format!("rust.{suffix}"));
        fs::copy(source, &dest).unwrap();
        write(&dest, &data).unwrap();
        let reread = read(&dest, &["MUZIK_MOOD"]).unwrap();
        assert_eq!(reread.custom, data.custom, "{suffix}: custom");
        assert_eq!(reread.lists, data.lists, "{suffix}: lists");
        for (key, value) in &data.fields {
            assert_eq!(reread.fields.get(key), Some(value), "{suffix}: {key}");
        }
    }
    if std::env::var_os("MUZIK_TAGS_WRITE_FIXTURES").is_none() {
        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn partial_update_preserves_custom_keys() {
    let root = std::env::temp_dir().join(format!("muzik-tags-preserve-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    for suffix in SUFFIXES {
        let source = fixtures().join(format!("mediafile.{suffix}"));
        let dest = root.join(format!("update.{suffix}"));
        fs::copy(source, &dest).unwrap();
        let data = TagData {
            fields: BTreeMap::from([("title".into(), "Changed Title".into())]),
            ..TagData::default()
        };
        write(&dest, &data).unwrap();
        let reread = read(&dest, &["MUZIK_MOOD"]).unwrap();
        assert_eq!(
            reread.fields.get("title").map(String::as_str),
            Some("Changed Title"),
            "{suffix}"
        );
        assert_eq!(
            reread.custom.get("MUZIK_MOOD").map(String::as_str),
            Some("calm"),
            "{suffix}"
        );
    }
    fs::remove_dir_all(root).unwrap();
}
