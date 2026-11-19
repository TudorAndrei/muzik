use super::*;
use crate::watchlist_view::empty_state;
use gpui_kit::component::alert::Alert;
use gpui_kit::component::clipboard::Clipboard;
use gpui_kit::component::table::{Table, TableBody, TableCell, TableHead, TableHeader, TableRow};
use gpui_kit::component::tag::Tag;
use muzik_core::source_reference::SpotifyReference;
use muzik_spotify::PlaylistRef;

fn update_main(
    main: &WeakEntity<Muzik>,
    cx: &mut App,
    change: impl FnOnce(&mut Muzik, &mut Context<Muzik>),
) {
    let _ = main.update(cx, |view, cx| {
        change(view, cx);
        cx.notify();
    });
}

impl Muzik {
    pub(crate) fn services_section(main: WeakEntity<Self>, cx: &App) -> AnyElement {
        let Some(main_view) = main.upgrade() else {
            return div().into_any_element();
        };
        let view = main_view.read(cx);
        let checking = view.reading(Read::Services);
        let services = view
            .services
            .get("services")
            .and_then(Value::as_array)
            .or_else(|| view.services.as_array())
            .cloned()
            .unwrap_or_default();
        let check = Button::new("service-refresh")
            .ghost()
            .small()
            .icon(IconName::RefreshCw)
            .label("Check again")
            .disabled(checking)
            .on_click(move |_, _, cx| {
                let _ = main.update(cx, |view, cx| {
                    view.check_services(cx);
                    cx.notify();
                });
            });
        let missing = services
            .iter()
            .filter(|service| service["available"] == false && service["optional"] != true)
            .count();
        let mut section = div()
            .v_flex()
            .gap_3()
            .w_full()
            .child(div().flex().justify_end().child(check));
        if missing > 0 {
            section = section.child(Alert::warning(
                "services-missing",
                format!(
                    "{missing} required service(s) unavailable. Jobs that need them will fail."
                ),
            ));
        }
        if services.is_empty() {
            return section
                .child(style::meta(
                    if checking {
                        "Checking services…"
                    } else {
                        "No service checks yet."
                    },
                    cx,
                ))
                .into_any_element();
        }
        let mut rows = div().v_flex();
        for (index, service) in services.iter().enumerate() {
            rows = rows.child(service_row(index, service, cx));
        }
        section.child(rows).into_any_element()
    }

    pub(crate) fn spotify_section(main: &WeakEntity<Self>, cx: &App) -> AnyElement {
        let Some(main_view) = main.upgrade() else {
            return div().into_any_element();
        };
        let view = main_view.read(cx);
        let saved_ids: HashSet<&str> = view
            .watchlist
            .playlists
            .iter()
            .map(|playlist| playlist.playlist_id.as_str())
            .collect();
        let connected = view.spotify.connected;
        let has_client_id = !view.spotify.client_id.trim().is_empty();
        let mut section = div()
            .v_flex()
            .gap_4()
            .w_full()
            .child(view.spotify_application(main, cx));
        if let Some(error) = view.spotify.error.as_deref() {
            section = section.child(Alert::error("spotify-error", error.to_string()));
        }
        if view.reading(Read::SpotifyStatus) {
            section = section.child(empty_state(
                "Checking account",
                "Reading the Spotify connection.",
                true,
            ));
        } else if connected {
            section =
                section.child(view.spotify_account(saved_ids.contains("spotify:liked"), main, cx));
        } else if has_client_id {
            section = section.child(view.spotify_connect(main, cx));
        } else {
            section = section.child(style::meta("Save a client ID to connect your account.", cx));
        }
        if !connected {
            return section.into_any_element();
        }
        let playlists: Vec<&PlaylistRef> = view
            .spotify
            .playlists
            .iter()
            .flatten()
            .filter(|playlist| playlist.uri != SpotifyReference::Liked)
            .collect();
        let playlists = if view.reading(Read::SpotifyPlaylists) {
            empty_state("Loading playlists", "Reading your Spotify playlists.", true)
                .into_any_element()
        } else if playlists.is_empty() {
            empty_state(
                "No playlists",
                "No Spotify playlists were found for this account.",
                false,
            )
            .into_any_element()
        } else {
            spotify_playlists_table(&playlists, &saved_ids, main, cx).into_any_element()
        };
        section
            .child(style::section_title("Playlists"))
            .child(playlists)
            .into_any_element()
    }

