use std::fmt::Display;

use serde::Serialize;

use crate::activity::truncate;

pub const NOTICE_LIMIT: usize = 200;

#[derive(Serialize)]
pub struct Notice {
    #[serde(skip_serializing_if = "Option::is_none")]
    notice: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

impl From<Result<String, String>> for Notice {
    fn from(outcome: Result<String, String>) -> Self {
        match outcome {
            Ok(text) => Self {
                notice: Some(truncate(&text, NOTICE_LIMIT)),
                error: None,
            },
            Err(text) => Self {
                notice: None,
                error: Some(truncate(&text, NOTICE_LIMIT)),
            },
        }
    }
}

/// # Errors
/// Returns the text of the error when the result is an error.
pub fn describe<T, E: Display>(
    result: Result<T, E>,
    ok: impl FnOnce(T) -> String,
) -> Result<String, String> {
    result.map(ok).map_err(|error| error.to_string())
}

/// # Errors
/// Returns the text of the error when the result is an error.
pub fn done<T, E: Display>(result: Result<T, E>, ok: &str) -> Result<String, String> {
    describe(result, |_| ok.to_owned())
}

#[cfg(test)]
mod tests {
    use super::{NOTICE_LIMIT, Notice, describe, done};

    #[test]
    fn a_result_becomes_a_notice_or_an_error_text() {
        assert_eq!(done(Ok::<_, String>(1), "Saved"), Ok("Saved".to_owned()));
        assert_eq!(
            done(Err::<(), _>(std::io::Error::other("disk full")), "Saved"),
            Err("disk full".to_owned())
        );
        assert_eq!(
            describe(Ok::<_, String>(4), |count| format!("Queued {count}")),
            Ok("Queued 4".to_owned())
        );
    }

    #[test]
    fn a_long_text_is_cut_to_the_limit() {
        let notice = Notice::from(Err("x".repeat(NOTICE_LIMIT * 2)));
        let text = serde_json::to_value(&notice).unwrap();
        assert_eq!(
            text["error"].as_str().unwrap().chars().count(),
            NOTICE_LIMIT + 1
        );
        assert!(text.get("notice").is_none());
    }
}
