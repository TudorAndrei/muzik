use super::*;
use crate::backend::{Backend, ItemRequest, SoulseekForm};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Read {
    Watchlist,
    Library,
    Services,
    SpotifyStatus,
    SpotifyPlaylists,
}

impl Read {
    const fn name(self) -> &'static str {
        match self {
            Self::Watchlist => "watchlist.load",
            Self::Library => "library.scan",
            Self::Services => "services.check",
            Self::SpotifyStatus => "spotify.status",
            Self::SpotifyPlaylists => "spotify.playlists",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Call {
    Hello,
    JobsList,
    ConfigSave,
    SoulseekSave,
    SoulseekGet,
    BandcampGet,
    BandcampSave,
    BandcampLogout,
    WatchlistAdd,
    WatchlistRename,
    WatchlistRemove,
    WorkflowStart,
    WatchlistRefresh,
    WatchlistAction,
    JobCancel,
    DecisionReply,
    JobsAnswer,
    ThumbnailsCache,
    SpotifySetClientId,
    SpotifyLogout,
    SpotifyLogin,
    Read(Read),
}

impl Call {
    const fn name(self) -> &'static str {
        match self {
            Self::Hello => "hello",
            Self::JobsList => "jobs.list",
            Self::ConfigSave => "config.save",
            Self::SoulseekSave => "soulseek.save",
            Self::SoulseekGet => "soulseek.get",
            Self::BandcampGet => "bandcamp.get",
            Self::BandcampSave => "bandcamp.save",
            Self::BandcampLogout => "bandcamp.logout",
            Self::WatchlistAdd => "watchlist.add",
            Self::WatchlistRename => "watchlist.rename",
            Self::WatchlistRemove => "watchlist.remove",
            Self::WorkflowStart => "workflow.start",
            Self::WatchlistRefresh => "watchlist.refresh",
            Self::WatchlistAction => "watchlist.action",
            Self::JobCancel => "job.cancel",
            Self::DecisionReply => "decision.reply",
            Self::JobsAnswer => "jobs.answer",
            Self::ThumbnailsCache => "thumbnails.cache",
            Self::SpotifySetClientId => "spotify.set_client_id",
            Self::SpotifyLogout => "spotify.logout",
            Self::SpotifyLogin => "spotify.login",
            Self::Read(read) => read.name(),
        }
    }

    const fn reports_in_settings(self) -> bool {
        match self {
            Self::ConfigSave | Self::SoulseekSave | Self::BandcampSave | Self::BandcampLogout => {
                true
            }
            Self::Hello
            | Self::JobsList
            | Self::SoulseekGet
            | Self::BandcampGet
            | Self::WatchlistAdd
            | Self::WatchlistRename
            | Self::WatchlistRemove
            | Self::WorkflowStart
            | Self::WatchlistRefresh
            | Self::WatchlistAction
            | Self::JobCancel
            | Self::DecisionReply
            | Self::JobsAnswer
            | Self::ThumbnailsCache
            | Self::SpotifySetClientId
            | Self::SpotifyLogout
            | Self::SpotifyLogin
            | Self::Read(_) => false,
        }
    }
}

#[derive(Clone)]
pub enum Command {
    Cancel(String),
    RemoveSource(String),
    RunItem(ItemRequest),
}

#[derive(Clone)]
pub struct PendingAction {
    pub(crate) title: String,
    pub(crate) description: &'static str,
    pub(crate) confirm: String,
    pub(crate) destructive: bool,
    pub(crate) command: Command,
}

impl Muzik {
    fn spawn_call<R: Send + 'static>(
        &mut self,
        call: Call,
        cx: &Context<Self>,
        work: impl FnOnce(&Backend) -> R + Send + 'static,
        finish: impl FnOnce(&mut Self, R, &mut Window, &mut Context<Self>) + 'static,
    ) {
        let Some(backend) = self.backend.clone() else {
            self.status = "Backend is not available".into();
            return;
        };
        self.status = format!("{} requested", call.name());
        let handle = self.window;
        let work = cx.background_spawn(async move { work(&backend) });
        cx.spawn(async move |view, cx| {
            let result = work.await;
            let _ = cx.update_window(handle, |_, window, cx| {
                view.update(cx, |view, cx| {
                    finish(view, result, window, cx);
                    cx.notify();
                })
            });
        })
        .detach();
    }

