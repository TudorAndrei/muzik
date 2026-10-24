use super::*;
use gpui_kit::component::alert::Alert;
use gpui_kit::component::clipboard::Clipboard;
use gpui_kit::component::empty::{Empty, EmptyContent, EmptyDescription, EmptyHeader, EmptyTitle};
use gpui_kit::component::sheet::Sheet;
use gpui_kit::component::sidebar::{
    Sidebar, SidebarFooter, SidebarHeader, SidebarMenu, SidebarMenuItem,
};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::table::DataTable;

impl Muzik {
    pub(crate) fn watchlist(&self, cx: &Context<Self>) -> AnyElement {
        let playlists = &self.watchlist.playlists;
        let has_playlists = !playlists.is_empty();
        let loading = self.reading(Read::Watchlist);
        let mut content = div()
            .v_flex()
            .gap_4()
            .p_6()
            .size_full()
            .max_w(px(1240.))
            .child(self.watchlist_header(has_playlists, cx));
        content = match playlists.get(self.selected_playlist) {
            Some(playlist) => content.child(self.playlist_view(playlist, cx)),
            None if loading => content.child(empty_state(
                "Loading sources",
                "Reading the saved watchlist.",
                true,
            )),
            None => content.child(empty_state(
                "No playlists yet",
                "Paste a YouTube or Spotify playlist link under Sources, then add it.",
                false,
            )),
        };
        div()
            .flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .child(self.sources_rail(playlists, has_playlists, cx))
            .child(
                div()
                    .flex()
                    .justify_center()
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .child(content),
            )
            .into_any_element()
    }

