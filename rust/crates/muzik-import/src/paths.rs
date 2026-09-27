//! Evaluation of beets path templates before filesystem sanitization.

use std::collections::{BTreeMap, HashSet};

pub type Fields = BTreeMap<String, String>;

#[derive(Clone, Debug, Default)]
pub struct AlbumFields {
    pub id: i64,
    pub fields: Fields,
}

#[derive(Clone, Debug, Default)]
pub struct TemplateContext {
    pub fields: Fields,
    pub album_id: Option<i64>,
    pub albums: Vec<AlbumFields>,
    pub aunique_keys: Vec<String>,
    pub aunique_disambiguators: Vec<String>,
    pub aunique_bracket: String,
}

impl TemplateContext {
    #[must_use]
    pub fn render(&self, template: &str) -> String {
        render_expression(template, self)
    }

    fn aunique(&self, args: &[String]) -> String {
        let Some(id) = self.album_id else {
            return String::new();
        };
        let Some(album) = self.albums.iter().find(|album| album.id == id) else {
            return String::new();
        };
        let keys = args
            .first()
            .filter(|value| !value.is_empty())
            .map(|value| value.split_whitespace().collect::<Vec<_>>())
            .unwrap_or_else(|| self.aunique_keys.iter().map(String::as_str).collect());
        let disambiguators = args
            .get(1)
            .filter(|value| !value.is_empty())
            .map(|value| value.split_whitespace().collect::<Vec<_>>())
            .unwrap_or_else(|| {
                self.aunique_disambiguators
                    .iter()
                    .map(String::as_str)
                    .collect()
            });
        let bracket = args
            .get(2)
            .map(String::as_str)
            .unwrap_or(&self.aunique_bracket);
        let duplicates: Vec<_> = self
            .albums
            .iter()
            .filter(|candidate| {
                keys.iter().all(|key| {
                    candidate.fields.get(*key).map(String::as_str)
                        == album.fields.get(*key).map(String::as_str)
                })
            })
            .collect();
        if duplicates.len() <= 1 {
            return String::new();
        }
        let (left, right) = if bracket.chars().count() == 2 {
            let mut chars = bracket.chars();
            (
                chars.next().unwrap().to_string(),
                chars.next().unwrap().to_string(),
            )
        } else {
            (String::new(), String::new())
        };
        for key in disambiguators {
            let values: HashSet<_> = duplicates
                .iter()
                .map(|candidate| candidate.fields.get(key).map(String::as_str).unwrap_or(""))
                .collect();
            if values.len() == duplicates.len() {
                let value = album.fields.get(key).map(String::as_str).unwrap_or("");
                return if value.is_empty() {
                    String::new()
                } else {
                    format!(" {left}{value}{right}")
                };
            }
        }
        format!(" {left}{id}{right}")
    }
}

fn is_identifier(character: char) -> bool {
    character == '_' || character.is_alphanumeric()
}

fn render_expression(input: &str, context: &TemplateContext) -> String {
    let mut output = String::new();
    let mut cursor = 0;
    while cursor < input.len() {
        let current = input[cursor..].chars().next().unwrap();
        if current == '$' {
            let next = cursor + 1;
            if let Some(escaped) = input[next..].chars().next()
                && matches!(escaped, '$' | '%' | '}' | ',')
            {
                output.push(escaped);
                cursor = next + escaped.len_utf8();
                continue;
            }
            if input[next..].starts_with('{') {
                if let Some(close) = input[next + 1..].find('}') {
                    let end = next + 1 + close;
                    let key = &input[next + 1..end];
                    if !key.is_empty() {
                        output.push_str(
                            context
                                .fields
                                .get(key)
                                .map(String::as_str)
                                .unwrap_or(&input[cursor..=end]),
                        );
                        cursor = end + 1;
                        continue;
                    }
                }
            } else {
                let end = next
                    + input[next..]
                        .char_indices()
                        .take_while(|(_, character)| is_identifier(*character))
                        .map(|(_, character)| character.len_utf8())
                        .sum::<usize>();
                if end > next {
                    let key = &input[next..end];
                    output.push_str(
                        context
                            .fields
                            .get(key)
                            .map(String::as_str)
                            .unwrap_or(&input[cursor..end]),
                    );
                    cursor = end;
                    continue;
                }
            }
        } else if current == '%' {
            let start = cursor + 1;
            let end = start
                + input[start..]
                    .char_indices()
                    .take_while(|(_, character)| is_identifier(*character))
                    .map(|(_, character)| character.len_utf8())
                    .sum::<usize>();
            if end > start
                && input[end..].starts_with('{')
                && let Some(close) = matching_brace(input, end)
            {
                let name = &input[start..end];
                let args = split_arguments(&input[end + 1..close])
                    .into_iter()
                    .map(|value| render_expression(value, context))
                    .collect::<Vec<_>>();
                if let Some(value) = call(name, &args, context) {
                    output.push_str(&value);
                } else {
                    output.push_str(&input[cursor..=close]);
                }
                cursor = close + 1;
                continue;
            }
        }
        output.push(current);
        cursor += current.len_utf8();
    }
    output
}

