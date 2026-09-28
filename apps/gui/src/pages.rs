use super::*;
use crate::watchlist_view::empty_state;
use gpui_kit::component::alert::Alert;
use gpui_kit::component::clipboard::Clipboard;
use gpui_kit::component::table::{Table, TableBody, TableCell, TableHead, TableHeader, TableRow};

impl Muzik {
    pub(crate) fn library(&self, cx: &mut Context<Self>) -> AnyElement {
        let scanning = self
            .pending
            .values()
            .any(|command| command == "library.scan");
        let items = self.library["items"]
            .as_array()
            .or_else(|| self.library.as_array());
        let mut page = page_frame().child(page_header(
            "Downloaded audio",
            Button::new("library-refresh")
                .ghost()
                .icon(IconName::RefreshCw)
                .label("Refresh")
                .disabled(scanning)
                .on_click(cx.listener(|view, _, _, cx| {
                    view.scan_library(cx);
                    cx.notify();
                })),
        ));
        let output = self.library["output"].as_str().unwrap_or("").to_string();
        match items {
            Some(items) if !items.is_empty() => {
                page = page.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(style::mono(output, cx).text_color(cx.theme().muted_foreground))
                        .child(style::meta(
                            format!(
                                "· {} files · {}",
                                items.len(),
                                self.library["total_size"].as_str().unwrap_or("0 B")
                            ),
                            cx,
                        )),
                );
                let mut body = TableBody::new();
                for item in items {
                    body =
                        body.child(
                            TableRow::new()
                                .child(TableCell::new().child(div().min_w_0().truncate().child(
                                    item["title"].as_str().unwrap_or("Audio file").to_string(),
                                )))
                                .child(TableCell::new().child(style::mono(
                                    item["ext"].as_str().unwrap_or("").to_string(),
                                    cx,
                                )))
                                .child(TableCell::new().text_right().child(style::mono(
                                    item["size_label"].as_str().unwrap_or("").to_string(),
                                    cx,
                                )))
                                .child(TableCell::new().child(style::mono(
                                    item["modified"].as_str().unwrap_or("").to_string(),
                                    cx,
                                ))),
                        );
                }
                page = page.child(
                    Table::new()
                        .accessibility_label("Downloaded audio files")
                        .child(
                            TableHeader::new().child(
                                TableRow::new()
                                    .child(TableHead::new().child("Title"))
                                    .child(TableHead::new().child("Format"))
                                    .child(TableHead::new().text_right().child("Size"))
                                    .child(TableHead::new().child("Modified")),
                            ),
                        )
                        .child(body),
                );
            }
            _ if scanning => {
                page = page.child(empty_state(
                    "Scanning downloads",
                    "Reading the download folder.",
                    true,
                ));
            }
            _ => {
                page = page.child(empty_state(
                    "No downloads yet",
                    "Run a workflow, or set the download folder in Config.",
                    false,
                ));
            }
        }
        page_scroll(page)
    }

    pub(crate) fn settings(&self, cx: &mut Context<Self>) -> AnyElement {
        let checking = self
            .pending
            .values()
            .any(|command| command == "services.check");
        let services = self.services["services"]
            .as_array()
            .or_else(|| self.services.as_array());
        let mut page = page_frame().child(page_header(
            "Services",
            Button::new("service-refresh")
                .ghost()
                .icon(IconName::RefreshCw)
                .label("Check again")
                .disabled(checking)
                .on_click(cx.listener(|view, _, _, cx| {
                    view.send("services.check", json!({}));
                    cx.notify();
                })),
        ));
        let Some(services) = services.filter(|all| !all.is_empty()) else {
            return page_scroll(page.child(if checking {
                empty_state(
                    "Checking services",
                    "Looking for the tools muzik uses.",
                    true,
                )
            } else {
                empty_state(
                    "No service checks",
                    "Select Check again to look for the tools muzik uses.",
                    false,
                )
            }));
        };
        let missing = services
            .iter()
            .filter(|service| service["available"] == false && service["optional"] != true)
            .count();
        page = page.child(if missing == 0 {
            style::meta("All required services are available.", cx).into_any_element()
        } else {
            Alert::warning(
                "services-missing",
                format!(
                    "{missing} required service(s) unavailable. Jobs that need them will fail."
                ),
            )
            .into_any_element()
        });
        let mut body = TableBody::new();
        for service in services {
            let status = match service["available"].as_bool() {
                Some(true) => Tag::success().child("Available"),
                Some(false) if service["optional"] == true => {
                    Tag::warning().child("Optional · unavailable")
                }
                Some(false) => Tag::danger().child("Unavailable"),
                None => Tag::secondary().child("Not configured"),
            };
            body = body.child(
                TableRow::new()
                    .child(
                        TableCell::new().child(
                            div()
                                .text_sm()
                                .font_semibold()
                                .child(service["name"].as_str().unwrap_or("Service").to_string()),
                        ),
                    )
                    .child(TableCell::new().child(style::meta(
                        service["detail"].as_str().unwrap_or("").to_string(),
                        cx,
                    )))
                    .child(TableCell::new().text_right().child(status)),
            );
        }
        page_scroll(
            page.child(
                Table::new()
                    .accessibility_label("Services")
                    .child(
                        TableHeader::new().child(
                            TableRow::new()
                                .child(TableHead::new().child("Service"))
                                .child(TableHead::new().child("Detail"))
                                .child(TableHead::new().text_right().child("Status")),
                        ),
                    )
                    .child(body),
            ),
        )
    }

    pub(crate) fn spotify(&self, cx: &mut Context<Self>) -> AnyElement {
        let saved_ids: HashSet<&str> = self.watchlist["playlists"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|playlist| playlist["playlist_id"].as_str())
            .collect();
        let liked_saved = saved_ids.contains("spotify:liked");
        let connected = self.spotify["connected"] == true;
        let has_client_id = self.spotify["client_id"]
            .as_str()
            .is_some_and(|id| !id.trim().is_empty());
        let redirect = self.spotify["redirect_uri"]
            .as_str()
            .unwrap_or("")
            .to_string();
        let checking = self
            .pending
            .values()
            .any(|command| command == "spotify.status");
        let loading_playlists = self
            .pending
            .values()
            .any(|command| command == "spotify.playlists");
        let mut redirect_row = div()
            .flex()
            .items_center()
            .gap_2()
            .child(style::mono(redirect.clone(), cx));
        if !redirect.is_empty() {
            redirect_row = redirect_row.child(
                Clipboard::new("copy-redirect")
                    .value(redirect)
                    .tooltip("Copy redirect URI"),
            );
        }
        let application = GroupBox::new()
            .id("spotify-application")
            .title("YOUR SPOTIFY APPLICATION")
            .outline()
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child("Create an application in Spotify, add this redirect URI, then save its client ID here."),
            )
            .child(
                div()
                    .v_flex()
                    .gap_1()
                    .child(div().text_sm().font_semibold().child("Client ID"))
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(
                                div()
                                    .flex_1()
                                    .max_w(px(500.))
                                    .child(Input::new(&self.spotify_client_id)),
                            )
                            .child(Button::new("spotify-save").label("Save client ID").on_click(
                                cx.listener(|view, _, _, cx| {
                                    let client_id =
                                        view.spotify_client_id.read(cx).value().to_string();
                                    view.send(
                                        "spotify.set_client_id",
                                        json!({"client_id":client_id}),
                                    );
                                    cx.notify();
                                }),
                            )),
                    ),
            )
            .child(
                div()
                    .v_flex()
                    .gap_1()
                    .child(div().text_sm().font_semibold().child("Redirect URI"))
                    .child(redirect_row)
                    .child(style::meta(
                        "Use the exact URI. localhost and 127.0.0.1 are different.",
                        cx,
                    )),
            )
            .child(
                div().child(
                    Button::new("spotify-dashboard")
                        .ghost()
                        .icon(IconName::ExternalLink)
                        .label("Open Spotify dashboard")
                        .on_click(cx.listener(|_, _, _, cx| {
                            cx.open_url("https://developer.spotify.com/dashboard")
                        })),
                ),
            );
        let mut page = page_frame()
            .child(style::page_title("Spotify"))
            .child(application);
        if let Some(error) = self.spotify["error"].as_str() {
            page = page.child(Alert::error("spotify-error", error.to_string()));
        }
        if checking {
            page = page.child(empty_state(
                "Checking account",
                "Reading the Spotify connection.",
                true,
            ));
        } else if connected {
            page = page.child(
                GroupBox::new()
                    .id("spotify-account")
                    .title("CONNECTED ACCOUNT")
                    .outline()
                    .child(
                        DescriptionList::horizontal().label_width(px(120.)).item(
                            "Account",
                            self.spotify["account_name"]
                                .as_str()
                                .unwrap_or("Spotify account")
                                .to_string(),
                            1,
                        ),
                    )
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(
                                Button::new("spotify-liked")
                                    .primary()
                                    .label(if liked_saved {
                                        "Liked Songs saved"
                                    } else {
                                        "Add Liked Songs to watchlist"
                                    })
                                    .disabled(
                                        liked_saved
                                            || self
                                                .pending
                                                .values()
                                                .any(|command| command == "watchlist.load"),
                                    )
                                    .on_click(cx.listener(|view, _, _, cx| {
                                        view.send("watchlist.add", json!({"url":"liked"}));
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new("spotify-reload")
                                    .ghost()
                                    .icon(IconName::RefreshCw)
                                    .label("Reload playlists")
                                    .on_click(cx.listener(|view, _, _, cx| {
                                        view.send("spotify.playlists", json!({}));
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new("spotify-disconnect")
                                    .ghost()
                                    .label("Disconnect")
                                    .on_click(cx.listener(|view, _, _, cx| {
                                        view.send("spotify.logout", json!({}));
                                        cx.notify();
                                    })),
                            ),
                    ),
            );
        } else if has_client_id {
            page = page.child(
                div()
                    .v_flex()
                    .gap_2()
                    .child(
                        div().child(
                            Button::new("spotify-connect")
                                .primary()
                                .label("Connect to Spotify")
                                .disabled(self.job_kind.is_some())
                                .on_click(cx.listener(|view, _, _, cx| {
                                    view.start_job("spotify.login", json!({}), cx);
                                    cx.notify();
                                })),
                        ),
                    )
                    .child(style::meta(
                        "muzik opens your browser. Approve access, then return here.",
                        cx,
                    )),
            );
        } else {
            page = page.child(style::meta("Save a client ID to connect your account.", cx));
        }
        if !connected {
            return page_scroll(page);
        }
        let playlists: Vec<&Value> = self.spotify["playlists"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|playlist| playlist["uri"] != "spotify:liked")
            .collect();
        if loading_playlists {
            return page_scroll(page.child(empty_state(
                "Loading playlists",
                "Reading your Spotify playlists.",
                true,
            )));
        }
        if playlists.is_empty() {
            return page_scroll(page.child(empty_state(
                "No playlists",
                "No Spotify playlists were found for this account.",
                false,
            )));
        }
        let mut body = TableBody::new();
        for (index, playlist) in playlists.into_iter().enumerate() {
            let uri = playlist["uri"].as_str().unwrap_or("").to_string();
            let saved = saved_ids.contains(uri.as_str());
            body =
                body.child(
                    TableRow::new()
                        .child(
                            TableCell::new().child(div().min_w_0().truncate().child(
                                playlist["name"].as_str().unwrap_or("Playlist").to_string(),
                            )),
                        )
                        .child(TableCell::new().child(style::meta(
                            playlist["owner"].as_str().unwrap_or("Spotify").to_string(),
                            cx,
                        )))
                        .child(TableCell::new().text_right().child(style::mono(
                            playlist["total"].as_u64().unwrap_or(0).to_string(),
                            cx,
                        )))
                        .child(
                            TableCell::new().text_right().child(
                                Button::new(("spotify-add", index))
                                    .small()
                                    .label(if saved { "Saved" } else { "Add to watchlist" })
                                    .disabled(saved)
                                    .on_click(cx.listener(move |view, _, _, cx| {
                                        view.send("watchlist.add", json!({"url":uri}));
                                        cx.notify();
                                    })),
                            ),
                        ),
                );
        }
        page_scroll(
            page.child(style::section_title("Playlists")).child(
                Table::new()
                    .accessibility_label("Spotify playlists")
                    .child(
                        TableHeader::new().child(
                            TableRow::new()
                                .child(TableHead::new().child("Name"))
                                .child(TableHead::new().child("Owner"))
                                .child(TableHead::new().text_right().child("Tracks"))
                                .child(TableHead::new()),
                        ),
                    )
                    .child(body),
            ),
        )
    }
}

fn page_frame() -> Div {
    div().v_flex().gap_4().p_6().w_full().max_w(px(960.))
}

fn page_header(title: &'static str, action: impl IntoElement) -> Div {
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap_3()
        .child(style::page_title(title))
        .child(action)
}

fn page_scroll(page: Div) -> AnyElement {
    div()
        .flex()
        .justify_center()
        .flex_1()
        .overflow_y_scrollbar()
        .child(page)
        .into_any_element()
}