    fn watchlist_header(&self, has_playlists: bool, cx: &Context<Self>) -> AnyElement {
        let refresh = Button::new("watch-refresh")
            .icon(IconName::RefreshCw)
            .label("Refresh all")
            .disabled(!has_playlists || self.activity.has_run(RunKind::Refresh))
            .on_click(cx.listener(move |view, _, _, cx| view.refresh(None, cx)));
        div()
            .flex()
            .items_center()
            .justify_between()
            .gap_3()
            .child(style::page_title("Watchlist"))
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(
                        Button::new("watch-reload")
                            .ghost()
                            .label("Reload")
                            .on_click(cx.listener(|view, _, _, cx| {
                                view.load_watchlist(cx);
                                cx.notify();
                            })),
                    )
                    .child(if has_playlists {
                        refresh.primary()
                    } else {
                        refresh
                    }),
            )
            .into_any_element()
    }

    fn sources_rail(
        &self,
        playlists: &[PlaylistView],
        has_playlists: bool,
        cx: &Context<Self>,
    ) -> AnyElement {
        let items: Vec<SidebarMenuItem> = playlists
            .iter()
            .enumerate()
            .map(|(index, playlist)| {
                let rename_title = playlist.title.as_deref().unwrap_or("").to_string();
                let item = SidebarMenuItem::new(playlist_title(playlist).to_string())
                    .active(index == self.selected_playlist)
                    .on_click(cx.listener(move |view, _, window, cx| {
                        view.select_playlist(index, rename_title.clone(), window, cx);
                    }));
                if playlist.last_error.is_some() {
                    item.icon(IconName::TriangleAlert)
                } else {
                    item.icon(style::source_icon(&playlist.playlist_id))
                }
            })
            .collect();
        let add = Button::new("add-playlist")
            .icon(IconName::Plus)
            .label("Add playlist")
            .w_full()
            .on_click(cx.listener(|view, _, _, cx| {
                let url = view.watch_url.read(cx).value().to_string();
                if !url.trim().is_empty() {
                    view.add_source(url, cx);
                    cx.notify();
                }
            }));
        Sidebar::new("sources")
            .collapsible(false)
            .header(SidebarHeader::new().child(style::section_title("Sources")))
            .child(SidebarMenu::new().children(items))
            .footer(
                SidebarFooter::new().child(
                    div()
                        .v_flex()
                        .gap_2()
                        .w_full()
                        .child(Input::new(&self.watch_url))
                        .child(if has_playlists { add } else { add.primary() }),
                ),
            )
            .into_any_element()
    }

    fn select_playlist(
        &mut self,
        index: usize,
        title: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.selected_playlist = index;
        self.playlist_name
            .update(cx, |state, cx| state.set_value(title, window, cx));
        self.watch_table.update(cx, TableState::clear_selection);
        self.sync_watch_table(cx);
        cx.notify();
    }

    fn playlist_view(&self, playlist: &PlaylistView, cx: &Context<Self>) -> AnyElement {
        let title = playlist_title(playlist).to_string();
        let items = &playlist.items;
        let facts = format!(
            "{} · {} items · {}",
            playlist.kind,
            items.len(),
            playlist
                .last_checked_at
                .as_deref()
                .map_or_else(|| "not checked".to_string(), |at| format!("checked {at}"))
        );
        let tools = self.playlist_tools(playlist, &title, items, cx);
        let mut section = div().v_flex().gap_4().flex_1().min_h_0().child(
            div()
                .v_flex()
                .gap_1()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap_3()
                        .child(style::section_title(title).min_w_0().truncate())
                        .child(tools),
                )
                .child(style::meta(facts, cx)),
        );
        if let Some(error) = playlist.last_error.as_deref() {
            section = section.child(Alert::error("playlist-error", error.to_string()));
        }
        section = section.child(self.filter_tabs(cx));
        if !items.iter().any(|item| matches_filter(item, self.filter)) {
            return section
                .child(empty_state(
                    "Nothing here",
                    self.empty_filter_message(playlist, items),
                    false,
                ))
                .into_any_element();
        }
        section
            .child(
                pages::table_frame()
                    .on_action(cx.listener(|view, _: &OpenSelected, window, cx| {
                        let table = view.watch_table.read(cx);
                        let key = table
                            .selected_row()
                            .and_then(|row| table.delegate().key(row));
                        if let Some(key) = key {
                            view.open_item_sheet(key, window, cx);
                        }
                    }))
                    .child(DataTable::new(&self.watch_table).stripe(true)),
            )
            .into_any_element()
    }

    fn filter_tabs(&self, cx: &Context<Self>) -> TabBar {
        TabBar::new("filters")
            .pill()
            .small()
            .selected_index(self.filter)
            .children((0..=Summary::ALL.len()).map(|filter| Tab::new().label(filter_label(filter))))
            .on_click(cx.listener(|view, index: &usize, _, cx| {
                view.filter = *index;
                view.watch_table.update(cx, TableState::clear_selection);
                view.sync_watch_table(cx);
                cx.notify();
            }))
    }

    fn empty_filter_message(&self, playlist: &PlaylistView, items: &[ItemView]) -> String {
        if playlist.kind == SourceKind::Spotify && items.is_empty() {
            "This Spotify source has no tracks. Refresh it to read track names. Set Audio source to Soulseek in Settings to get audio.".to_string()
        } else if items.is_empty() && playlist.last_checked_at.is_none() {
            "This playlist has not been checked. Select Refresh to read it.".to_string()
        } else if items.is_empty() {
            "This playlist has no videos. Refresh it to check again.".to_string()
        } else {
            format!(
                "No items have the {} status. Select All to see every item.",
                filter_label(self.filter)
            )
        }
    }

    fn playlist_tools(
        &self,
        playlist: &PlaylistView,
        title: &str,
        items: &[ItemView],
        cx: &Context<Self>,
    ) -> Div {
        let id = playlist_id(playlist);
        let source_url = playlist.url.clone();
        let refresh_source = (id.clone(), title.to_string());
        let mut tools = div().flex().items_center().gap_1().child(
            Button::new("refresh-source")
                .ghost()
                .small()
                .icon(IconName::RefreshCw)
                .label("Refresh")
                .disabled(self.activity.has_run(RunKind::Refresh))
                .on_click(cx.listener(move |view, _, _, cx| {
                    view.refresh(Some(refresh_source.clone()), cx);
                })),
        );
        if !source_url.is_empty() {
            let open_url = source_url.clone();
            tools = tools
                .child(
                    Button::new("open-source")
                        .ghost()
                        .small()
                        .icon(IconName::ExternalLink)
                        .label("Open")
                        .on_click(cx.listener(move |_, _, _, cx| cx.open_url(&open_url))),
                )
                .child(
                    Clipboard::new("copy-source")
                        .value(source_url)
                        .tooltip("Copy source link"),
                );
        }
        let retries: Vec<ItemRequest> = items
            .iter()
            .filter(|item| {
                retryable(item) && !self.is_queued(&id, item_position(item), &item_video_id(item))
            })
            .map(|item| self.item_request(ItemId::of(&id, &item.item), ItemAction::Retry))
            .collect();
        if !retries.is_empty() {
            tools = tools.child(
                Button::new("retry-failed")
                    .ghost()
                    .small()
                    .icon(IconName::Redo)
                    .label(format!("Retry {} failed", retries.len()))
                    .on_click(cx.listener(move |view, _, _, cx| {
                        for item in &retries {
                            view.run_item(item.clone(), cx);
                        }
                    })),
            );
        }
        let rename_id = id.clone();
        let remove_id = id;
        let remove_title = title.to_string();
        tools
            .child(
                Button::new("rename-playlist")
                    .ghost()
                    .small()
                    .label("Rename")
                    .on_click(cx.listener(move |view, _, window, cx| {
                        view.open_rename(rename_id.clone(), window, cx);
                    })),
            )
            .child(
                Button::new("remove-playlist")
                    .ghost()
                    .small()
                    .label("Remove")
                    .text_color(cx.theme().danger)
                    .on_click(cx.listener(move |_, _, window, cx| {
                        Self::request_action(
                            PendingAction {
                                title: format!("Remove “{remove_title}”?"),
                                description: "The playlist leaves the watchlist.",
                                confirm: "Remove playlist".into(),
                                destructive: true,
                                command: Command::RemoveSource(remove_id.clone()),
                            },
                            window,
                            cx,
                        );
                    })),
            )
    }

    fn open_rename(&self, playlist_id: String, window: &mut Window, cx: &mut Context<Self>) {
        let view = cx.entity().downgrade();
        let name = self.playlist_name.clone();
        window.open_alert_dialog(cx, move |dialog, _, _| {
            let view = view.clone();
            let playlist_id = playlist_id.clone();
            dialog
                .title("Rename playlist")
                .child(Input::new(&name))
                .show_cancel(true)
                .cancel_text("Cancel")
                .ok_text("Rename")
                .on_ok(move |_, _, cx| {
                    let playlist_id = playlist_id.clone();
                    let _ = view.update(cx, |view, cx| {
                        let title = view.playlist_name.read(cx).value().to_string();
                        if !title.trim().is_empty() {
                            view.rename_source(playlist_id, title, cx);
                            cx.notify();
                        }
                    });
                    true
                })
        });
    }

    fn is_queued(&self, playlist_id: &str, position: u64, video_id: &str) -> bool {
        self.activity
            .queued_items()
            .contains(&ItemId::new(playlist_id, position, Some(video_id)).to_string())
    }

    pub(crate) fn item_request(&self, id: ItemId, action: ItemAction) -> ItemRequest {
        let title = self.find_item(&id).map_or_else(
            || id.video_id.clone().unwrap_or_default(),
            |item| item.item.title,
        );
        ItemRequest { id, title, action }
    }

    fn find_item(&self, key: &ItemId) -> Option<ItemView> {
        self.watchlist
            .playlists
            .iter()
            .find(|playlist| playlist.playlist_id == key.playlist_id)?
            .items
            .iter()
            .find(|item| ItemId::of(&key.playlist_id, &item.item) == *key)
            .cloned()
    }

    pub(crate) fn open_item_sheet(
        &mut self,
        key: ItemId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.sheet_item.clone_from(&key.video_id);
        self.cache_visible_thumbnails(cx);
        let view = cx.entity().downgrade();
        window.open_sheet(cx, move |sheet, _, cx| {
            let Some(entity) = view.upgrade() else {
                return sheet;
            };
            let Some(item) = entity.read(cx).find_item(&key) else {
                return sheet.title("Item not found");
            };
            item_sheet(sheet, &item, &key, &view, cx)
        });
    }
}

