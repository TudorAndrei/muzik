use super::*;
use gpui_kit::component::alert::Alert;
use gpui_kit::component::clipboard::Clipboard;
use gpui_kit::component::empty::{Empty, EmptyContent, EmptyDescription, EmptyHeader, EmptyTitle};
use gpui_kit::component::pagination::Pagination;
use gpui_kit::component::sheet::Sheet;
use gpui_kit::component::sidebar::{
    Sidebar, SidebarFooter, SidebarHeader, SidebarMenu, SidebarMenuItem,
};
use gpui_kit::component::spinner::Spinner;

impl Muzik {
    pub(crate) fn watchlist(&self, cx: &mut Context<Self>) -> AnyElement {
        let playlists = self.watchlist["playlists"]
            .as_array()
            .or_else(|| self.watchlist.as_array());
        let has_playlists = playlists.is_some_and(|all| !all.is_empty());
        let loading = self
            .pending
            .values()
            .any(|command| command == "watchlist.load");
        let mut content = div()
            .v_flex()
            .gap_4()
            .p_6()
            .w_full()
            .max_w(px(960.))
            .child(self.watchlist_header(has_playlists, cx));
        content = match playlists.and_then(|all| all.get(self.selected_playlist)) {
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
            .size_full()
            .child(self.sources_rail(playlists, has_playlists, cx))
            .child(
                div()
                    .flex()
                    .justify_center()
                    .flex_1()
                    .min_w_0()
                    .overflow_y_scrollbar()
                    .child(content),
            )
            .into_any_element()
    }

    fn watchlist_header(&self, has_playlists: bool, cx: &mut Context<Self>) -> AnyElement {
        let refresh_params = self.launcher_params(cx);
        let refresh = Button::new("watch-refresh")
            .icon(IconName::RefreshCw)
            .label("Refresh")
            .disabled(!has_playlists || self.has_run(RunKind::Refresh))
            .on_click(cx.listener(move |view, _, _, cx| {
                view.start_job("watchlist.refresh", refresh_params.clone(), cx)
            }));
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
                                view.send("watchlist.load", json!({}));
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
        playlists: Option<&Vec<Value>>,
        has_playlists: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let items: Vec<SidebarMenuItem> = playlists
            .into_iter()
            .flatten()
            .enumerate()
            .map(|(index, playlist)| {
                let rename_title = playlist["title"].as_str().unwrap_or("").to_string();
                let item = SidebarMenuItem::new(playlist_title(playlist).to_string())
                    .active(index == self.selected_playlist)
                    .on_click(cx.listener(move |view, _, window, cx| {
                        view.select_playlist(index, rename_title.clone(), window, cx)
                    }));
                if playlist["last_error"].is_string() {
                    item.icon(IconName::TriangleAlert)
                } else {
                    item
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
                    view.send("watchlist.add", json!({"url":url}));
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
        self.watch_page = 0;
        self.playlist_name
            .update(cx, |state, cx| state.set_value(title, window, cx));
        self.cache_visible_thumbnails(cx);
        cx.notify();
    }

    fn playlist_view(&self, playlist: &Value, cx: &mut Context<Self>) -> AnyElement {
        let id = playlist_id(playlist);
        let title = playlist_title(playlist).to_string();
        let items = playlist["items"].as_array();
        let facts = format!(
            "{} · {} items · {}",
            playlist["kind"].as_str().unwrap_or("source"),
            items.map_or(0, Vec::len),
            playlist["last_checked_at"]
                .as_str()
                .map_or_else(|| "not checked".to_string(), |at| format!("checked {at}"))
        );
        let source_url = playlist["url"].as_str().unwrap_or("").to_string();
        let mut tools = div().flex().items_center().gap_1();
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
        let rename_id = id.clone();
        let remove_id = id.clone();
        let remove_title = title.clone();
        tools = tools
            .child(
                Button::new("rename-playlist")
                    .ghost()
                    .small()
                    .label("Rename")
                    .on_click(cx.listener(move |view, _, window, cx| {
                        view.open_rename(rename_id.clone(), window, cx)
                    })),
            )
            .child(
                Button::new("remove-playlist")
                    .ghost()
                    .small()
                    .label("Remove")
                    .text_color(cx.theme().danger)
                    .on_click(cx.listener(move |view, _, window, cx| {
                        view.request_action(
                            PendingAction {
                                title: format!("Remove “{remove_title}”?"),
                                description: "The playlist leaves the watchlist.",
                                confirm: "Remove playlist".into(),
                                destructive: true,
                                command: "watchlist.remove",
                                params: json!({"playlist_id":remove_id}),
                            },
                            window,
                            cx,
                        );
                    })),
            );
        let mut section = div().v_flex().gap_4().child(
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
        if let Some(error) = playlist["last_error"].as_str() {
            section = section.child(Alert::error("playlist-error", error.to_string()));
        }
        section = section.child(
            TabBar::new("filters")
                .pill()
                .small()
                .selected_index(self.filter)
                .children(
                    (0..=Summary::ALL.len()).map(|filter| Tab::new().label(filter_label(filter))),
                )
                .on_click(cx.listener(|view, index: &usize, _, cx| {
                    view.filter = *index;
                    view.watch_page = 0;
                    view.cache_visible_thumbnails(cx);
                    cx.notify();
                })),
        );
        let Some(items) = items else {
            return section.into_any_element();
        };
        let filtered: Vec<&Value> = items
            .iter()
            .filter(|item| matches_filter(item, self.filter))
            .collect();
        if filtered.is_empty() {
            let message = if SourceKind::of(playlist) == SourceKind::Spotify && items.is_empty() {
                "This Spotify source has no tracks. Refresh it to read track names. Set Audio source to Soulseek in Settings to get audio.".to_string()
            } else if items.is_empty() && playlist["last_checked_at"].is_null() {
                "This playlist has not been checked. Select Refresh to read it.".to_string()
            } else if items.is_empty() {
                "This playlist has no videos. Refresh it to check again.".to_string()
            } else {
                format!(
                    "No items have the {} status. Select All to see every item.",
                    filter_label(self.filter)
                )
            };
            return section
                .child(empty_state("Nothing here", message, false))
                .into_any_element();
        }
        let page_count = filtered.len().div_ceil(WATCH_PAGE_SIZE).max(1);
        let current = self.watch_page.min(page_count - 1);
        let mut list = div().v_flex().gap_3();
        for item in filtered
            .into_iter()
            .skip(current * WATCH_PAGE_SIZE)
            .take(WATCH_PAGE_SIZE)
        {
            list = list.child(self.watch_item(item, playlist, cx));
        }
        section = section.child(list);
        if page_count > 1 {
            section = section.child(
                Pagination::new("watch-pages")
                    .current_page(current + 1)
                    .total_pages(page_count)
                    .compact()
                    .on_click(cx.listener(|view, page: &usize, _, cx| {
                        view.watch_page = page.saturating_sub(1);
                        view.cache_visible_thumbnails(cx);
                        cx.notify();
                    })),
            );
        }
        section.into_any_element()
    }

    fn open_rename(&mut self, playlist_id: String, window: &mut Window, cx: &mut Context<Self>) {
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
                            view.send(
                                "watchlist.rename",
                                json!({"playlist_id":playlist_id,"title":title}),
                            );
                            cx.notify();
                        }
                    });
                    true
                })
        });
    }

    fn watch_item(&self, item: &Value, playlist: &Value, cx: &mut Context<Self>) -> AnyElement {
        let title = item["title"].as_str().unwrap_or("Untitled").to_string();
        let position = item["position"].as_u64().unwrap_or(0) as usize;
        let video_id = item_video_id(item);
        let playlist_id = playlist_id(playlist);
        let sheet_key = (playlist_id.clone(), position, video_id.clone());
        let mut card = div()
            .id(("watch-item", position))
            .v_flex()
            .gap_3()
            .p_4()
            .border_1()
            .border_color(cx.theme().border)
            .rounded_md()
            .bg(cx.theme().background)
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_3()
                    .child(
                        div()
                            .min_w_0()
                            .text_sm()
                            .font_semibold()
                            .truncate()
                            .child(title),
                    )
                    .child(
                        Button::new(("item-more", position))
                            .ghost()
                            .small()
                            .icon(IconName::Ellipsis)
                            .tooltip("More")
                            .on_click(cx.listener(move |view, _, window, cx| {
                                view.open_item_sheet(sheet_key.clone(), window, cx)
                            })),
                    ),
            );
        let queued = self.is_queued(&playlist_id, position, &video_id);
        let mut state =
            div()
                .v_flex()
                .gap_1()
                .child(style::stage_track(("stages", position), item, cx));
        if queued {
            state = state.child(style::meta("In the queue", cx));
        }
        if let Some(error) = item["last_error"].as_str() {
            state = state.child(div().text_sm().child(error.to_string()));
        }
        card = card.child(state);
        let primary = item["primary_action"]["action"]
            .as_str()
            .and_then(|action| action.parse::<ItemAction>().ok());
        if let (Some(action), Some(label)) = (primary, item["primary_action"]["label"].as_str()) {
            let enabled = item["actions"][action.as_ref()]["enabled"]
                .as_bool()
                .unwrap_or(true);
            let params = self.item_params(&playlist_id, position, &video_id, action, cx);
            card = card.child(
                div().flex().child(
                    Button::new(("primary-action", position))
                        .small()
                        .label(label.to_string())
                        .disabled(!enabled || queued)
                        .on_click(cx.listener(move |view, _, _, cx| {
                            view.start_job("watchlist.action", params.clone(), cx);
                        })),
                ),
            );
        }
        card.into_any_element()
    }

    fn is_queued(&self, playlist_id: &str, position: usize, video_id: &str) -> bool {
        self.queued_items.contains(&muzik_runner::item_key(
            &json!({"playlist_id":playlist_id,"position":position,"video_id":video_id}),
        ))
    }

    fn item_params(
        &self,
        playlist_id: &str,
        position: usize,
        video_id: &str,
        action: ItemAction,
        cx: &App,
    ) -> Value {
        let mut params = self
            .launcher_params(cx)
            .as_object()
            .cloned()
            .unwrap_or_default();
        params.insert("playlist_id".into(), json!(playlist_id));
        params.insert("position".into(), json!(position));
        params.insert("video_id".into(), json!(video_id));
        params.insert("action".into(), json!(action));
        Value::Object(params)
    }

    fn find_item(&self, key: &(String, usize, String)) -> Option<Value> {
        let playlists = self.watchlist["playlists"].as_array()?;
        let playlist = playlists
            .iter()
            .find(|playlist| playlist_id(playlist) == key.0)?;
        playlist["items"]
            .as_array()?
            .iter()
            .find(|item| {
                item["position"].as_u64() == Some(key.1 as u64) && item_video_id(item) == key.2
            })
            .cloned()
    }

    fn open_item_sheet(
        &mut self,
        key: (String, usize, String),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
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
    item: &Value,
    key: &(String, usize, String),
    view: &WeakEntity<Muzik>,
    cx: &App,
) -> Sheet {
    let title = item["title"].as_str().unwrap_or("Untitled").to_string();
    let spotify = SourceKind::of(item) == SourceKind::Spotify;
    let mut body = div().v_flex().gap_5();
    if let Some(path) = item["thumbnail_path"].as_str() {
        body = body.child(
            img(PathBuf::from(path))
                .w_full()
                .h(px(180.))
                .rounded_md()
                .object_fit(ObjectFit::Cover),
        );
    }
    let mut state = div()
        .v_flex()
        .gap_1()
        .child(style::stage_track("sheet-stages", item, cx));
    if let Some(error) = item["last_error"].as_str() {
        state = state.child(div().text_sm().child(error.to_string()));
    }
    body = body.child(state);
    let states = style::stage_states(item);
    let mut facts = DescriptionList::horizontal()
        .columns(1)
        .label_width(px(110.))
        .item(
            if spotify { "Spotify ID" } else { "YouTube ID" },
            key.2.clone(),
            1,
        );
    for (stage, state) in states {
        facts = facts.item(style::stage_label(stage), style::status_word(state), 1);
    }
    let mut links = div().flex().items_center().gap_2().child(
        Clipboard::new("copy-item-id")
            .value(key.2.clone())
            .tooltip("Copy ID"),
    );
    if let Some(url) = item["video_url"].as_str() {
        let url = url.to_string();
        links = links.child(
            Button::new("open-item")
                .small()
                .icon(IconName::ExternalLink)
                .label(if spotify {
                    "Open in Spotify"
                } else {
                    "Open on YouTube"
                })
                .on_click(move |_, _, cx| cx.open_url(&url)),
        );
    }
    body = body.child(div().v_flex().gap_2().child(facts).child(links));
    let Some(entity) = view.upgrade() else {
        return sheet.title(title).child(body);
    };
    let mut commands = div()
        .v_flex()
        .gap_3()
        .pt_4()
        .border_t_1()
        .border_color(cx.theme().border)
        .child(style::overline("COMMANDS", cx));
    for (index, action) in ItemAction::ALL.iter().copied().enumerate() {
        let availability = &item["actions"][action.as_ref()];
        let enabled = availability["enabled"].as_bool().unwrap_or(true);
        let params = entity
            .read(cx)
            .item_params(&key.0, key.1, &key.2, action, cx);
        let label = action_label(action);
        let item_title = title.clone();
        let view = view.clone();
        let mut row = div().v_flex().gap_1().child(
            Button::new(("item-action", index))
                .small()
                .label(label)
                .disabled(!enabled)
                .on_click(move |_, window, cx| {
                    window.close_sheet(cx);
                    let params = params.clone();
                    let item_title = item_title.clone();
                    let _ = view.update(cx, |view, cx| {
                        if action.replaces_files() {
                            view.request_action(
                                PendingAction {
                                    title: format!("{label} for “{item_title}”?"),
                                    description: REPLACE_WARNING,
                                    confirm: label.into(),
                                    destructive: false,
                                    command: "watchlist.action",
                                    params,
                                },
                                window,
                                cx,
                            );
                        } else {
                            view.start_job("watchlist.action", params, cx);
                        }
                    });
                }),
        );
        if !enabled {
            if let Some(reason) = availability["reason"].as_str() {
                row = row.child(style::meta(reason.to_string(), cx));
            }
        }
        commands = commands.child(row);
    }
    sheet
        .title(title)
        .size(px(420.))
        .child(body.child(commands).overflow_y_scrollbar())
}

pub(crate) fn empty_state(
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

pub(crate) fn playlist_id(playlist: &Value) -> String {
    playlist["id"]
        .as_str()
        .or_else(|| playlist["playlist_id"].as_str())
        .unwrap_or("")
        .to_string()
}

fn playlist_title(playlist: &Value) -> &str {
    playlist["title"]
        .as_str()
        .or_else(|| playlist["playlist_id"].as_str())
        .unwrap_or("Playlist")
}

fn item_video_id(item: &Value) -> String {
    item["video_id"]
        .as_str()
        .or_else(|| item["id"].as_str())
        .unwrap_or("")
        .to_string()
}

pub(crate) fn matches_filter(item: &Value, filter: usize) -> bool {
    let Some(wanted) = filter_summary(filter) else {
        return true;
    };
    item["summary"]
        .as_str()
        .and_then(|summary| summary.parse::<Summary>().ok())
        == Some(wanted)
}
