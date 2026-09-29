use muzik_core::{ChapterAnswer, DecisionKind, DuplicateAnswer, KEEP_CURRENT_TAGS};
use serde_json::{json, Value};

pub struct Choice {
    pub label: String,
    pub meta: String,
    pub score: Option<u64>,
    pub value: Value,
}

impl Choice {
    fn plain(label: &str, value: Value) -> Self {
        Self {
            label: label.to_owned(),
            meta: String::new(),
            score: None,
            value,
        }
    }
}

pub fn kind(question: &Value) -> Option<DecisionKind> {
    question["kind"].as_str()?.parse().ok()
}

pub fn title(kind: Option<DecisionKind>) -> &'static str {
    match kind {
        Some(DecisionKind::SoulseekCandidate) => "Choose a Soulseek download",
        Some(DecisionKind::ChapterReview) => "Check the chapters",
        Some(DecisionKind::ChapterEdit) => "Edit the chapters",
        Some(DecisionKind::QualityReplacement) => "Replace the file with a better one?",
        Some(DecisionKind::ImportMatch) => "Choose the album tags",
        Some(DecisionKind::ImportDuplicate) => "This album is already in the library",
        None => "Choose an option",
    }
}

pub fn note(question: &Value) -> Option<&'static str> {
    let matches = question["payload"]["task"]["matches"]
        .as_array()
        .map_or(0, Vec::len);
    match kind(question)? {
        DecisionKind::ImportMatch if matches == 0 => {
            Some("No online release matches these files. Keep the current tags, or skip the album.")
        }
        DecisionKind::ImportMatch => Some("Pick the release that matches these files."),
        DecisionKind::SoulseekCandidate => Some("The best match is first."),
        _ => None,
    }
}

pub fn agent_note(agent: &Value) -> Option<String> {
    let model = agent["model"].as_str().unwrap_or("The assistant");
    if let Some(error) = agent["error"].as_str() {
        return Some(format!("{model} could not choose: {error}"));
    }
    let reason = agent["reason"].as_str()?;
    let confidence = (agent["confidence"].as_f64().unwrap_or(0.0) * 100.0).round();
    Some(format!("{model} is not sure ({confidence}%): {reason}"))
}

pub fn suggestion(question: &Value) -> Option<usize> {
    question["payload"]["agent"]["suggestion"]
        .as_u64()
        .and_then(|index| usize::try_from(index).ok())
        .or((kind(question) != Some(DecisionKind::ChapterEdit)).then_some(0))
}

pub fn details(question: &Value) -> Vec<String> {
    let payload = &question["payload"];
    let Some(kind) = kind(question) else {
        return Vec::new();
    };
    match kind {
        DecisionKind::SoulseekCandidate => payload["candidates"]
            .as_array()
            .into_iter()
            .flatten()
            .enumerate()
            .map(|(index, candidate)| {
                format!(
                    "{}. {} · score {:.0} · {} · {} · {} files · {}",
                    index + 1,
                    candidate["title"].as_str().unwrap_or("Candidate"),
                    candidate["score"].as_f64().unwrap_or(0.0),
                    candidate["user"].as_str().unwrap_or("Unknown user"),
                    candidate["quality"]["format"]
                        .as_str()
                        .unwrap_or("Unknown format"),
                    candidate["files"].as_array().map_or(0, Vec::len),
                    candidate["path"]
                        .as_str()
                        .or_else(|| candidate["source_id"].as_str())
                        .unwrap_or("")
                )
            })
            .collect(),
        DecisionKind::ChapterReview | DecisionKind::ChapterEdit => {
            let mut details = Vec::new();
            if let Some(source) = payload["source"].as_str() {
                details.push(format!("Source: {source}"));
            }
            if let Some(chapters) = payload["chapters"].as_array() {
                for chapter in chapters {
                    details.push(format!(
                        "{}. {} s to {} s · {}",
                        chapter["index"].as_u64().unwrap_or(0),
                        chapter["start"].as_u64().unwrap_or(0),
                        chapter["end"]
                            .as_u64()
                            .map_or_else(|| "end".to_string(), |value| value.to_string()),
                        chapter["title"].as_str().unwrap_or("Untitled")
                    ));
                }
            }
            details
        }
        DecisionKind::QualityReplacement => vec![
            format!(
                "Current file: {}",
                payload["current"].as_str().unwrap_or("")
            ),
            format!(
                "Candidate: {} · {} · {} kbps",
                payload["candidate"]["title"]
                    .as_str()
                    .unwrap_or("Audio file"),
                payload["candidate"]["quality"]["format"]
                    .as_str()
                    .unwrap_or("Unknown format"),
                payload["candidate"]["quality"]["bitrate"]
                    .as_u64()
                    .map_or_else(|| "?".to_string(), |value| value.to_string())
            ),
        ],
        DecisionKind::ImportMatch | DecisionKind::ImportDuplicate => {
            let task = &payload["task"];
            let mut details = vec![format!(
                "Current tags: {} · {} · {}",
                task["current_artist"].as_str().unwrap_or("Unknown artist"),
                task["current_album"].as_str().unwrap_or("Unknown album"),
                task["current_year"].as_str().unwrap_or("Unknown year")
            )];
            if let Some(paths) = task["paths"].as_array() {
                details.extend(
                    paths
                        .iter()
                        .filter_map(|path| path.as_str().map(str::to_owned)),
                );
            }
            if kind == DecisionKind::ImportDuplicate {
                if let Some(duplicates) = payload["duplicates"].as_array() {
                    details.extend(duplicates.iter().map(|duplicate| {
                        format!(
                            "Existing: {} · {} · {}",
                            duplicate["artist"].as_str().unwrap_or("Unknown artist"),
                            duplicate["album"].as_str().unwrap_or("Unknown album"),
                            duplicate["path"].as_str().unwrap_or("No path")
                        )
                    }));
                }
            }
            details
        }
    }
}