fn item_sheet(
    sheet: Sheet,
    item: &ItemView,
    key: &ItemId,
    view: &WeakEntity<Muzik>,
    cx: &App,
) -> Sheet {
    let title = item.item.title.clone();
    let (id_label, open_label) = match item.item.kind {
        SourceKind::Spotify => ("Spotify ID", "Open in Spotify"),
        SourceKind::Bandcamp => ("Bandcamp ID", "Open on Bandcamp"),
        SourceKind::Youtube => ("YouTube ID", "Open on YouTube"),
    };
    let mut body = div().v_flex().gap_5();
    if let Some(path) = item.thumbnail_path.as_deref() {
        body = body.child(
            img(PathBuf::from(path))
                .w_full()
                .h(px(180.))
                .rounded_md()
                .object_fit(ObjectFit::Cover),
        );
    }
    let mut state =
        div()
            .v_flex()
            .gap_1()
            .child(style::stage_track("sheet-stages", &item.item, cx));
    if let Some(error) = item.item.last_error.as_deref() {
        state = state.child(div().text_sm().child(error.to_string()));
    }
    body = body.child(state);
    let states = style::stage_states(&item.item);
    let mut facts = DescriptionList::horizontal()
        .columns(1)
        .label_width(px(110.))
        .item(id_label, key.video_id.clone().unwrap_or_default(), 1);
    for (stage, state) in states {
        facts = facts.item(style::stage_label(stage), style::status_word(state), 1);
    }
    let mut links = div().flex().items_center().gap_2().child(
        Clipboard::new("copy-item-id")
            .value(key.video_id.clone().unwrap_or_default())
            .tooltip("Copy ID"),
    );
    if let Some(url) = item.item.video_url.as_deref() {
        let url = url.to_string();
        links = links.child(
            Button::new("open-item")
                .small()
                .icon(IconName::ExternalLink)
                .label(open_label)
                .on_click(move |_, _, cx| cx.open_url(&url)),
        );
    }
    body = body.child(div().v_flex().gap_2().child(facts).child(links));
    let Some(entity) = view.upgrade() else {
        return sheet.title(title).child(body);
    };
    let item_id = key.to_string();
    let waiting = entity
        .read(cx)
        .activity
        .waiting()
        .iter()
        .find(|job| job.data.get("item").and_then(Value::as_str) == Some(item_id.as_str()))
        .and_then(|job| job.data.get("queue_job").and_then(Value::as_i64));
    let waiting_stage = style::stage_states(&item.item)
        .into_iter()
        .find(|(_, status)| *status == muzik_store::watchlist::StageStatus::Waiting)
        .map(|(stage, _)| stage);
    if let Some(job) = waiting {
        let view = view.clone();
        body = body.child(
            Button::new("open-item-decision")
                .primary()
                .label(waiting_stage.map_or_else(
                    || "Open waiting choice".to_string(),
                    |stage| format!("Open {} review", style::stage_label(stage)),
                ))
                .on_click(move |_, window, cx| {
                    window.close_sheet(cx);
                    let _ = view.update(cx, |view, cx| view.open_waiting(job, window, cx));
                }),
        );
    } else if waiting_stage.is_some() {
        body = body.child(style::meta(
            "No choice is open. Select Resume under Commands to continue this item.",
            cx,
        ));
    }
    let commands = item_commands(item, key, &title, &entity, view, cx);
    sheet
        .title(title)
        .size(px(420.))
        .child(body.child(commands).overflow_y_scrollbar())
}

