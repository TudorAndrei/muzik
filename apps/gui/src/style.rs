use gpui_kit::component::theme::{Theme, ThemeRegistry};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::*;
use gpui_kit::*;
use muzik_core::watchlist::{stage_status, Stage, StageStatus};
use serde_json::Value;

const THEME: &str = include_str!("../themes/muzik.json");
const LOGO: &[u8] = include_bytes!("../../../assets/muzik-logo-v2.png");

gpui_kit::assets::icon_assets!(SourceIcons, [SquarePlay, ListMusic, Heart, Disc3]);

pub struct AppAssets;

impl AssetSource for AppAssets {
    fn load(&self, path: &str) -> Result<Option<std::borrow::Cow<'static, [u8]>>> {
        match SourceIcons.load(path)? {
            Some(icon) => Ok(Some(icon)),
            None => gpui_kit::assets::Assets.load(path),
        }
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut paths = SourceIcons.list(path)?;
        paths.extend(gpui_kit::assets::Assets.list(path)?);
        Ok(paths)
    }
}

pub fn source_icon(playlist: &Value) -> gpui_kit::assets::IconName {
    use gpui_kit::assets::IconName;
    let id = playlist["playlist_id"].as_str().unwrap_or("");
    if id == "spotify:liked" {
        IconName::Heart
    } else if id.starts_with("spotify:album:") {
        IconName::Disc3
    } else if id.starts_with("spotify:") {
        IconName::ListMusic
    } else {
        IconName::SquarePlay
    }
}

pub fn apply_theme(cx: &mut App) {
    if let Err(error) = ThemeRegistry::global_mut(cx).load_themes_from_str(THEME) {
        eprintln!("Muzik theme did not load: {error}");
        return;
    }
    let themes = ThemeRegistry::global(cx).themes();
    let (Some(light), Some(dark)) = (
        themes.get("Muzik Light").cloned(),
        themes.get("Muzik Dark").cloned(),
    ) else {
        return;
    };
    let theme = Theme::global_mut(cx);
    theme.light_theme = light;
    theme.dark_theme = dark;
    Theme::sync_system_appearance(None, cx);
}

pub fn logo() -> std::sync::Arc<Image> {
    std::sync::Arc::new(Image::from_bytes(ImageFormat::Png, LOGO.to_vec()))
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tone {
    Warning,
    Danger,
    Info,
}

pub fn strong(tone: Tone, cx: &App) -> Hsla {
    let theme = cx.theme();
    match tone {
        Tone::Warning => theme.warning,
        Tone::Danger => theme.danger,
        Tone::Info => theme.info,
    }
}

pub fn page_title(text: impl Into<SharedString>) -> Div {
    div().text_xl().font_semibold().child(text.into())
}

pub fn section_title(text: impl Into<SharedString>) -> Div {
    div().text_base().font_semibold().child(text.into())
}

pub fn meta(text: impl Into<SharedString>, cx: &App) -> Div {
    div()
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .child(text.into())
}

pub fn mono(text: impl Into<SharedString>, cx: &App) -> Div {
    div()
        .font_family(cx.theme().mono_font_family.clone())
        .text_xs()
        .child(text.into())
}

pub fn overline(text: impl Into<SharedString>, cx: &App) -> Div {
    let text = text.into();
    div()
        .text_size(px(11.))
        .font_semibold()
        .text_color(cx.theme().muted_foreground)
        .child(text)
}

pub fn stage_label(stage: Stage) -> &'static str {
    match stage {
        Stage::Download => "Download",
        Stage::Quality => "Quality",
        Stage::Parse => "Parse",
        Stage::Split => "Split",
        Stage::Organize => "Organize",
    }
}

pub fn status_word(status: StageStatus) -> &'static str {
    match status {
        StageStatus::NotStarted => "Not started",
        StageStatus::Running => "Running",
        StageStatus::Waiting => "Waiting for you",
        StageStatus::Complete => "Complete",
        StageStatus::Failed => "Failed",
        StageStatus::Skipped => "Skipped",
        StageStatus::Stale => "Stale",
    }
}

pub fn stage_states(item: &Value) -> Vec<(Stage, StageStatus)> {
    Stage::ALL
        .iter()
        .map(|stage| {
            (
                *stage,
                stage_status(item, *stage).unwrap_or(StageStatus::NotStarted),
            )
        })
        .collect()
}