    fn spotify_application(&self, main: &WeakEntity<Self>, cx: &App) -> Div {
        let redirect = self.spotify.redirect_uri.clone();
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
        let save = main.clone();
        div()
            .v_flex()
            .gap_3()
            .child(style::meta(
                "Create an application in Spotify, add this redirect URI, then save its client ID here.",
                cx,
            ))
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
                                move |_, _, cx| {
                                    update_main(&save, cx, |view, cx| {
                                        let client_id =
                                            view.spotify_client_id.read(cx).value().to_string();
                                        view.set_spotify_client_id(client_id, cx);
                                    });
                                },
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
                div().flex().child(
                    Button::new("spotify-dashboard")
                        .ghost()
                        .icon(IconName::ExternalLink)
                        .label("Open Spotify dashboard")
                        .on_click(|_, _, cx| {
                            cx.open_url("https://developer.spotify.com/dashboard");
                        }),
                ),
            )
    }

    fn spotify_account(&self, liked_saved: bool, main: &WeakEntity<Self>, cx: &App) -> Div {
        let (liked, reload, disconnect) = (main.clone(), main.clone(), main.clone());
        div()
            .v_flex()
            .gap_3()
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(div().text_sm().font_semibold().child("Connected account"))
                    .child(style::meta(
                        self.spotify
                            .account_name
                            .as_deref()
                            .unwrap_or("Spotify account")
                            .to_string(),
                        cx,
                    )),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .child(
                        Button::new("spotify-liked")
                            .primary()
                            .label(if liked_saved {
                                "Liked Songs saved"
                            } else {
                                "Add Liked Songs to watchlist"
                            })
                            .disabled(liked_saved || self.reading(Read::Watchlist))
                            .on_click(move |_, _, cx| {
                                update_main(&liked, cx, |view, cx| {
                                    view.add_source("liked".into(), cx);
                                });
                            }),
                    )
                    .child(
                        Button::new("spotify-reload")
                            .ghost()
                            .icon(IconName::RefreshCw)
                            .label("Reload playlists")
                            .on_click(move |_, _, cx| {
                                update_main(&reload, cx, |view, cx| view.spotify_playlists(cx));
                            }),
                    )
                    .child(
                        Button::new("spotify-disconnect")
                            .ghost()
                            .label("Disconnect")
                            .on_click(move |_, _, cx| {
                                update_main(&disconnect, cx, |view, cx| view.spotify_logout(cx));
                            }),
                    ),
            )
    }

    fn spotify_connect(&self, main: &WeakEntity<Self>, cx: &App) -> Div {
        let connect = main.clone();
        div()
            .v_flex()
            .gap_2()
            .child(
                div().flex().child(
                    Button::new("spotify-connect")
                        .primary()
                        .label("Connect to Spotify")
                        .disabled(self.activity.has_run(RunKind::SpotifyLogin))
                        .on_click(move |_, _, cx| {
                            update_main(&connect, cx, Self::spotify_login);
                        }),
                ),
            )
            .child(style::meta(
                "muzik opens your browser. Approve access, then return here.",
                cx,
            ))
    }
}

fn service_row(index: usize, service: &Value, cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    let (word, tag) = match service["available"].as_bool() {
        Some(true) => ("Available", Tag::success()),
        Some(false) if service["optional"] == true => ("Optional", Tag::warning()),
        Some(false) => ("Unavailable", Tag::danger()),
        None => ("Not set up", Tag::secondary().outline()),
    };
    div()
        .id(("service", index))
        .flex()
        .items_center()
        .gap_3()
        .py_2()
        .when(index > 0, |row| row.border_t_1().border_color(theme.border))
        .child(
            div()
                .w(px(110.))
                .flex_none()
                .text_sm()
                .font_semibold()
                .child(service["name"].as_str().unwrap_or("Service").to_string()),
        )
        .child(
            style::meta(service["detail"].as_str().unwrap_or("").to_string(), cx)
                .flex_1()
                .min_w_0(),
        )
        .child(div().flex().flex_none().child(tag.small().child(word)))
}

fn spotify_playlists_table(
    playlists: &[&PlaylistRef],
    saved_ids: &HashSet<&str>,
    main: &WeakEntity<Muzik>,
    cx: &App,
) -> Table {
    let mut body = TableBody::new();
    for playlist in playlists {
        let uri = playlist.uri.to_string();
        let saved = saved_ids.contains(uri.as_str());
        let add = main.clone();
        body = body.child(
            TableRow::new()
                .id(SharedString::from(playlist.uri.to_string()))
                .child(
                    TableCell::new().child(div().min_w_0().truncate().child(playlist.name.clone())),
                )
                .child(TableCell::new().child(style::meta(playlist.owner.clone(), cx)))
                .child(
                    TableCell::new()
                        .text_right()
                        .child(style::mono(playlist.total.unwrap_or(0).to_string(), cx)),
                )
                .child(
                    TableCell::new().text_right().child(
                        Button::new("spotify-add")
                            .small()
                            .label(if saved { "Saved" } else { "Add to watchlist" })
                            .disabled(saved)
                            .on_click(move |_, _, cx| {
                                let uri = uri.clone();
                                update_main(&add, cx, |view, cx| view.add_source(uri, cx));
                            }),
                    ),
                ),
        );
    }
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
        .child(body)
}

pub fn page_frame() -> Div {
    div().v_flex().gap_4().p_6().w_full().max_w(px(960.))
}

pub fn page_header(title: &'static str, action: impl IntoElement) -> Div {
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap_3()
        .child(style::page_title(title))
        .child(action)
}

pub fn table_frame() -> Div {
    div()
        .flex_1()
        .min_h(px(320.))
        .on_action(|_: &FocusNext, window, cx| window.focus_next(cx))
        .on_action(|_: &FocusPrev, window, cx| window.focus_prev(cx))
}

pub fn page_scroll(page: Div) -> AnyElement {
    div()
        .flex()
        .justify_center()
        .flex_1()
        .min_w_0()
        .overflow_y_scrollbar()
        .child(page)
        .into_any_element()
}