    fn call<T: Send + 'static>(
        &mut self,
        call: Call,
        cx: &Context<Self>,
        work: impl FnOnce(&Backend) -> anyhow::Result<T> + Send + 'static,
        done: impl FnOnce(&mut Self, T, &mut Window, &mut Context<Self>) + 'static,
    ) {
        self.spawn_call(
            call,
            cx,
            work,
            move |view, result, window, cx| match result {
                Ok(value) => done(view, value, window, cx),
                Err(error) => view.failed(call, error.to_string()),
            },
        );
    }

    fn read<T: Send + 'static>(
        &mut self,
        read: Read,
        cx: &Context<Self>,
        work: impl FnOnce(&Backend) -> anyhow::Result<T> + Send + 'static,
        done: impl FnOnce(&mut Self, T, &mut Window, &mut Context<Self>) + 'static,
    ) {
        if self.backend.is_none() {
            self.status = "Backend is not available".into();
            return;
        }
        self.read_serial = self.read_serial.wrapping_add(1);
        let serial = self.read_serial;
        self.reads.insert(read, serial);
        self.spawn_call(
            Call::Read(read),
            cx,
            work,
            move |view, result, window, cx| {
                if view.reads.get(&read) != Some(&serial) {
                    return;
                }
                view.reads.remove(&read);
                match result {
                    Ok(value) => done(view, value, window, cx),
                    Err(error) => view.failed(Call::Read(read), error.to_string()),
                }
            },
        );
    }

    pub(crate) fn reading(&self, read: Read) -> bool {
        self.reads.contains_key(&read)
    }

    fn failed(&mut self, call: Call, message: String) {
        self.status = message;
        self.error = Some(self.status.clone());
        if call.reports_in_settings() {
            self.config_status.borrow_mut().clone_from(&self.status);
        }
    }

    fn completed(&mut self, call: Call) {
        self.status = format!("{} complete", call.name());
    }

    pub(crate) fn hello(&mut self, cx: &Context<Self>) {
        self.call(
            Call::Hello,
            cx,
            |backend| Ok(backend.defaults()),
            |view, defaults, _, cx| {
                view.status = "Backend ready".into();
                view.apply_defaults(defaults, cx);
            },
        );
    }

    pub(crate) fn load_jobs(&mut self, cx: &Context<Self>) {
        self.call(
            Call::JobsList,
            cx,
            |backend| Ok(backend.jobs()),
            |view, jobs, window, cx| {
                let previous = view.activity.decision().cloned();
                view.activity.apply_jobs(&jobs);
                if previous.as_ref() != view.activity.decision() {
                    view.prepare_decision(window, cx);
                }
                view.sync_watch_table(cx);
                view.gates = jobs.get("gates").cloned().unwrap_or_default();
            },
        );
    }

    pub(crate) fn save_config(
        &mut self,
        defaults: GuiDefaults,
        soulseek: Option<SoulseekForm>,
        cx: &mut Context<Self>,
    ) {
        self.error = None;
        self.spawn_call(
            Call::ConfigSave,
            cx,
            move |backend| {
                let saved = backend.save_defaults(&defaults);
                let account = soulseek.map(|form| backend.save_soulseek(&form));
                (saved, account)
            },
            |view, (saved, account), window, cx| {
                match saved {
                    Ok(defaults) => {
                        view.status = "Config saved".into();
                        view.error = None;
                        view.config_status.borrow_mut().clone_from(&view.status);
                        view.apply_defaults(defaults, cx);
                    }
                    Err(error) => view.failed(Call::ConfigSave, error.to_string()),
                }
                match account {
                    Some(Ok(account)) => {
                        view.show_soulseek(&account, window, cx);
                        *view.config_status.borrow_mut() =
                            "Config and Soulseek account saved".into();
                        view.check_services(cx);
                    }
                    Some(Err(error)) => view.failed(Call::SoulseekSave, error.to_string()),
                    None => {}
                }
            },
        );
        cx.notify();
    }

    fn show_soulseek(&self, account: &Value, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(view) = self.config_view.clone() {
            view.update(cx, |view, cx| view.set_soulseek(account, window, cx));
        }
    }

    fn show_bandcamp(&self, settings: &Value, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(view) = self.config_view.clone() {
            view.update(cx, |view, cx| view.set_bandcamp(settings, window, cx));
        }
    }

    pub(crate) fn load_accounts(&mut self, cx: &Context<Self>) {
        self.call(
            Call::SoulseekGet,
            cx,
            Backend::soulseek_account,
            |view, account, window, cx| view.show_soulseek(&account, window, cx),
        );
        self.call(
            Call::BandcampGet,
            cx,
            |backend| Ok(backend.bandcamp()),
            |view, settings, window, cx| view.show_bandcamp(&settings, window, cx),
        );
    }

    pub(crate) fn save_bandcamp(&mut self, user: String, cookies: String, cx: &mut Context<Self>) {
        self.error = None;
        self.call(
            Call::BandcampSave,
            cx,
            move |backend| backend.save_bandcamp(&user, &cookies),
            |view, settings, window, cx| {
                view.show_bandcamp(&settings, window, cx);
                *view.config_status.borrow_mut() = "Bandcamp login saved".into();
                view.load_watchlist(cx);
            },
        );
        cx.notify();
    }

    pub(crate) fn logout_bandcamp(&mut self, cx: &mut Context<Self>) {
        self.error = None;
        self.call(
            Call::BandcampLogout,
            cx,
            Backend::logout_bandcamp,
            |view, settings, window, cx| {
                view.show_bandcamp(&settings, window, cx);
                *view.config_status.borrow_mut() = "Bandcamp login removed".into();
            },
        );
        cx.notify();
    }

    pub(crate) fn check_services(&mut self, cx: &Context<Self>) {
        self.read(
            Read::Services,
            cx,
            |backend| Ok(backend.services()),
            |view, services, _, _| {
                view.services = services;
                view.status = "Services checked".into();
            },
        );
    }

    pub(crate) fn load_library(&mut self, cx: &mut Context<Self>) {
        let config = self
            .defaults
            .as_ref()
            .map(|defaults| defaults.config.clone())
            .filter(|config| !config.as_os_str().is_empty());
        self.read(
            Read::Library,
            cx,
            move |backend| backend.library(config.as_deref()),
            |view, library, _, cx| {
                view.apply_library(library, cx);
                view.status = "Library ready".into();
            },
        );
        if self.library.is_none() && self.reading(Read::Library) {
            self.library_table.update(cx, |state, cx| {
                state.delegate_mut().set_loading(true);
                cx.notify();
            });
        }
    }

    pub(crate) fn load_watchlist(&mut self, cx: &Context<Self>) {
        self.read(
            Read::Watchlist,
            cx,
            Backend::load_watchlist,
            |view, (saved, check), window, cx| {
                view.replace_watchlist(saved, window, cx);
                view.cache_visible_thumbnails(cx);
                view.status = "Watchlist ready".into();
                cx.background_spawn(async move { check.run() }).detach();
            },
        );
    }

    pub(crate) fn add_source(&mut self, url: String, cx: &Context<Self>) {
        self.call(
            Call::WatchlistAdd,
            cx,
            move |backend| backend.add_source(&url).map(drop),
            |view, (), window, cx| {
                view.watch_url
                    .update(cx, |state, cx| state.set_value("", window, cx));
                view.load_watchlist(cx);
            },
        );
    }

    pub(crate) fn rename_source(&mut self, playlist_id: String, title: String, cx: &Context<Self>) {
        self.call(
            Call::WatchlistRename,
            cx,
            move |backend| backend.rename_source(&playlist_id, &title),
            |view, _, _, cx| view.load_watchlist(cx),
        );
    }

    fn remove_source(&mut self, playlist_id: String, cx: &Context<Self>) {
        self.call(
            Call::WatchlistRemove,
            cx,
            move |backend| backend.remove_source(&playlist_id),
            |view, _, _, cx| view.load_watchlist(cx),
        );
    }

    fn queued(
        &mut self,
        call: Call,
        cx: &mut Context<Self>,
        work: impl FnOnce(&Backend) -> anyhow::Result<String> + Send + 'static,
    ) {
        self.error = None;
        self.call(call, cx, work, |view, _, _, _| {
            view.status = "Added to the queue".into();
        });
        cx.notify();
    }

    pub(crate) fn start_workflow(&mut self, params: Value, cx: &mut Context<Self>) {
        self.queued(Call::WorkflowStart, cx, move |backend| {
            backend.start_workflow(&params)
        });
    }

    pub(crate) fn refresh(&mut self, source: Option<(String, String)>, cx: &mut Context<Self>) {
        let source = source.filter(|(id, _)| !id.is_empty());
        self.queued(Call::WatchlistRefresh, cx, move |backend| {
            backend.refresh(
                source
                    .as_ref()
                    .map(|(id, title)| (id.as_str(), title.as_str())),
            )
        });
    }

    pub(crate) fn run_item(&mut self, item: ItemRequest, cx: &mut Context<Self>) {
        self.queued(Call::WatchlistAction, cx, move |backend| {
            backend.run_item(&item)
        });
    }

    pub(crate) fn cancel_job(&mut self, job_id: String, cx: &Context<Self>) {
        self.call(
            Call::JobCancel,
            cx,
            move |backend| backend.cancel(&job_id),
            |view, (), _, _| view.completed(Call::JobCancel),
        );
    }

    pub(crate) fn send_reply(&mut self, decision_id: String, value: Value, cx: &Context<Self>) {
        self.call(
            Call::DecisionReply,
            cx,
            move |backend| backend.reply(&decision_id, value),
            |view, (), _, _| view.completed(Call::DecisionReply),
        );
    }

    pub(crate) fn send_answer(&mut self, id: Option<i64>, value: Value, cx: &Context<Self>) {
        let Some(id) = id else {
            self.failed(Call::JobsAnswer, "id and value are required.".into());
            return;
        };
        self.call(
            Call::JobsAnswer,
            cx,
            move |backend| backend.answer(id, &value),
            |view, _, _, _| view.status = "Answer saved; the item is queued".into(),
        );
    }

    pub(crate) fn cache_thumbnails(&mut self, video_ids: Vec<String>, cx: &Context<Self>) {
        self.call(
            Call::ThumbnailsCache,
            cx,
            move |backend| Ok(backend.cache_thumbnails(video_ids)),
            |view, data, _, _| {
                if let Some(data) = data {
                    view.merge_thumbnail_results(&data);
                }
                view.completed(Call::ThumbnailsCache);
            },
        );
    }

    fn forget_spotify(&mut self) {
        self.reads.remove(&Read::SpotifyStatus);
        self.reads.remove(&Read::SpotifyPlaylists);
        if let Some(spotify) = self.spotify.as_object_mut() {
            spotify.remove("playlists");
        }
    }

    pub(crate) fn spotify_status(&mut self, cx: &Context<Self>) {
        self.reads.remove(&Read::SpotifyPlaylists);
        self.read(
            Read::SpotifyStatus,
            cx,
            Backend::spotify_status,
            |view, status, window, cx| {
                view.spotify = status;
                if view.spotify_client_id.read(cx).value().is_empty()
                    && let Some(client_id) = view.spotify.get("client_id").and_then(Value::as_str)
                {
                    let client_id = client_id.to_string();
                    view.spotify_client_id
                        .update(cx, |state, cx| state.set_value(client_id, window, cx));
                }
                if view.spotify.get("connected").and_then(Value::as_bool) == Some(true) {
                    view.spotify_playlists(cx);
                }
                view.status = "Spotify ready".into();
            },
        );
    }

    pub(crate) fn spotify_playlists(&mut self, cx: &Context<Self>) {
        self.read(
            Read::SpotifyPlaylists,
            cx,
            Backend::spotify_playlists,
            |view, playlists, _, _| {
                if let Some(spotify) = view.spotify.as_object_mut().filter(|spotify| {
                    spotify.get("connected").and_then(Value::as_bool) == Some(true)
                }) {
                    spotify.insert("playlists".into(), playlists);
                }
                view.status = "Spotify ready".into();
            },
        );
    }

    pub(crate) fn set_spotify_client_id(&mut self, client_id: String, cx: &Context<Self>) {
        self.forget_spotify();
        self.call(
            Call::SpotifySetClientId,
            cx,
            move |backend| backend.set_spotify_client_id(&client_id),
            |view, _, _, cx| {
                view.spotify_status(cx);
                view.status = "Spotify ready".into();
            },
        );
    }

    pub(crate) fn spotify_logout(&mut self, cx: &Context<Self>) {
        self.forget_spotify();
        self.call(
            Call::SpotifyLogout,
            cx,
            Backend::spotify_logout,
            |view, _, _, cx| {
                view.spotify_status(cx);
                view.status = "Spotify ready".into();
            },
        );
    }

    pub(crate) fn spotify_login(&mut self, cx: &mut Context<Self>) {
        self.forget_spotify();
        self.error = None;
        let Some(backend) = self.backend.clone() else {
            self.status = "Backend is not available".into();
            return;
        };
        match backend.spotify_login() {
            Ok(login) => {
                self.activity.start_login(login.job_id());
                self.spawn_call(
                    Call::SpotifyLogin,
                    cx,
                    move |_| login.run(),
                    |view, event, window, cx| view.app_events(vec![event], window, cx),
                );
            }
            Err(error) => self.failed(Call::SpotifyLogin, error.to_string()),
        }
        cx.notify();
    }

    pub(crate) fn run_action(&mut self, action: PendingAction, cx: &mut Context<Self>) {
        match action.command {
            Command::RunItem(item) => self.run_item(item, cx),
            Command::Cancel(job_id) => self.cancel_job(job_id, cx),
            Command::RemoveSource(playlist_id) => self.remove_source(playlist_id, cx),
        }
        cx.notify();
    }
}