fn matching_brace(input: &str, open: usize) -> Option<usize> {
    let mut depth = 0;
    let mut escaped = false;
    for (offset, character) in input[open..].char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if character == '$'
            && input[open + offset + 1..]
                .chars()
                .next()
                .is_some_and(|next| matches!(next, '$' | '%' | '}' | ','))
        {
            escaped = true;
            continue;
        }
        match character {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(open + offset);
                }
            }
            _ => {}
        }
    }
    None
}

fn split_arguments(input: &str) -> Vec<&str> {
    if input.is_empty() {
        return Vec::new();
    }
    let mut args = Vec::new();
    let mut depth = 0;
    let mut start = 0;
    let mut escaped = false;
    for (offset, character) in input.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if character == '$'
            && input[offset + 1..]
                .chars()
                .next()
                .is_some_and(|next| matches!(next, '$' | '%' | '}' | ','))
        {
            escaped = true;
            continue;
        }
        match character {
            '{' => depth += 1,
            '}' => depth -= 1,
            ',' if depth == 0 => {
                args.push(&input[start..offset]);
                start = offset + 1;
            }
            _ => {}
        }
    }
    args.push(&input[start..]);
    args
}

fn call(name: &str, args: &[String], context: &TemplateContext) -> Option<String> {
    let first = args.first().map(String::as_str).unwrap_or("");
    let second = args.get(1).map(String::as_str).unwrap_or("");
    let result = match name {
        "if" => {
            let condition = first.trim();
            let truth = !condition.is_empty()
                && condition != "0"
                && !condition.eq_ignore_ascii_case("false");
            if truth {
                second
            } else {
                args.get(2).map(String::as_str).unwrap_or("")
            }
            .to_string()
        }
        "left" => first
            .chars()
            .take(second.trim().parse::<usize>().ok()?)
            .collect(),
        "right" => {
            let count = second.trim().parse::<usize>().ok()?;
            first
                .chars()
                .rev()
                .take(count)
                .collect::<String>()
                .chars()
                .rev()
                .collect()
        }
        "lower" => first.to_lowercase(),
        "upper" => first.to_uppercase(),
        "title" => first
            .split_whitespace()
            .map(capitalize)
            .collect::<Vec<_>>()
            .join(" "),
        "asciify" => deunicode::deunicode(first),
        "aunique" => context.aunique(args),
        _ => return None,
    };
    Some(result)
}

fn capitalize(value: &str) -> String {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return String::new();
    };
    format!("{}{}", first.to_uppercase(), chars.as_str().to_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    use serde_json::Value;

    #[test]
    fn renders_fields_and_nested_functions() {
        let context = TemplateContext {
            fields: Fields::from([
                ("artist".into(), "Björk".into()),
                ("title".into(), "hidden place".into()),
            ]),
            ..Default::default()
        };
        assert_eq!(
            context.render("%asciify{$artist}/%title{$title}"),
            "Bjork/Hidden Place"
        );
        assert_eq!(
            context.render("%if{$artist,%left{$title,6},none}"),
            "hidden"
        );
        assert_eq!(context.render("%if{${artist},yes,no}"), "yes");
        assert_eq!(context.render("${unknown}/$$artist"), "${unknown}/$artist");
    }

    #[test]
    fn aunique_uses_first_field_that_separates_albums() {
        let albums = vec![
            AlbumFields {
                id: 1,
                fields: Fields::from([
                    ("albumartist".into(), "Artist".into()),
                    ("album".into(), "Album".into()),
                    ("year".into(), "2001".into()),
                ]),
            },
            AlbumFields {
                id: 2,
                fields: Fields::from([
                    ("albumartist".into(), "Artist".into()),
                    ("album".into(), "Album".into()),
                    ("year".into(), "2002".into()),
                ]),
            },
        ];
        let context = TemplateContext {
            album_id: Some(1),
            albums,
            aunique_keys: vec!["albumartist".into(), "album".into()],
            aunique_disambiguators: vec!["year".into()],
            aunique_bracket: "[]".into(),
            ..Default::default()
        };
        assert_eq!(context.render("Album%aunique{}"), "Album [2001]");
    }

    #[test]
    fn configured_paths_match_beets_destinations() {
        let fixture: Value = serde_json::from_str(include_str!("../tests/fixtures/paths.json"))
            .expect("valid fixture");
        assert_eq!(fixture["beets_version"], "2.13.1");
        let cases = fixture["cases"].as_array().unwrap();
        let albums = cases
            .iter()
            .filter_map(|case| {
                Some(AlbumFields {
                    id: case["album_id"].as_i64()?,
                    fields: serde_json::from_value(case["fields"].clone()).ok()?,
                })
            })
            .collect::<Vec<_>>();
        for case in cases {
            let format = match case["path_format"].as_str().unwrap() {
                "default" => "$albumartist/$album%aunique{}/$track $title",
                "comp" => "Compilations/$album%aunique{}/$track $title",
                "singleton" => "Non-Album/$artist/$title",
                other => panic!("unknown path format: {other}"),
            };
            let context = TemplateContext {
                fields: serde_json::from_value(case["fields"].clone()).unwrap(),
                album_id: case["album_id"].as_i64(),
                albums: albums.clone(),
                aunique_keys: vec!["albumartist".into(), "album".into()],
                aunique_disambiguators: vec!["year".into()],
                aunique_bracket: "[]".into(),
            };
            assert_eq!(
                format!("{}.flac", context.render(format)),
                case["destination"].as_str().unwrap(),
                "{}",
                case["name"]
            );
        }
    }
}
