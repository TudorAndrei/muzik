use muzik_core::SyncPreset;
use muzik_core::audio::Codec;
use muzik_media::quality::MeasuredQuality;
use muzik_sync::{Action, Encoding, Plan, Target, plan};
use proptest::prelude::*;
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

const LIBRARY: &str = "/muzik-sync-properties/library";
const DEVICE: &str = "/muzik-sync-properties/device";

fn codec() -> impl Strategy<Value = Codec> {
    prop_oneof![
        Just(Codec::Mp3),
        Just(Codec::Aac),
        Just(Codec::Opus),
        Just(Codec::Vorbis),
        Just(Codec::Flac),
        Just(Codec::Alac),
        Just(Codec::Ape),
        Just(Codec::WavPack),
        Just(Codec::WmaV2),
        Just(Codec::Pcm("pcm_s24le".into())),
        Just(Codec::Dsd("dsd_lsbf_planar".into())),
        Just(Codec::Other("unknown".into())),
    ]
}

fn quality() -> impl Strategy<Value = MeasuredQuality> {
    (
        codec(),
        proptest::option::of(8_u32..2_000),
        proptest::option::of(prop_oneof![
            Just(44_100_u32),
            Just(48_000),
            Just(96_000),
            Just(192_000),
            Just(352_800),
            Just(384_000),
            Just(768_000),
            1_u32..1_000_000,
        ]),
        proptest::option::of(prop_oneof![Just(16_u32), Just(24), Just(32), 1_u32..64]),
        proptest::option::of(any::<u64>()),
    )
        .prop_map(
            |(format, bitrate_kbps, sample_rate, bit_depth, size)| MeasuredQuality {
                lossless: format.is_lossless(),
                format,
                bitrate_kbps,
                sample_rate,
                bit_depth,
                channels: Some(2),
                size,
            },
        )
}

fn target() -> impl Strategy<Value = Target> {
    (
        prop_oneof![
            Just(SyncPreset::EchoMini),
            Just(SyncPreset::Mp3),
            Just(SyncPreset::Opus)
        ],
        proptest::option::of(32_u32..512),
        any::<bool>(),
    )
        .prop_map(|(preset, bitrate, covers)| Target {
            path: PathBuf::from(DEVICE),
            preset,
            bitrate,
            covers,
        })
}

fn track() -> impl Strategy<Value = (PathBuf, Option<MeasuredQuality>)> {
    let inside = "[aAb .?*:<|\t]{1,3}(/[aAb .?\"]{1,3}){0,2}/[aAb .?*]{1,3}\\.(flac|Flac|mp3|opus|m4a|wav|txt)"
        .prop_map(|relative| Path::new(LIBRARY).join(relative));
    let outside = "[ab]{1,3}\\.flac".prop_map(|name| Path::new("/elsewhere").join(name));
    (
        prop_oneof![9 => inside, 1 => outside],
        proptest::option::of(quality()),
    )
}

fn cover() -> impl Strategy<Value = PathBuf> {
    "[aA]{1,2}/cover\\.(jpg|png)".prop_map(|relative| Path::new(LIBRARY).join(relative))
}

fn planned(
    target: &Target,
    tracks: &[(PathBuf, Option<MeasuredQuality>)],
    covers: &[PathBuf],
    jobs: usize,
) -> Plan {
    let qualities: BTreeMap<_, _> = tracks.iter().cloned().collect();
    let paths: Vec<_> = tracks.iter().map(|(path, _)| path.clone()).collect();
    let probe = |path: &Path| match qualities.get(path) {
        Some(Some(quality)) => Ok(Some(quality.clone())),
        _ => Err("unreadable".to_owned()),
    };
    plan(
        target,
        Path::new(LIBRARY),
        &paths,
        covers,
        &BTreeMap::new(),
        jobs,
        &probe,
    )
}

#[test]
fn a_converted_track_keeps_its_folder_and_dotted_stem() {
    let target = Target {
        path: PathBuf::from(DEVICE),
        preset: SyncPreset::Mp3,
        bitrate: None,
        covers: false,
    };
    let flac = MeasuredQuality {
        lossless: true,
        format: Codec::Flac,
        bitrate_kbps: Some(900),
        sample_rate: Some(44_100),
        bit_depth: Some(16),
        channels: Some(2),
        size: Some(1),
    };
    let plan = planned(
        &target,
        &[(Path::new(LIBRARY).join("A/a.b.flac"), Some(flac))],
        &[],
        1,
    );
    assert_eq!(plan.pending.len(), 1);
    assert!(matches!(plan.pending[0].action, Action::Convert(_)));
    assert_eq!(
        plan.pending[0].destination,
        Path::new(DEVICE).join("A/a.b.mp3")
    );
}

proptest! {
    #[test]
    fn every_source_gets_exactly_one_outcome(
        target in target(),
        tracks in proptest::collection::vec(track(), 0..12),
        covers in proptest::collection::vec(cover(), 0..3),
        jobs in 1_usize..4,
    ) {
        let plan = planned(&target, &tracks, &covers, jobs);
        let outcomes: usize = [
            plan.fresh,
            plan.pending.len(),
            plan.duplicates.len(),
            plan.outside.len(),
            plan.unreadable.len(),
        ]
        .iter()
        .sum();
        prop_assert_eq!(outcomes, tracks.len().saturating_add(covers.len()));
        prop_assert_eq!(plan.planned.len(), plan.fresh.saturating_add(plan.pending.len()));
    }

    #[test]
    fn destinations_differ_without_regard_to_case(
        target in target(),
        tracks in proptest::collection::vec(track(), 0..12),
        covers in proptest::collection::vec(cover(), 0..3),
    ) {
        let plan = planned(&target, &tracks, &covers, 1);
        let mut seen = HashSet::new();
        for destination in &plan.planned {
            prop_assert!(
                seen.insert(destination.to_string_lossy().to_lowercase()),
                "{}",
                destination.display()
            );
        }
    }

    #[test]
    fn destinations_are_safe_names_inside_the_target(
        target in target(),
        tracks in proptest::collection::vec(track(), 0..12),
        covers in proptest::collection::vec(cover(), 0..3),
    ) {
        let plan = planned(&target, &tracks, &covers, 1);
        for transfer in &plan.pending {
            let relative = transfer.destination.strip_prefix(&target.path);
            prop_assert!(relative.is_ok(), "{}", transfer.destination.display());
            for component in relative.into_iter().flat_map(Path::components) {
                let name = component.as_os_str().to_string_lossy();
                prop_assert!(!name.is_empty());
                prop_assert!(!name.ends_with([' ', '.']), "{name:?}");
                prop_assert!(
                    !name
                        .chars()
                        .any(|character| character.is_control() || "<>:\"\\|?*".contains(character)),
                    "{name:?}"
                );
            }
        }
    }

    #[test]
    fn flac_conversions_stay_within_the_device_limits(
        target in target(),
        tracks in proptest::collection::vec(track(), 0..12),
    ) {
        let plan = planned(&target, &tracks, &[], 1);
        for transfer in &plan.pending {
            if let Action::Convert(Encoding::Flac { sample_rate, bit_depth }) = &transfer.action {
                prop_assert!(sample_rate.is_none_or(|rate| rate <= 192_000), "{sample_rate:?}");
                prop_assert!(bit_depth.is_none_or(|depth| depth <= 24), "{bit_depth:?}");
            }
        }
    }
}
