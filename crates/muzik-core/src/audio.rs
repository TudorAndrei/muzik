use std::path::Path;

pub const EXTENSIONS: &[&str] = &[
    "mp3", "flac", "m4a", "mp4", "opus", "ogg", "wav", "aiff", "aif", "ape", "wv", "aac", "alac",
    "mpc", "spx",
];

pub fn is_audio(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| EXTENSIONS.contains(&extension.to_ascii_lowercase().as_str()))
}

pub fn is_lossless_codec(codec: &str) -> bool {
    matches!(codec, "flac" | "alac" | "ape" | "wavpack" | "tta")
        || codec.starts_with("pcm_")
        || codec.starts_with("dsd_")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_files_include_ogg_and_aiff() {
        assert!(is_audio(Path::new("a/b.OGG")));
        assert!(is_audio(Path::new("x.aiff")));
        assert!(is_audio(Path::new("x.flac")));
        assert!(!is_audio(Path::new("x.jpg")));
        assert!(!is_audio(Path::new("x")));
        assert!(!is_audio(Path::new("x.chapters.txt")));
    }

    #[test]
    fn lossless_codecs_cover_pcm_and_dsd() {
        for codec in [
            "flac",
            "pcm_s16be",
            "pcm_s24le",
            "dsd_lsbf_planar",
            "wavpack",
            "tta",
        ] {
            assert!(is_lossless_codec(codec), "{codec}");
        }
        for codec in ["mp3", "aac", "opus", "vorbis"] {
            assert!(!is_lossless_codec(codec), "{codec}");
        }
    }
}
