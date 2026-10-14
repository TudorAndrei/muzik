//! Shared download sidecar lookup and metadata precedence.

use serde_json::Value;
use std::path::Path;
use std::{fs, io};

/// Preserve the existing import and split contracts at their input boundaries.
#[derive(Clone, Copy)]
pub enum Policy {
    /// Require an object, report read errors, and skip blank/zero field values.
    Import,
    /// Accept any JSON, ignore read errors, and retain whitespace and zero.
    Split,
}

/// Read the track sidecar before the folder sidecar. Invalid JSON is skipped.
///
/// # Errors
/// Import reports read errors other than missing files; Split ignores them.
pub fn read(source: &Path, policy: Policy) -> io::Result<Option<Value>> {
    for candidate in [
        source.with_extension("muzik.json"),
        source
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(".muzik.json"),
    ] {
        let contents = match fs::read_to_string(candidate) {
            Ok(contents) => contents,
            Err(error)
                if error.kind() == io::ErrorKind::NotFound || matches!(policy, Policy::Split) =>
            {
                continue;
            }
            Err(error) => return Err(error),
        };
        if let Ok(data) = serde_json::from_str::<Value>(&contents)
            && (matches!(policy, Policy::Split) || data.is_object())
        {
            return Ok(Some(data));
        }
    }
    Ok(None)
}

/// Select resolved metadata, candidate metadata, root fields, then candidate fields.
/// Within each source, keys are tried in the caller's order.
#[must_use]
pub fn value<'a>(data: &'a Value, keys: &[&str], policy: Policy) -> Option<&'a Value> {
    [
        data.get("resolved"),
        data.get("candidate")
            .and_then(|candidate| candidate.get("metadata")),
        Some(data),
        data.get("candidate"),
    ]
    .into_iter()
    .flatten()
    .find_map(|source| {
        keys.iter()
            .filter_map(|key| source.get(key))
            .find(|value| match policy {
                Policy::Import => match value {
                    Value::String(text) => !text.trim().is_empty(),
                    Value::Number(number) => number.as_i64() != Some(0),
                    Value::Bool(value) => *value,
                    Value::Null => false,
                    _ => true,
                },
                Policy::Split => {
                    matches!(value, Value::String(text) if !text.is_empty()) || value.is_number()
                }
            })
    })
}

#[must_use]
pub fn text(data: &Value, keys: &[&str], policy: Policy) -> Option<String> {
    match value(data, keys, policy)? {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) if matches!(policy, Policy::Split) => Some(number.to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn metadata_precedence_preserves_import_and_split_value_rules() {
        let data = json!({
            "resolved": {"artist": "  ", "year": 0},
            "candidate": {"metadata": {"artist": "Metadata", "year": 2001}, "album": "Candidate"},
            "album": "Root"
        });
        assert_eq!(
            text(&data, &["artist"], Policy::Import).as_deref(),
            Some("Metadata")
        );
        assert_eq!(value(&data, &["year"], Policy::Import), Some(&json!(2001)));
        assert_eq!(
            text(&data, &["artist"], Policy::Split).as_deref(),
            Some("  ")
        );
        assert_eq!(text(&data, &["year"], Policy::Split).as_deref(), Some("0"));
        for policy in [Policy::Import, Policy::Split] {
            assert_eq!(text(&data, &["album"], policy).as_deref(), Some("Root"));
        }
    }

    #[test]
    fn sidecar_fallback_preserves_file_and_error_rules() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("track.mp3");
        let track = source.with_extension("muzik.json");
        let folder = directory.path().join(".muzik.json");
        fs::write(&folder, r#"{"artist":"Folder"}"#).unwrap();
        fs::write(&track, "null").unwrap();
        let imported = read(&source, Policy::Import).unwrap().unwrap();
        assert_eq!(
            text(&imported, &["artist"], Policy::Import).as_deref(),
            Some("Folder")
        );
        assert_eq!(read(&source, Policy::Split).unwrap(), Some(Value::Null));
        fs::write(&track, "invalid json").unwrap();
        assert_eq!(
            read(&source, Policy::Split).unwrap(),
            Some(imported.clone())
        );
        fs::remove_file(&track).unwrap();
        fs::create_dir(&track).unwrap();
        assert!(read(&source, Policy::Import).is_err());
        assert_eq!(read(&source, Policy::Split).unwrap(), Some(imported));
    }
}