fn item_commands(
    item: &ItemView,
    key: &ItemId,
    title: &str,
    entity: &Entity<Muzik>,
    view: &WeakEntity<Muzik>,
    cx: &App,
) -> Div {
    let is_waiting = style::stage_states(&item.item)
        .iter()
        .any(|(_, status)| *status == muzik_store::watchlist::StageStatus::Waiting);
    let mut commands = div()
        .v_flex()
        .gap_3()
        .pt_4()
        .border_t_1()
        .border_color(cx.theme().border)
        .child(style::overline("COMMANDS", cx));
    for (index, action) in ItemAction::ALL.iter().copied().enumerate() {
        let availability = item.actions.get(&action);
        let enabled = availability.is_some_and(|availability| availability.enabled);
        let request = entity.read(cx).item_request(key.clone(), action);
        let label = if action == ItemAction::Run && is_waiting {
            "Resume"
        } else {
            action_label(action)
        }
        .to_string();
        let item_title = title.to_string();
        let view = view.clone();
        let mut row = div().v_flex().gap_1().child(
            Button::new(("item-action", index))
                .small()
                .label(label.clone())
                .disabled(!enabled)
                .on_click(move |_, window, cx| {
                    window.close_sheet(cx);
                    let request = request.clone();
                    let item_title = item_title.clone();
                    let _ = view.update(cx, |view, cx| {
                        if action.replaces_files() {
                            Muzik::request_action(
                                PendingAction {
                                    title: format!("{label} for “{item_title}”?"),
                                    description: REPLACE_WARNING,
                                    confirm: label.clone(),
                                    destructive: false,
                                    command: Command::RunItem(request),
                                },
                                window,
                                cx,
                            );
                        } else {
                            view.run_item(request, cx);
                        }
                    });
                }),
        );
        if !enabled
            && let Some(reason) =
                availability.and_then(|availability| availability.reason.as_deref())
        {
            row = row.child(style::meta(reason.to_string(), cx));
        }
        commands = commands.child(row);
    }
    commands
}

