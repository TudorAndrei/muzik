use muzik_core::PreferredAudio;
use muzik_soulseek::ranking::{rank, search_query};
use muzik_soulseek::types::{Candidate, FileEntry};
use proptest::collection::vec;
use proptest::option::of;
use proptest::prelude::*;
use proptest::sample::select;

fn preference() -> impl Strategy<Value = PreferredAudio> {
    select(vec![
        PreferredAudio::Lossless,
        PreferredAudio::Best,
        PreferredAudio::Any,
        PreferredAudio::Mp3_320,
        PreferredAudio::Format(muzik_core::audio::AudioFormat::Mp3),
        PreferredAudio::Format(muzik_core::audio::AudioFormat::Flac),
        PreferredAudio::Format(muzik_core::audio::AudioFormat::Opus),
    ])
}

fn file() -> impl Strategy<Value = FileEntry> {
    (
        "[A-Za-z0-9 /\\\\.]{1,20}\\.(flac|mp3|ogg|txt|m4a)",
        any::<u64>(),
        (of(any::<u32>()), of(any::<u32>()), of(any::<bool>())),
        (of(any::<u32>()), of(any::<u32>())),
    )
        .prop_map(
            |(name, size, (bitrate_kbps, duration_seconds, vbr), (sample_rate_hz, bit_depth))| {
                FileEntry {
                    name,
                    size,
                    bitrate_kbps,
                    duration_seconds,
                    vbr,
                    sample_rate_hz,
                    bit_depth,
                }
            },
        )
}

fn candidate() -> impl Strategy<Value = Candidate> {
    ("[a-c]{1,2}", any::<u8>(), any::<u32>(), vec(file(), 0..5)).prop_map(
        |(username, slots, speed, files)| Candidate {
            username,
            slots,
            speed,
            files,
        },
    )
}

proptest! {
    #[test]
    fn search_query_adds_the_quality_term_only_once(
        query in "[ ]{0,2}[A-Za-z0-9-]{1,8}( [A-Za-z0-9-]{1,8}){0,3}[ ]{0,2}",
        prefer in preference(),
    ) {
        let once = search_query(&query, prefer);
        prop_assert!(once.starts_with(query.trim()));
        prop_assert_eq!(search_query(&once, prefer), once);
    }

    #[test]
    fn rank_keeps_the_best_candidates_in_order(
        candidates in vec(candidate(), 0..8),
        query in "[A-Za-z ]{0,20}",
        prefer in preference(),
        limit in 0_usize..10,
    ) {
        let ranked = rank(candidates.clone(), &query, prefer, limit);
        prop_assert_eq!(ranked.len(), candidates.len().min(limit));
        prop_assert!(ranked.iter().all(|item| item.score.is_finite()));
        for pair in ranked.windows(2) {
            let (first, second) = (&pair[0], &pair[1]);
            prop_assert!(
                first.score > second.score
                    || (first.score.total_cmp(&second.score).is_eq()
                        && first.candidate.username <= second.candidate.username)
            );
        }
        let mut unused = candidates;
        for item in &ranked {
            let position = unused.iter().position(|candidate| *candidate == item.candidate);
            prop_assert!(position.is_some());
            if let Some(position) = position {
                unused.swap_remove(position);
            }
        }
    }
}
