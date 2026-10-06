//! Candidate types converted from `soulseek_rs` wire types.

use serde::{Deserialize, Serialize};
use soulseek_rs::{File as WireFile, SearchResult as WireSearchResult};

// Soulseek's FileSearchResponse attribute codes, per
// <https://nicotine-plus.org/doc/SLSKPROTOCOL.html#file-attribute-types>.
// Code 3 (encoder) is obsolete and not decoded.
const ATTRIB_BITRATE_KBPS: u32 = 0;
const ATTRIB_DURATION_SECONDS: u32 = 1;
const ATTRIB_VBR: u32 = 2;
const ATTRIB_SAMPLE_RATE_HZ: u32 = 4;
const ATTRIB_BIT_DEPTH: u32 = 5;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileEntry {
    pub name: String,
    pub size: u64,
    pub bitrate_kbps: Option<u32>,
    pub duration_seconds: Option<u32>,
    pub vbr: Option<bool>,
    pub sample_rate_hz: Option<u32>,
    pub bit_depth: Option<u32>,
}

impl From<&WireFile> for FileEntry {
    fn from(file: &WireFile) -> Self {
        Self {
            name: file.name.clone(),
            size: file.size,
            bitrate_kbps: file.attribs.get(&ATTRIB_BITRATE_KBPS).copied(),
            duration_seconds: file.attribs.get(&ATTRIB_DURATION_SECONDS).copied(),
            vbr: file.attribs.get(&ATTRIB_VBR).map(|&value| value != 0),
            sample_rate_hz: file.attribs.get(&ATTRIB_SAMPLE_RATE_HZ).copied(),
            bit_depth: file.attribs.get(&ATTRIB_BIT_DEPTH).copied(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Candidate {
    pub username: String,
    pub slots: u8,
    pub speed: u32,
    pub files: Vec<FileEntry>,
}

impl From<WireSearchResult> for Candidate {
    fn from(result: WireSearchResult) -> Self {
        Self {
            username: result.username,
            slots: result.slots,
            speed: result.speed,
            files: result.files.iter().map(FileEntry::from).collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn wire_file(name: &str, size: u64) -> WireFile {
        wire_file_with_attribs(name, size, HashMap::new())
    }

    fn wire_file_with_attribs(name: &str, size: u64, attribs: HashMap<u32, u32>) -> WireFile {
        WireFile {
            username: "peer".to_string(),
            name: name.to_string(),
            size,
            attribs,
        }
    }

    #[test]
    fn search_result_becomes_a_candidate_with_its_files() {
        let result = WireSearchResult {
            token: 1,
            files: vec![wire_file("01 Track.flac", 42_000_000)],
            slots: 3,
            speed: 512_000,
            username: "peer".to_string(),
        };

        let candidate = Candidate::from(result);

        assert_eq!(candidate.username, "peer");
        assert_eq!(candidate.slots, 3);
        assert_eq!(candidate.speed, 512_000);
        assert_eq!(candidate.files.len(), 1);
        assert_eq!(candidate.files[0].name, "01 Track.flac");
        assert_eq!(candidate.files[0].size, 42_000_000);
    }

    #[test]
    fn file_attribute_codes_decode_to_the_documented_meaning() {
        let attribs = HashMap::from([(0, 320), (1, 245), (2, 0), (4, 44100), (5, 16)]);
        let file = wire_file_with_attribs("01 Track.mp3", 9_800_000, attribs);

        let entry = FileEntry::from(&file);

        assert_eq!(entry.bitrate_kbps, Some(320));
        assert_eq!(entry.duration_seconds, Some(245));
        assert_eq!(entry.vbr, Some(false));
        assert_eq!(entry.sample_rate_hz, Some(44_100));
        assert_eq!(entry.bit_depth, Some(16));
    }

    #[test]
    fn missing_attribute_codes_become_none() {
        let entry = FileEntry::from(&wire_file("01 Track.flac", 1));

        assert_eq!(entry.bitrate_kbps, None);
        assert_eq!(entry.duration_seconds, None);
        assert_eq!(entry.vbr, None);
        assert_eq!(entry.sample_rate_hz, None);
        assert_eq!(entry.bit_depth, None);
    }
}