pub fn choices(question: &Value) -> Vec<Choice> {
    let payload = &question["payload"];
    let Some(kind) = kind(question) else {
        return Vec::new();
    };
    match kind {
        DecisionKind::SoulseekCandidate => {
            let candidates: Vec<&Value> = payload["candidates"]
                .as_array()
                .into_iter()
                .flatten()
                .collect();
            let best = candidates
                .iter()
                .filter_map(|candidate| candidate["score"].as_f64())
                .fold(0.0_f64, f64::max);
            let mut choices: Vec<Choice> = candidates
                .iter()
                .enumerate()
                .map(|(index, candidate)| Choice {
                    label: candidate["title"]
                        .as_str()
                        .or_else(|| candidate["name"].as_str())
                        .unwrap_or("Candidate")
                        .to_owned(),
                    meta: joined_facts([
                        fact(&candidate["quality"]["format"]),
                        format!(
                            "{} files",
                            candidate["files"].as_array().map_or(0, Vec::len)
                        ),
                        format!("user {}", fact(&candidate["user"])),
                    ]),
                    score: candidate["score"]
                        .as_f64()
                        .filter(|_| best > 0.0)
                        .map(|score| ((score / best).clamp(0.0, 1.0) * 100.0).round() as u64),
                    value: json!(index),
                })
                .collect();
            choices.push(Choice::plain("Skip these downloads", Value::Null));
            choices
        }
        DecisionKind::ChapterReview => vec![
            Choice::plain("Use these chapters", json!(ChapterAnswer::Accept)),
            Choice::plain("Edit the chapters", json!(ChapterAnswer::Edit)),
            Choice::plain("Do not split", json!(ChapterAnswer::Reject)),
        ],
        DecisionKind::ChapterEdit => vec![
            Choice::plain("Keep original chapters", payload["chapters"].clone()),
            Choice::plain("Cancel chapter edit", Value::Null),
        ],
        DecisionKind::QualityReplacement => vec![
            Choice::plain("Replace file", json!(true)),
            Choice::plain("Keep current file", json!(false)),
        ],
        DecisionKind::ImportMatch => {
            let mut choices: Vec<Choice> = payload["task"]["matches"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|candidate| {
                    let id = candidate["candidate_id"].as_str()?;
                    Some(Choice {
                        label: format!(
                            "{} — {}",
                            candidate["artist"].as_str().unwrap_or("Unknown artist"),
                            candidate["album"]
                                .as_str()
                                .or_else(|| candidate["title"].as_str())
                                .unwrap_or("Unknown release")
                        ),
                        meta: joined_facts([
                            fact(&candidate["year"]),
                            fact(&candidate["country"]),
                            fact(&candidate["media"]),
                            fact(&candidate["label"]),
                            candidate["track_count"]
                                .as_u64()
                                .map_or_else(String::new, |count| format!("{count} tracks")),
                        ]),
                        score: candidate["score"].as_u64().or_else(|| {
                            candidate["distance"].as_f64().map(|distance| {
                                ((1.0 - distance.clamp(0.0, 1.0)) * 100.0).round() as u64
                            })
                        }),
                        value: json!(id),
                    })
                })
                .collect();
            choices.push(Choice::plain("Keep current tags", json!(KEEP_CURRENT_TAGS)));
            choices.push(Choice::plain("Skip", Value::Null));
            choices
        }
        DecisionKind::ImportDuplicate => vec![
            Choice::plain("Skip the new files", json!(DuplicateAnswer::Skip)),
            Choice::plain("Keep both", json!(DuplicateAnswer::KeepAll)),
            Choice::plain("Replace the old files", json!(DuplicateAnswer::RemoveOld)),
        ],
    }
}

fn joined_facts(parts: impl IntoIterator<Item = String>) -> String {
    parts
        .into_iter()
        .filter(|part| !part.trim().is_empty())
        .collect::<Vec<_>>()
        .join(" · ")
}

fn fact(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Number(number) => number.to_string(),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::{choices, details};
    use serde_json::json;

    #[test]
    fn soulseek_review_shows_candidate_quality_and_selects_its_index() {
        let decision = json!({
            "kind": "soulseek_candidate",
            "payload": {"candidates": [{
                "title": "Album", "score": 91.0, "user": "listener",
                "quality": {"format": "FLAC"}, "files": [{"name": "track.flac"}],
                "path": "Music/Album"
            }]}
        });
        let details = details(&decision);
        assert!(details[0].contains("91"));
        assert!(details[0].contains("FLAC"));
        assert!(details[0].contains("Music/Album"));
        let options = choices(&decision);
        assert_eq!(options[0].value, json!(0));
        assert_eq!(options[0].score, Some(100));
        assert!(options[0].meta.contains("FLAC"));
        assert_eq!(
            options.last().map(|option| &option.value),
            Some(&json!(null))
        );
    }

    #[test]
    fn import_duplicate_review_shows_existing_file_and_reply_options() {
        let decision = json!({
            "kind": "import_duplicate",
            "payload": {
                "task": {"current_artist": "Artist", "current_album": "Album", "paths": ["new.flac"]},
                "duplicates": [{"artist": "Artist", "album": "Album", "path": "old.flac"}]
            }
        });
        let details = details(&decision);
        assert!(details.iter().any(|line| line.contains("old.flac")));
        assert!(details.iter().any(|line| line.contains("new.flac")));
        assert_eq!(
            choices(&decision)
                .into_iter()
                .map(|option| option.value)
                .collect::<Vec<_>>(),
            vec![json!("skip"), json!("keep_all"), json!("remove_old")]
        );
    }
}
