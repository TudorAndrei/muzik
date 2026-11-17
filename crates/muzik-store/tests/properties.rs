use muzik_store::watchlist::{ItemId, SourceKind, parse_source};
use proptest::prelude::*;

proptest! {
    #[test]
    fn an_item_id_parses_back_from_its_text(
        playlist_id in ".{0,10}",
        position in any::<u64>(),
        video_id in proptest::option::of("[^:]{1,11}"),
    ) {
        let id = ItemId::new(&playlist_id, position, video_id.as_deref());
        prop_assert_eq!(id.to_string().parse::<ItemId>()?, id);
    }

    #[test]
    fn a_youtube_playlist_keeps_its_id(id in "[A-Za-z0-9_-]{1,34}") {
        let playlist = parse_source(&format!("https://www.youtube.com/playlist?list={id}"))?;
        prop_assert_eq!(&playlist.playlist_id, &id);
        prop_assert_eq!(playlist.kind, SourceKind::Youtube);
        prop_assert_eq!(parse_source(&playlist.url)?.playlist_id, id);
    }

    #[test]
    fn every_spotify_form_names_the_same_source(
        id in "[A-Za-z0-9]{1,22}",
        kind in prop_oneof![Just("playlist"), Just("album")],
        locale in proptest::option::of("intl-[a-z]{2}/"),
    ) {
        let expected = format!("spotify:{kind}:{id}");
        let locale = locale.unwrap_or_default();
        for input in [
            expected.clone(),
            format!("https://open.spotify.com/{kind}/{id}"),
            format!("https://open.spotify.com/{locale}{kind}/{id}/"),
        ] {
            let playlist = parse_source(&input)?;
            prop_assert_eq!(&playlist.playlist_id, &expected);
            prop_assert_eq!(playlist.kind, SourceKind::Spotify);
            prop_assert_eq!(&parse_source(&playlist.url)?.playlist_id, &expected);
        }
    }
}
