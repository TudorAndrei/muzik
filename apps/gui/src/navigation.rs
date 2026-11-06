use gpui_kit::component::IconName;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Page {
    Workflow,
    Watchlist,
    Library,
    Settings,
    Spotify,
}

pub struct PageInfo {
    pub page: Page,
    pub label: &'static str,
    pub icon: IconName,
    pub keywords: &'static [&'static str],
}

pub const PAGES: [PageInfo; 5] = [
    PageInfo {
        page: Page::Workflow,
        label: "Workflow",
        icon: IconName::Play,
        keywords: &["run", "download", "split"],
    },
    PageInfo {
        page: Page::Watchlist,
        label: "Watchlist",
        icon: IconName::Eye,
        keywords: &["playlist", "sources"],
    },
    PageInfo {
        page: Page::Library,
        label: "Library",
        icon: IconName::BookOpen,
        keywords: &["beets", "albums"],
    },
    PageInfo {
        page: Page::Settings,
        label: "Settings",
        icon: IconName::Settings,
        keywords: &["config", "preferences"],
    },
    PageInfo {
        page: Page::Spotify,
        label: "Spotify",
        icon: IconName::Globe,
        keywords: &["login", "import"],
    },
];
