use crate::{Muzik, Page};
use gpui_kit::component::command::{Command, CommandGroup, CommandItem, CommandState};
use gpui_kit::component::input::{Copy, Cut, Paste, Redo, SelectAll, Undo};
use gpui_kit::component::{IconName, IndexPath, WindowExt as _};
use gpui_kit::prelude::*;
use gpui_kit::{
    Action, App, Entity, KeyBinding, Menu, MenuItem, OsAction, WeakEntity, Window, actions, rems,
};

actions!(
    muzik,
    [
        #[derive(Eq)]
        Quit,
        #[derive(Eq)]
        ShowWorkflow,
        #[derive(Eq)]
        ShowWatchlist,
        #[derive(Eq)]
        ShowLibrary,
        #[derive(Eq)]
        ShowSettings,
        #[derive(Eq)]
        ShowSpotify,
        #[derive(Eq)]
        OpenCommands,
        #[derive(Eq)]
        Refresh,
        #[derive(Eq)]
        Minimize,
        #[derive(Eq)]
        Zoom
    ]
);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AppCommand {
    Show(Page),
    RunWorkflow,
    SearchLibrary,
    Refresh,
}

const GO_TO: [(Page, &str, IconName, &[&str]); 5] = [
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

const ACTIONS: [(AppCommand, &str, IconName, &[&str]); 3] = [
    (
        AppCommand::RunWorkflow,
        "Run workflow",
        IconName::Play,
        &["start"],
    ),
    (
        AppCommand::SearchLibrary,
        "Search the library",
        IconName::Search,
        &["find", "album", "artist"],
    ),
    (
        AppCommand::Refresh,
        "Refresh this page",
        IconName::RefreshCw,
        &["reload"],
    ),
];

pub fn page_action(page: Page) -> Box<dyn Action> {
    match page {
        Page::Workflow => Box::new(ShowWorkflow),
        Page::Watchlist => Box::new(ShowWatchlist),
        Page::Library => Box::new(ShowLibrary),
        Page::Settings => Box::new(ShowSettings),
        Page::Spotify => Box::new(ShowSpotify),
    }
}

pub fn command_at(index: IndexPath) -> Option<AppCommand> {
    match index.section {
        0 => GO_TO
            .get(index.row)
            .map(|(page, ..)| AppCommand::Show(*page)),
        1 => ACTIONS.get(index.row).map(|(command, ..)| *command),
        _ => None,
    }
}

pub fn init(cx: &mut App) {
    cx.on_action(|_: &Quit, cx| cx.quit());
    cx.bind_keys([
        KeyBinding::new("cmd-q", Quit, None),
        KeyBinding::new("secondary-1", ShowWorkflow, None),
        KeyBinding::new("secondary-2", ShowWatchlist, None),
        KeyBinding::new("secondary-3", ShowLibrary, None),
        KeyBinding::new("secondary-4", ShowSettings, None),
        KeyBinding::new("secondary-,", ShowSettings, None),
        KeyBinding::new("secondary-5", ShowSpotify, None),
        KeyBinding::new("secondary-k", OpenCommands, None),
        KeyBinding::new("secondary-shift-p", OpenCommands, None),
        KeyBinding::new("secondary-r", Refresh, None),
        KeyBinding::new("cmd-m", Minimize, None),
    ]);
    cx.set_menus([
        Menu::new("Muzik").items([
            MenuItem::action("Settings…", ShowSettings),
            MenuItem::separator(),
            MenuItem::action("Quit Muzik", Quit),
        ]),
        Menu::new("Edit").items([
            MenuItem::os_action("Undo", Undo, OsAction::Undo),
            MenuItem::os_action("Redo", Redo, OsAction::Redo),
            MenuItem::separator(),
            MenuItem::os_action("Cut", Cut, OsAction::Cut),
            MenuItem::os_action("Copy", Copy, OsAction::Copy),
            MenuItem::os_action("Paste", Paste, OsAction::Paste),
            MenuItem::os_action("Select All", SelectAll, OsAction::SelectAll),
        ]),
        Menu::new("View").items([
            MenuItem::action("Workflow", ShowWorkflow),
            MenuItem::action("Watchlist", ShowWatchlist),
            MenuItem::action("Library", ShowLibrary),
            MenuItem::action("Spotify", ShowSpotify),
            MenuItem::separator(),
            MenuItem::action("Command Palette…", OpenCommands),
            MenuItem::action("Refresh", Refresh),
        ]),
        Menu::new("Window").items([
            MenuItem::action("Minimize", Minimize),
            MenuItem::action("Zoom", Zoom),
        ]),
    ]);
}

pub fn open_palette(
    state: &Entity<CommandState>,
    view: WeakEntity<Muzik>,
    window: &mut Window,
    cx: &mut App,
) {
    if window.has_active_dialog(cx) {
        return;
    }
    state.update(cx, |state, cx| state.set_query("", window, cx));
    let palette = state.clone();
    window.open_dialog(cx, move |dialog, _, _| {
        let palette = palette.clone();
        let view = view.clone();
        dialog
            .close_button(false)
            .p_0()
            .content(move |content, _, _| {
                let view = view.clone();
                content.child(
                    Command::new(&palette)
                        .bordered(false)
                        .placeholder("Go to a page or run a command")
                        .group(CommandGroup::new().label("Go to").items(GO_TO.iter().map(
                            |(page, label, icon, keywords)| {
                                CommandItem::new()
                                    .label(*label)
                                    .icon(icon.clone())
                                    .keywords(keywords.iter().copied())
                                    .action(page_action(*page))
                            },
                        )))
                        .group(
                            CommandGroup::new()
                                .label("Actions")
                                .items(ACTIONS.iter().map(|(command, label, icon, keywords)| {
                                    let item = CommandItem::new()
                                        .label(*label)
                                        .icon(icon.clone())
                                        .keywords(keywords.iter().copied());
                                    if *command == AppCommand::Refresh {
                                        item.action(Box::new(Refresh))
                                    } else {
                                        item
                                    }
                                })),
                        )
                        .max_h(rems(22.))
                        .on_confirm(move |index, window, cx| {
                            window.close_dialog(cx);
                            if let Some(command) = command_at(index) {
                                let _ = view.update(cx, |view, cx| {
                                    view.run_command(command, window, cx);
                                });
                            }
                        }),
                )
            })
    });
    state.update(cx, |state, cx| state.focus(window, cx));
}
