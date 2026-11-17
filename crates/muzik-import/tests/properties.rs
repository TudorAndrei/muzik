use muzik_import::ftclean::clean;
use muzik_import::paths::{Fields, TemplateContext};
use proptest::prelude::*;

fn credited_title() -> impl Strategy<Value = String> {
    "[A-Za-z ]{1,8}( [\\(\\[](feat|ft|featuring)\\.? [A-Za-z&, ]{1,8}[\\)\\]]){0,2}( (feat|ft)\\. [A-Za-z]{1,6})?"
}

proptest! {
    #[test]
    fn templates_without_markers_render_unchanged(template in "[^$%]*") {
        prop_assert_eq!(TemplateContext::default().render(&template), template);
    }

    #[test]
    fn escaped_text_renders_back_to_itself(text in any::<String>()) {
        let escaped = text.replace('$', "$$").replace('%', "$%");
        prop_assert_eq!(TemplateContext::default().render(&escaped), text);
    }

    #[test]
    fn rendered_fields_never_add_a_path_separator(
        key in "[a-z_][a-z0-9_]{0,8}",
        value in any::<String>(),
    ) {
        let context = TemplateContext {
            fields: Fields::from([(key.clone(), value)]),
            ..TemplateContext::default()
        };
        let bare = context.render(&["$", &key].concat());
        let braced = context.render(&["${", &key, "}"].concat());
        prop_assert!(!bare.contains('/'), "{bare:?}");
        prop_assert!(!braced.contains('/'), "{braced:?}");
    }

    #[test]
    fn cleaned_titles_are_trimmed(title in credited_title(), artist in "[A-Za-z ]{0,8}") {
        let (title, _) = clean(&title, &artist);
        prop_assert_eq!(title.trim(), title.as_str());
    }

    #[test]
    fn cleaning_twice_changes_nothing(title in credited_title(), artist in "[A-Za-z ]{0,8}") {
        let (title, artist) = clean(&title, &artist);
        prop_assert_eq!(clean(&title, &artist), (title, artist));
    }

    #[test]
    fn credited_artists_keep_their_existing_credit(
        title in credited_title(),
        artist in "[A-Za-z ]{0,4}(feat|ft\\.)[A-Za-z ]{0,4}",
    ) {
        prop_assert_eq!(clean(&title, &artist).1, artist);
    }

    #[test]
    fn featured_names_move_to_the_artist(title in credited_title(), artist in "[A-Za-z]{1,8}") {
        prop_assume!(!artist.to_ascii_lowercase().contains("feat"));
        let (_, featured) = muzik_core::featured::parse(&title);
        let (_, credited) = clean(&title, &artist);
        prop_assert!(credited.starts_with(&artist));
        for name in featured {
            prop_assert!(credited.contains(&name), "{credited:?} lacks {name:?}");
        }
    }
}
