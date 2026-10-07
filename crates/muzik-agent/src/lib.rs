use muzik_core::DecisionKind;
use muzik_media::process::{self, Stopped};
use serde_json::{Value, json};
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::AtomicBool;
use std::time::Duration;
use strum_macros::EnumString;

pub const DEFAULT_MODEL: &str = "gpt-6-luna";
const STRONG_DISTANCE: f64 = 0.10;
const MIN_CONFIDENCE: f64 = 0.65;
const TIMEOUT: Duration = Duration::from_secs(120);
const MAX_FILES: usize = 25;

const SCHEMA: &str = r#"{"type":"object","additionalProperties":false,"required":["action","index","confidence","reason"],"properties":{"action":{"type":"string","enum":["pick","keep","ask"]},"index":{"type":"integer"},"confidence":{"type":"number"},"reason":{"type":"string"}}}"#;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Message(String),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

impl From<String> for Error {
    fn from(message: String) -> Self {
        Self::Message(message)
    }
}

impl From<&str> for Error {
    fn from(message: &str) -> Self {
        Self::Message(message.to_owned())
    }
}

impl From<Error> for String {
    fn from(error: Error) -> Self {
        error.to_string()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Choice {
    pub value: Value,
    pub label: String,
    pub confidence: f64,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    Decided(Choice),
    Unsure {
        suggestion: Option<usize>,
        confidence: f64,
        reason: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, EnumString)]
#[strum(serialize_all = "snake_case")]
enum Action {
    Pick,
    Keep,
    Ask,
}

#[must_use]
pub const fn supports(kind: DecisionKind) -> bool {
    matches!(
        kind,
        DecisionKind::ImportMatch | DecisionKind::SoulseekCandidate
    )
}

/// # Errors
/// Returns an error when there are no candidates, or when Codex cannot run or gives no valid answer.
pub fn decide(kind: DecisionKind, payload: &Value, model: &str) -> Result<Outcome> {
    if let Some(choice) = strong_match(kind, payload) {
        return Ok(Outcome::Decided(choice));
    }
    let options = options(kind, payload);
    if options.is_empty() {
        return Err("There are no candidates to choose from.".into());
    }
    let answer = run_codex(&prompt(kind, payload, &options), model)?;
    Ok(interpret(kind, &answer, &options))
}

pub fn strong_match(kind: DecisionKind, payload: &Value) -> Option<Choice> {
    if kind != DecisionKind::ImportMatch {
        return None;
    }
    let best = payload
        .pointer("/task/matches")
        .and_then(Value::as_array)?
        .iter()
        .filter_map(|item| Some((item, item["distance"].as_f64()?)))
        .min_by(|left, right| left.1.total_cmp(&right.1))?;
    (best.1 <= STRONG_DISTANCE).then(|| Choice {
        value: best.0["candidate_id"].clone(),
        label: release_label(best.0),
        confidence: 1.0,
        reason: format!("Distance {:.3} is a close match.", best.1),
    })
}

pub fn options(kind: DecisionKind, payload: &Value) -> Vec<(String, Value)> {
    match kind {
        DecisionKind::ImportMatch => payload
            .pointer("/task/matches")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|item| item["candidate_id"].is_string())
            .map(|item| (release_label(item), item["candidate_id"].clone()))
            .collect(),
        DecisionKind::SoulseekCandidate => payload["candidates"]
            .as_array()
            .into_iter()
            .flatten()
            .enumerate()
            .map(|(index, item)| {
                (
                    item["title"].as_str().unwrap_or("Candidate").to_owned(),
                    json!(index),
                )
            })
            .collect(),
        _ => Vec::new(),
    }
}

pub fn prompt(kind: DecisionKind, payload: &Value, options: &[(String, Value)]) -> String {
    let mut text = String::new();
    if kind == DecisionKind::ImportMatch {
        let task = &payload["task"];
        let paths: Vec<&str> = task["paths"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect();
        let folder = paths
            .first()
            .and_then(|path| Path::new(path).parent())
            .and_then(Path::file_name)
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let _ = writeln!(
            text,
            "You choose the release for one album in a music tagging tool."
        );
        let _ = writeln!(text, "Source folder: {folder}");
        let _ = writeln!(
            text,
            "Current tags: {} · {} · {}",
            text_or(&task["current_artist"], "unknown artist"),
            text_or(&task["current_album"], "unknown album"),
            text_or(&task["current_year"], "unknown year")
        );
        let _ = writeln!(text, "Files ({}):", paths.len());
        for path in paths.iter().take(MAX_FILES) {
            let name = Path::new(path)
                .file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
                .unwrap_or_default();
            let _ = writeln!(text, "- {name}");
        }
        if let Some(more) = paths.len().checked_sub(MAX_FILES).filter(|more| *more > 0) {
            let _ = writeln!(text, "- and {more} more");
        }
        let _ = writeln!(
            text,
            "Candidates (distance 0.0 is a perfect match, 1.0 is a poor match):"
        );
        for (index, item) in task["matches"].as_array().into_iter().flatten().enumerate() {
            let _ = writeln!(
                text,
                "[{index}] {} · distance {:.3}",
                release_details(item),
                item["distance"].as_f64().unwrap_or(1.0)
            );
        }
        let _ = writeln!(
            text,
            "Use action \"pick\" with the index of the candidate that is the same release. Use \"keep\" when no candidate fits and the current tags are right. Use \"ask\" when you are not sure."
        );
    } else {
        let _ = writeln!(
            text,
            "You choose one Soulseek download for a music tool. The wanted music is: {}",
            text_or(&payload["query"], "unknown")
        );
        let _ = writeln!(
            text,
            "Candidates (a higher score is a better match; prefer lossless and complete albums):"
        );
        for (index, item) in payload["candidates"]
            .as_array()
            .into_iter()
            .flatten()
            .enumerate()
        {
            let _ = writeln!(
                text,
                "[{index}] {} · {} · {} files · user {} · score {:.0}",
                text_or(&item["path"], item["title"].as_str().unwrap_or("")),
                text_or(
                    item.pointer("/quality/format").unwrap_or(&Value::Null),
                    "unknown format"
                ),
                item["files"].as_array().map_or(0, Vec::len),
                text_or(&item["user"], "unknown"),
                item["score"].as_f64().unwrap_or(0.0)
            );
        }
        let _ = writeln!(
            text,
            "Use action \"pick\" with the index of the best download for the wanted music. Use \"ask\" when none fits or you are not sure."
        );
    }
    let _ = writeln!(
        text,
        "Use index -1 unless the action is \"pick\". Choose an index from 0 to {}. Never invent an index. Give confidence from 0.0 to 1.0 and a one-sentence reason.",
        options.len().saturating_sub(1)
    );
    text
}

#[must_use]
pub fn interpret(kind: DecisionKind, answer: &Value, options: &[(String, Value)]) -> Outcome {
    let confidence = answer["confidence"].as_f64().unwrap_or(0.0).clamp(0.0, 1.0);
    let reason = answer["reason"].as_str().unwrap_or("").trim().to_owned();
    let index = answer["index"]
        .as_i64()
        .and_then(|index| usize::try_from(index).ok())
        .filter(|index| *index < options.len());
    let confident = confidence >= MIN_CONFIDENCE;
    let action = answer["action"]
        .as_str()
        .and_then(|action| action.parse::<Action>().ok());
    let picked = index.and_then(|index| options.get(index));
    match (action, index, picked) {
        (Some(Action::Pick), _, Some((label, value))) if confident => Outcome::Decided(Choice {
            value: value.clone(),
            label: label.clone(),
            confidence,
            reason,
        }),
        (Some(Action::Keep), _, _) if confident && kind == DecisionKind::ImportMatch => {
            Outcome::Decided(Choice {
                value: json!("as_is"),
                label: "Keep current tags".into(),
                confidence,
                reason,
            })
        }
        (Some(Action::Pick), suggestion, _) => Outcome::Unsure {
            suggestion,
            confidence,
            reason,
        },
        (Some(Action::Keep | Action::Ask) | None, _, _) => Outcome::Unsure {
            suggestion: None,
            confidence,
            reason,
        },
    }
}

fn run_codex(prompt: &str, model: &str) -> Result<Value> {
    let directory = tempfile::tempdir()?;
    let schema = directory.path().join("schema.json");
    let answer = directory.path().join("answer.json");
    std::fs::write(&schema, SCHEMA)?;
    let log_path = directory.path().join("codex.log");
    let log = std::fs::File::create(&log_path)?;
    let mut command = Command::new("codex");
    command
        .args([
            "exec",
            "--skip-git-repo-check",
            "--sandbox",
            "read-only",
            "-m",
            model,
        ])
        .args(["-c", "model_reasoning_effort=low", "--output-schema"])
        .arg(&schema)
        .arg("-o")
        .arg(&answer)
        .arg("-")
        .current_dir(directory.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(log);
    let mut child = process::spawn(command).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            "Codex is not installed.".to_owned()
        } else {
            format!("Cannot start Codex: {error}")
        }
    })?;
    if let Some(mut stdin) = child.stdin().take() {
        stdin
            .write_all(prompt.as_bytes())
            .map_err(|error| format!("Cannot send the question to Codex: {error}"))?;
    }
    let status = match process::wait(&mut child, Some(TIMEOUT), &AtomicBool::new(false)) {
        Ok(status) => status,
        Err(Stopped::TimedOut | Stopped::Cancelled) => {
            return Err("Codex did not answer within 2 minutes.".into());
        }
        Err(Stopped::Io(error)) => return Err(error.into()),
    };
    if !status.success() {
        let stderr = std::fs::read_to_string(&log_path).unwrap_or_default();
        let line = stderr
            .lines()
            .rev()
            .find(|line| line.contains("ERROR") || line.contains("error"))
            .unwrap_or("Codex failed.");
        return Err(line.trim().into());
    }
    let text = std::fs::read_to_string(&answer).map_err(|_| "Codex gave no answer.")?;
    serde_json::from_str(&text).map_err(|_| "Codex gave an answer that is not valid JSON.".into())
}