pub fn empty_state(
    title: impl Into<SharedString>,
    description: impl Into<SharedString>,
    loading: bool,
) -> AnyElement {
    let header = EmptyHeader::new()
        .title(EmptyTitle::new().child(title.into()))
        .description(EmptyDescription::new().child(description.into()));
    let empty = Empty::new().header(header);
    if loading {
        empty
            .content(EmptyContent::new().child(Spinner::new()))
            .into_any_element()
    } else {
        empty.into_any_element()
    }
}

pub fn playlist_id(playlist: &PlaylistView) -> String {
    playlist.playlist_id.clone()
}

fn playlist_title(playlist: &PlaylistView) -> &str {
    playlist.title.as_deref().unwrap_or(&playlist.playlist_id)
}

const fn item_position(item: &ItemView) -> u64 {
    item.item.position
}

fn retryable(item: &ItemView) -> bool {
    item.summary == Summary::Failed
        && item
            .primary_action
            .as_ref()
            .is_some_and(|primary| primary.action == ItemAction::Retry)
        && item
            .actions
            .get(&ItemAction::Retry)
            .is_some_and(|retry| retry.enabled)
}

fn item_video_id(item: &ItemView) -> String {
    item.item.video_id.clone().unwrap_or_default()
}

pub fn matches_filter(item: &ItemView, filter: usize) -> bool {
    filter_summary(filter).map_or_else(
        || item.summary != Summary::Unavailable,
        |wanted| item.summary == wanted,
    )
}

#[cfg(test)]
mod tests {
    use super::retryable;
    use serde_json::json;

    #[gpui_kit::test]
    fn details_find_unavailable_items_with_empty_or_missing_ids(cx: &mut gpui_kit::TestAppContext) {
        use gpui_kit::AppContext as _;
        let (handle, main) = cx.update(|cx| {
            gpui_kit::init(cx);
            gpui_kit::open_window(gpui_kit::WindowOptions::default(), cx, |window, cx| {
                cx.new(|cx| super::Muzik::new_with_backend(window, cx, false))
            })
            .unwrap()
        });
        cx.update_window(handle, |_, _, cx| {
            main.update(cx, |view, _| {
                view.watchlist = serde_json::from_value(json!({"playlists":[{
                    "playlist_id":"PL1","url":"https://youtube.com/playlist?list=PL1","items":[
                        {"position":1,"title":"Empty ID","video_id":"","summary":"Unavailable","actions":{}},
                        {"position":2,"title":"Missing ID","video_id":null,"summary":"Unavailable","actions":{}}
                    ]
                }]})).unwrap();
                for (position, title) in [(1,"Empty ID"),(2,"Missing ID")] {
                    let key = super::ItemId::new("PL1", position, None);
                    assert_eq!(view.find_item(&key).unwrap().item.title, title);
                }
            });
        }).unwrap();
    }

    #[test]
    fn retry_all_takes_only_failed_items_with_an_enabled_retry() {
        let failed = |enabled: bool| {
            serde_json::from_value(json!({"position":1,"title":"Track","summary": "Failed",
                   "primary_action": {"action": "retry", "label": "Retry"},
                   "actions": {"retry": {"enabled": enabled, "reason": null}}}))
            .unwrap()
        };
        assert!(retryable(&failed(true)));
        assert!(!retryable(&failed(false)));
        assert!(!retryable(
            &serde_json::from_value(json!({"position":1,"title":"Track","summary": "Processed", "primary_action": null,"actions":{}})).unwrap()
        ));
    }
}
