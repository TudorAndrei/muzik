use gpui_kit::component::IconName;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Page {
    Workflow,
    Watchlist,
    Library,
    Settings,
    Spotify,
}

pub const PAGES: [(Page, &str, IconName, &[&str]); 5] = [
    (
        Page::Workflow,
        "Workflow",
        IconName::Play,
        &["run", "download", "split"],
    ),
    (
        Page::Watchlist,
        "Watchlist",
        IconName::Eye,
        &["playlist", "sources"],
    ),
    (
        Page::Library,
        "Library",
        IconName::BookOpen,
        &["beets", "albums"],
    ),
    (
        Page::Settings,
        "Settings",
        IconName::Settings,
        &["config", "preferences"],
    ),
    (
        Page::Spotify,
        "Spotify",
        IconName::Globe,
        &["login", "import"],
    ),
];
