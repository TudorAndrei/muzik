use muzik_metadata::{MetadataClient, ReleaseSearch};

const RELEASE_ID: &str = "76df3287-6cda-33eb-8e9a-044b5e15ffdd";

#[test]
#[ignore = "requires the public MusicBrainz service"]
fn searches_and_looks_up_a_release() {
    let client = MetadataClient::new("muzik/0.1 (https://github.com/TudorAndrei/muzik)");
    let results = client
        .search_releases(
            &ReleaseSearch {
                release: "Dummy".to_string(),
                artist: Some("Portishead".to_string()),
                ..ReleaseSearch::default()
            },
            5,
        )
        .unwrap();
    assert!(results.iter().any(|result| result.id.0 == RELEASE_ID));

    let release = client.lookup_release(RELEASE_ID).unwrap();
    assert_eq!(release.title, "Dummy");
    assert_eq!(release.artist, "Portishead");
    assert_eq!(release.tracks.len(), 11);
}
