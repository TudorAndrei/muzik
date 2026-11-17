extern crate bolero as _;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use muzik_tags::{ListField, TagData, read, write};
use proptest::prelude::*;

const SUFFIXES: &[&str] = &["mp3", "flac", "m4a", "opus", "ogg"];
const TEXT_FIELDS: &[&str] = &[
    "title",
    "artist",
    "album",
    "albumartist",
    "label",
    "catalognum",
    "country",
    "media",
    "albumdisambig",
];
const ID_FIELDS: &[&str] = &[
    "mb_trackid",
    "mb_releasetrackid",
    "mb_workid",
    "mb_albumid",
    "mb_releasegroupid",
    "mb_artistid",
    "mb_albumartistid",
];
const CUSTOM_KEYS: &[&str] = &["MUZIK_MOOD", "MUZIK_SOURCE"];

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> std::io::Result<Self> {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "muzik-tags-properties-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path)?;
        Ok(Self(path))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _removed = fs::remove_dir_all(&self.0);
    }
}

fn text() -> BoxedStrategy<String> {
    "[^\u{0}]{1,30}".boxed()
}

fn field(
    name: &'static str,
    value: BoxedStrategy<String>,
) -> impl Strategy<Value = Option<(String, String)>> {
    proptest::option::of(value).prop_map(move |value| value.map(|value| (name.to_owned(), value)))
}

fn tag_data() -> impl Strategy<Value = TagData> {
    let fields: Vec<_> = TEXT_FIELDS
        .iter()
        .map(|name| field(name, text()))
        .chain(
            ID_FIELDS
                .iter()
                .map(|name| field(name, "[ -~]{1,36}".boxed())),
        )
        .collect();
    (
        fields,
        proptest::collection::btree_map(
            prop_oneof![Just(ListField::Artists), Just(ListField::AlbumArtists)],
            proptest::collection::vec(text(), 1..3),
            0..=2,
        ),
        proptest::collection::btree_map(
            proptest::sample::select(CUSTOM_KEYS).prop_map(str::to_owned),
            text(),
            0..=2,
        ),
    )
        .prop_map(|(fields, lists, custom)| TagData {
            fields: fields.into_iter().flatten().collect::<BTreeMap<_, _>>(),
            lists,
            custom,
        })
}

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(32))]

    #[test]
    fn written_tags_read_back_unchanged(data in tag_data()) {
        let scratch = Scratch::new()?;
        for suffix in SUFFIXES {
            let path = scratch.0.join(format!("blank.{suffix}"));
            fs::copy(fixtures().join(format!("blank.{suffix}")), &path)?;
            write(&path, &data)?;
            let reread = read(&path, CUSTOM_KEYS)?;
            prop_assert_eq!(&reread.custom, &data.custom, "{}: custom", suffix);
            prop_assert_eq!(&reread.lists, &data.lists, "{}: lists", suffix);
            for (key, value) in &data.fields {
                prop_assert_eq!(reread.fields.get(key), Some(value), "{}: {}", suffix, key);
            }
        }
    }
}