pub fn stage_headline(states: &[(Stage, StageStatus)]) -> (String, Option<Tone>) {
    let named = |wanted: StageStatus| {
        states
            .iter()
            .find(|(_, status)| *status == wanted)
            .map(|(stage, _)| stage_label(*stage))
    };
    if let Some(stage) = named(StageStatus::Failed) {
        return (format!("{stage} failed"), Some(Tone::Danger));
    }
    if let Some(stage) = named(StageStatus::Running) {
        return (format!("{stage} running"), Some(Tone::Info));
    }
    if let Some(stage) = named(StageStatus::Waiting) {
        return (format!("{stage} waits for you"), Some(Tone::Warning));
    }
    if let Some(stage) = named(StageStatus::Stale) {
        return (format!("{stage} stale"), Some(Tone::Warning));
    }
    if states.iter().all(|(_, status)| status.is_done()) {
        return ("Done".into(), None);
    }
    if states
        .iter()
        .all(|(_, status)| *status == StageStatus::NotStarted)
    {
        return ("Not started".into(), None);
    }
    let done = states.iter().filter(|(_, status)| status.is_done()).count();
    (format!("{done} of {} stages done", states.len()), None)
}

pub fn stage_track(id: impl Into<ElementId>, item: &Value, cx: &App) -> AnyElement {
    let states = stage_states(item);
    let theme = cx.theme();
    let mut bars = div().id(id).flex().gap(px(3.)).flex_none();
    for (stage, state) in states.iter().copied() {
        let label = stage_label(stage);
        let bar = div().w(px(18.)).h(px(4.)).rounded(px(2.));
        let bar = match state {
            StageStatus::NotStarted => bar.bg(theme.border),
            StageStatus::Running => bar.bg(theme.info),
            StageStatus::Waiting => bar.bg(theme.warning.opacity(0.4)),
            StageStatus::Complete => bar.bg(theme.success),
            StageStatus::Failed => bar.bg(theme.danger),
            StageStatus::Stale => bar.bg(theme.warning),
            StageStatus::Skipped => bar.border_1().border_color(theme.input),
        };
        let tip: SharedString = format!("{label} · {}", status_word(state)).into();
        bars = bars.child(
            div()
                .id(SharedString::from(format!("stage-{label}")))
                .child(bar)
                .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx)),
        );
    }
    let (headline, tone) = stage_headline(&states);
    let color = tone.map_or(theme.foreground, |tone| strong(tone, cx));
    div()
        .flex()
        .items_center()
        .gap_2p5()
        .child(bars)
        .child(
            div()
                .text_sm()
                .font_semibold()
                .text_color(color)
                .child(headline),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::{source_icon, stage_headline, stage_states, AppAssets, Tone};
    use gpui_kit::AssetSource;
    use muzik_core::watchlist::{Stage, StageStatus};
    use serde_json::json;

    #[test]
    fn every_source_icon_and_the_default_icons_load() -> gpui_kit::Result<()> {
        for id in [
            "PL123",
            "spotify:liked",
            "spotify:album:abc",
            "spotify:playlist:abc",
        ] {
            let path = source_icon(&json!({"playlist_id": id})).path();
            assert!(AppAssets.load(&path)?.is_some(), "{path} is missing");
        }
        assert!(AppAssets.load("icons/plus.svg")?.is_some());
        Ok(())
    }

    #[test]
    fn stage_headline_names_the_stage_that_needs_attention() {
        let item = json!({"stages": {
            "download": {"status": "complete"},
            "quality": {"status": "skipped"},
            "parse": {"status": "complete"},
            "split": {"status": "failed"},
            "organize": {"status": "stale"}
        }});
        let states = stage_states(&item);
        assert_eq!(states[1], (Stage::Quality, StageStatus::Skipped));
        assert_eq!(
            stage_headline(&states),
            ("Split failed".to_string(), Some(Tone::Danger))
        );
    }

    #[test]
    fn stage_headline_is_quiet_when_every_stage_is_done() {
        let item = json!({"stages": {
            "download": {"status": "complete"},
            "quality": {"status": "skipped"},
            "parse": {"status": "complete"},
            "split": {"status": "complete"},
            "organize": {"status": "complete"}
        }});
        assert_eq!(
            stage_headline(&stage_states(&item)),
            ("Done".to_string(), None)
        );
        assert_eq!(
            stage_headline(&stage_states(&json!({}))),
            ("Not started".to_string(), None)
        );
    }
}
