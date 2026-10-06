use crate::SqlValue;
use std::path::{Path, PathBuf};

pub fn path_from_sql(value: &SqlValue) -> Option<PathBuf> {
    match value {
        SqlValue::Blob(bytes) => {
            #[cfg(unix)]
            {
                use std::os::unix::ffi::OsStringExt;
                Some(std::ffi::OsString::from_vec(bytes.clone()).into())
            }
            #[cfg(not(unix))]
            {
                Some(PathBuf::from(String::from_utf8_lossy(bytes).into_owned()))
            }
        }
        SqlValue::Text(text) => Some(PathBuf::from(text)),
        _ => None,
    }
}

pub fn path_to_sql(path: &Path) -> SqlValue {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        SqlValue::Blob(path.as_os_str().as_bytes().to_vec())
    }
    #[cfg(not(unix))]
    {
        SqlValue::Blob(path.to_string_lossy().as_bytes().to_vec())
    }
}

pub fn scalar_text(value: &SqlValue) -> Option<String> {
    match value {
        SqlValue::Text(text) => Some(text.clone()),
        SqlValue::Integer(number) => Some(number.to_string()),
        SqlValue::Real(number) => Some(number.to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_survives_the_round_trip_through_sql() {
        let path = Path::new("/music/Artist/Song.flac");
        assert_eq!(path_from_sql(&path_to_sql(path)), Some(path.to_path_buf()));
    }

    #[test]
    fn text_values_decode_as_paths() {
        assert_eq!(
            path_from_sql(&SqlValue::Text("a/b".into())),
            Some(PathBuf::from("a/b"))
        );
    }

    #[test]
    fn scalar_text_reads_numbers_but_not_null() {
        assert_eq!(scalar_text(&SqlValue::Integer(3)), Some("3".to_owned()));
        assert_eq!(scalar_text(&SqlValue::Null), None);
    }
}