fn release_label(item: &Value) -> String {
    format!(
        "{} — {}",
        text_or(&item["artist"], "Unknown artist"),
        text_or(&item["album"], "Unknown album")
    )
}

fn release_details(item: &Value) -> String {
    let mut parts = vec![release_label(item)];
    for key in ["year", "country", "media", "label"] {
        let value = text_or(&item[key], "");
        if !value.is_empty() {
            parts.push(value);
        }
    }
    if let Some(tracks) = item["track_count"].as_u64() {
        parts.push(format!("{tracks} tracks"));
    }
    parts.join(" · ")
}

fn text_or(value: &Value, fallback: &str) -> String {
    match value {
        Value::String(text) if !text.trim().is_empty() => text.clone(),
        Value::Number(number) => number.to_string(),
        _ => fallback.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::{Outcome, interpret, options, prompt, strong_match};
    use muzik_core::DecisionKind;
    use serde_json::json;

    fn album() -> serde_json::Value {
        json!({"task": {
            "paths": ["/splits/Sunburst (1980)/01-sunburst.opus", "/splits/Sunburst (1980)/02-cool-k.opus"],
            "current_artist": "sam-jam", "current_album": "Sunburst", "current_year": null,
            "matches": [
                {"candidate_id": "m0", "artist": "Sunburst", "album": "Sunburst", "year": 1980, "country": "JP", "track_count": 5, "distance": 0.31},
                {"candidate_id": "m1", "artist": "LOUDNESS", "album": "Sunburst~Gamushara", "year": 1981, "distance": 0.55}
            ]
        }})
    }

    #[test]
    #[ignore = "calls the real Codex CLI and uses the account quota"]
    fn live_codex_picks_the_same_release() {
        let outcome =
            super::decide(DecisionKind::ImportMatch, &album(), super::DEFAULT_MODEL).unwrap();
        assert!(
            matches!(&outcome, Outcome::Decided(choice) if choice.value == json!("m0")),
            "{outcome:?}"
        );
    }

    #[test]
    fn close_match_is_chosen_without_the_model() {
        let mut payload = album();
        payload["task"]["matches"][1]["distance"] = json!(0.08);
        let choice = strong_match(DecisionKind::ImportMatch, &payload);
        assert_eq!(choice.map(|choice| choice.value), Some(json!("m1")));
        assert!(strong_match(DecisionKind::ImportMatch, &album()).is_none());
    }

    #[test]
    fn prompt_names_the_folder_files_and_numbered_candidates() {
        let payload = album();
        let text = prompt(
            DecisionKind::ImportMatch,
            &payload,
            &options(DecisionKind::ImportMatch, &payload),
        );
        assert!(text.contains("Source folder: Sunburst (1980)"));
        assert!(text.contains("- 01-sunburst"));
        assert!(text.contains("[0] Sunburst — Sunburst · 1980 · JP · 5 tracks · distance 0.310"));
        assert!(text.contains("from 0 to 1"));
    }

    #[test]
    fn confident_pick_decides_and_low_confidence_asks() {
        let payload = album();
        let options = options(DecisionKind::ImportMatch, &payload);
        let picked = interpret(
            DecisionKind::ImportMatch,
            &json!({"action": "pick", "index": 0, "confidence": 0.9, "reason": "Same release."}),
            &options,
        );
        assert!(matches!(picked, Outcome::Decided(choice) if choice.value == json!("m0")));
        let unsure = interpret(
            DecisionKind::ImportMatch,
            &json!({"action": "pick", "index": 1, "confidence": 0.4, "reason": "Maybe."}),
            &options,
        );
        assert_eq!(
            unsure,
            Outcome::Unsure {
                suggestion: Some(1),
                confidence: 0.4,
                reason: "Maybe.".into()
            }
        );
    }

    #[test]
    fn invented_index_and_keep_for_downloads_ask_the_user() {
        let payload = album();
        let choices = options(DecisionKind::ImportMatch, &payload);
        let invented = interpret(
            DecisionKind::ImportMatch,
            &json!({"action": "pick", "index": 7, "confidence": 0.99, "reason": ""}),
            &choices,
        );
        assert!(matches!(
            invented,
            Outcome::Unsure {
                suggestion: None,
                ..
            }
        ));
        let keep = interpret(
            DecisionKind::ImportMatch,
            &json!({"action": "keep", "index": -1, "confidence": 0.8, "reason": "No match."}),
            &choices,
        );
        assert!(matches!(keep, Outcome::Decided(choice) if choice.value == json!("as_is")));
        let download = json!({"query": "Sunburst", "candidates": [{"title": "a.flac"}]});
        let keep_download = interpret(
            DecisionKind::SoulseekCandidate,
            &json!({"action": "keep", "index": -1, "confidence": 0.9, "reason": ""}),
            &options(DecisionKind::SoulseekCandidate, &download),
        );
        assert!(matches!(keep_download, Outcome::Unsure { .. }));
    }
}
