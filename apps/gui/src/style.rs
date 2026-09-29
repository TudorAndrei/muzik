use gpui_kit::component::theme::{Theme, ThemeRegistry};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::*;
use gpui_kit::*;
use serde_json::Value;

const THEME: &str = include_str!("../themes/muzik.json");
const LOGO: &[u8] = include_bytes!("../../../assets/muzik-logo-v2.png");

pub const STAGES: [(&str, &str); 5] = [
    ("download", "Download"),
    ("quality", "Quality"),
    ("parse", "Parse"),
    ("split", "Split"),
    ("organize", "Organize"),
];

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

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StageState {
    NotStarted,
    Running,
    Waiting,
    Complete,
    Failed,
    Skipped,
    Stale,
}

impl StageState {
    pub fn parse(status: &str) -> Self {
        match status {
            "running" => Self::Running,
            "waiting" => Self::Waiting,
            "complete" => Self::Complete,
            "failed" => Self::Failed,
            "skipped" => Self::Skipped,
            "stale" => Self::Stale,
            _ => Self::NotStarted,
        }
    }

    pub fn word(self) -> &'static str {
        match self {
            Self::NotStarted => "Not started",
            Self::Running => "Running",
            Self::Waiting => "Waiting for you",
            Self::Complete => "Complete",
            Self::Failed => "Failed",
            Self::Skipped => "Skipped",
            Self::Stale => "Stale",
        }
    }
}

pub fn stage_states(item: &Value) -> [StageState; 5] {
    STAGES.map(|(key, _)| StageState::parse(item["stages"][key]["status"].as_str().unwrap_or("")))
}

pub fn stage_headline(states: &[StageState; 5]) -> (String, Option<Tone>) {
    let named = |state: StageState| {
        states
            .iter()
            .position(|current| *current == state)
            .map(|index| STAGES[index].1)
    };
    if let Some(stage) = named(StageState::Failed) {
        return (format!("{stage} failed"), Some(Tone::Danger));
    }
    if let Some(stage) = named(StageState::Running) {
        return (format!("{stage} running"), Some(Tone::Info));
    }
    if let Some(stage) = named(StageState::Waiting) {
        return (format!("{stage} waits for you"), Some(Tone::Warning));
    }
    if let Some(stage) = named(StageState::Stale) {
        return (format!("{stage} stale"), Some(Tone::Warning));
    }
    if states
        .iter()
        .all(|state| matches!(state, StageState::Complete | StageState::Skipped))
    {
        return ("Done".into(), None);
    }
    if states.iter().all(|state| *state == StageState::NotStarted) {
        return ("Not started".into(), None);
    }
    let done = states
        .iter()
        .filter(|state| matches!(state, StageState::Complete | StageState::Skipped))
        .count();
    (format!("{done} of 5 stages done"), None)
}

pub fn stage_track(id: impl Into<ElementId>, item: &Value, cx: &App) -> AnyElement {
    let states = stage_states(item);
    let theme = cx.theme();
    let mut bars = div().id(id).flex().gap(px(3.)).flex_none();
    for ((_, label), state) in STAGES.iter().zip(states) {
        let bar = div().w(px(18.)).h(px(4.)).rounded(px(2.));
        let bar = match state {
            StageState::NotStarted => bar.bg(theme.border),
            StageState::Running => bar.bg(theme.info),
            StageState::Waiting => bar.bg(theme.warning.opacity(0.4)),
            StageState::Complete => bar.bg(theme.success),
            StageState::Failed => bar.bg(theme.danger),
            StageState::Stale => bar.bg(theme.warning),
            StageState::Skipped => bar.border_1().border_color(theme.input),
        };
        let tip: SharedString = format!("{label} · {}", state.word()).into();
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
    use super::{stage_headline, stage_states, StageState, Tone};
    use serde_json::json;

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
        assert_eq!(states[1], StageState::Skipped);
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
