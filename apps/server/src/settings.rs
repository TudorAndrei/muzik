use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

use muzik_core::{
    AudioFallback, AudioSource, DuplicatePolicy, MetadataSource, PlaylistFormat, PreferredAudio,
    QualityPolicy, app_config::GuiDefaults,
};
use muzik_runner::{
    library_tasks::LibraryTask,
    service::{Service, SoulseekForm, SpotifyStatus},
    setup::{ServiceStatus, SoulseekProfile},
};
use serde::Deserialize;
use topcoat::{
    Result,
    context::Cx,
    router::{
        content::Form,
        error::{SeeOther, see_other},
        href, page, query_params, route,
    },
    view::{View, component, view},
};

use crate::{
    activity::truncate,
    auth,
    notice::{Notice, describe, done},
    state::{backend, hub},
};

const SOURCE_CHOICES: [Choice; 6] = [
    Choice::AudioSource,
    Choice::MetadataSource,
    Choice::Prefer,
    Choice::Fallback,
    Choice::QualityPolicy,
    Choice::Duplicates,
];

const PROCESSING_FLAGS: [Flag; 9] = [
    Flag::Review,
    Flag::NoSplit,
    Flag::NoOrganize,
    Flag::Import,
    Flag::TagOnly,
    Flag::DryRun,
    Flag::KeepSource,
    Flag::Force,
    Flag::Interactive,
];

const IMPORT_FLAGS: [Flag; 2] = [Flag::ReplayGainAfterImport, Flag::CoversAfterImport];

const JOBS: &str = "jobs";
const MIN_BITRATE: &str = "min_bitrate";
const AGENT_MODEL: &str = "agent_model";

const BANDCAMP_HELP: &str = "Muzik uses your Bandcamp login to read your collection and download your purchases in FLAC. To get the login cookie: log in to bandcamp.com in your browser, open the developer tools, open Storage (Firefox, Zen) or Application (Chrome), select Cookies, then https://bandcamp.com, and copy the Value of the identity row. Paste it below and select Save Bandcamp login. Muzik finds your user name. A full Cookie header or a cookies.txt file also works. The cookie stays on the server. Do not share it.";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PathField {
    Output,
    Splits,
    Config,
}

impl PathField {
    const ALL: [Self; 3] = [Self::Output, Self::Splits, Self::Config];

    const fn name(self) -> &'static str {
        match self {
            Self::Output => "output",
            Self::Splits => "splits",
            Self::Config => "config",
        }
    }

    const fn label(self) -> &'static str {
        match self {
            Self::Output => "Downloads",
            Self::Splits => "Splits",
            Self::Config => "Beets config",
        }
    }

    const fn required(self) -> bool {
        !matches!(self, Self::Config)
    }

    fn get(self, defaults: &GuiDefaults) -> &Path {
        match self {
            Self::Output => &defaults.output,
            Self::Splits => &defaults.splits,
            Self::Config => &defaults.config,
        }
    }

    fn set(self, defaults: &mut GuiDefaults, value: &str) -> Result<(), String> {
        let value = value.trim();
        if value.contains('\0') || (self.required() && value.is_empty()) {
            return Err(format!("{} is not valid.", self.label()));
        }
        let path = PathBuf::from(value);
        match self {
            Self::Output => defaults.output = path,
            Self::Splits => defaults.splits = path,
            Self::Config => defaults.config = path,
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Choice {
    AudioSource,
    MetadataSource,
    Prefer,
    Fallback,
    QualityPolicy,
    Duplicates,
    PlaylistFormat,
}

impl Choice {
    const ALL: [Self; 7] = [
        Self::AudioSource,
        Self::MetadataSource,
        Self::Prefer,
        Self::Fallback,
        Self::QualityPolicy,
        Self::Duplicates,
        Self::PlaylistFormat,
    ];

    const fn name(self) -> &'static str {
        match self {
            Self::AudioSource => "audio_source",
            Self::MetadataSource => "metadata_source",
            Self::Prefer => "prefer",
            Self::Fallback => "fallback",
            Self::QualityPolicy => "quality_policy",
            Self::Duplicates => "duplicates",
            Self::PlaylistFormat => "playlist_format",
        }
    }

    const fn label(self) -> &'static str {
        match self {
            Self::AudioSource => "Audio source",
            Self::MetadataSource => "Metadata",
            Self::Prefer => "Prefer",
            Self::Fallback => "Fallback",
            Self::QualityPolicy => "Quality policy",
            Self::Duplicates => "Album already in library",
            Self::PlaylistFormat => "Playlist format",
        }
    }

    const fn options(self) -> &'static [&'static str] {
        match self {
            Self::AudioSource => AudioSource::CHOICES,
            Self::MetadataSource => MetadataSource::CHOICES,
            Self::Prefer => PreferredAudio::CHOICES,
            Self::Fallback => AudioFallback::CHOICES,
            Self::QualityPolicy => QualityPolicy::CHOICES,
            Self::Duplicates => DuplicatePolicy::CHOICES,
            Self::PlaylistFormat => PlaylistFormat::CHOICES,
        }
    }

    fn get(self, defaults: &GuiDefaults) -> String {
        match self {
            Self::AudioSource => defaults.audio_source.to_string(),
            Self::MetadataSource => defaults.metadata_source.to_string(),
            Self::Prefer => defaults.prefer.to_string(),
            Self::Fallback => defaults.fallback.to_string(),
            Self::QualityPolicy => defaults.quality_policy.to_string(),
            Self::Duplicates => defaults.duplicates.to_string(),
            Self::PlaylistFormat => defaults.playlist_format.to_string(),
        }
    }

    fn set(self, defaults: &mut GuiDefaults, value: &str) -> Result<(), String> {
        let invalid = || format!("{} is not valid.", self.label());
        let value = value.trim();
        match self {
            Self::AudioSource => defaults.audio_source = value.parse().map_err(|_| invalid())?,
            Self::MetadataSource => {
                defaults.metadata_source = value.parse().map_err(|_| invalid())?;
            }
            Self::Prefer => defaults.prefer = value.parse().map_err(|_| invalid())?,
            Self::Fallback => defaults.fallback = value.parse().map_err(|_| invalid())?,
            Self::QualityPolicy => {
                defaults.quality_policy = value.parse().map_err(|_| invalid())?;
            }
            Self::Duplicates => defaults.duplicates = value.parse().map_err(|_| invalid())?,
            Self::PlaylistFormat => {
                defaults.playlist_format = value.parse().map_err(|_| invalid())?;
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Flag {
    Review,
    NoSplit,
    NoOrganize,
    Import,
    TagOnly,
    DryRun,
    KeepSource,
    Force,
    Interactive,
    AutoDecide,
    ReplayGainAfterImport,
    CoversAfterImport,
    Playlists,
}

impl Flag {
    const ALL: [Self; 13] = [
        Self::Review,
        Self::NoSplit,
        Self::NoOrganize,
        Self::Import,
        Self::TagOnly,
        Self::DryRun,
        Self::KeepSource,
        Self::Force,
        Self::Interactive,
        Self::AutoDecide,
        Self::ReplayGainAfterImport,
        Self::CoversAfterImport,
        Self::Playlists,
    ];

    const fn name(self) -> &'static str {
        match self {
            Self::Review => "review",
            Self::NoSplit => "no_split",
            Self::NoOrganize => "no_organize",
            Self::Import => "import",
            Self::TagOnly => "tag_only",
            Self::DryRun => "dry_run",
            Self::KeepSource => "keep_source",
            Self::Force => "force",
            Self::Interactive => "interactive",
            Self::AutoDecide => "auto_decide",
            Self::ReplayGainAfterImport => "replaygain_after_import",
            Self::CoversAfterImport => "covers_after_import",
            Self::Playlists => "playlists",
        }
    }

    const fn label(self) -> &'static str {
        match self {
            Self::Review => "Review chapters",
            Self::NoSplit => "No split",
            Self::NoOrganize => "No organize",
            Self::Import => "Import",
            Self::TagOnly => "Tag only",
            Self::DryRun => "Dry run",
            Self::KeepSource => "Keep source",
            Self::Force => "Force",
            Self::Interactive => "Interactive",
            Self::AutoDecide => "Choose automatically",
            Self::ReplayGainAfterImport => "Calculate ReplayGain",
            Self::CoversAfterImport => "Add cover files",
            Self::Playlists => "Write playlist files",
        }
    }

    const fn get(self, defaults: &GuiDefaults) -> bool {
        match self {
            Self::Review => defaults.review,
            Self::NoSplit => defaults.no_split,
            Self::NoOrganize => defaults.no_organize,
            Self::Import => defaults.import,
            Self::TagOnly => defaults.tag_only,
            Self::DryRun => defaults.dry_run,
            Self::KeepSource => defaults.keep_source,
            Self::Force => defaults.force,
            Self::Interactive => defaults.interactive,
            Self::AutoDecide => defaults.auto_decide,
            Self::ReplayGainAfterImport => defaults.replaygain_after_import,
            Self::CoversAfterImport => defaults.covers_after_import,
            Self::Playlists => defaults.playlists,
        }
    }

    const fn set(self, defaults: &mut GuiDefaults, on: bool) {
        match self {
            Self::Review => defaults.review = on,
            Self::NoSplit => defaults.no_split = on,
            Self::NoOrganize => defaults.no_organize = on,
            Self::Import => defaults.import = on,
            Self::TagOnly => defaults.tag_only = on,
            Self::DryRun => defaults.dry_run = on,
            Self::KeepSource => defaults.keep_source = on,
            Self::Force => defaults.force = on,
            Self::Interactive => defaults.interactive = on,
            Self::AutoDecide => defaults.auto_decide = on,
            Self::ReplayGainAfterImport => defaults.replaygain_after_import = on,
            Self::CoversAfterImport => defaults.covers_after_import = on,
            Self::Playlists => defaults.playlists = on,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Setting {
    Path(PathField),
    Choice(Choice),
    Flag(Flag),
    Jobs,
    MinBitrate,
    AgentModel,
}

impl Setting {
    fn named(name: &str) -> Option<Self> {
        PathField::ALL
            .into_iter()
            .find(|field| field.name() == name)
            .map(Self::Path)
            .or_else(|| {
                Choice::ALL
                    .into_iter()
                    .find(|choice| choice.name() == name)
                    .map(Self::Choice)
            })
            .or_else(|| {
                Flag::ALL
                    .into_iter()
                    .find(|flag| flag.name() == name)
                    .map(Self::Flag)
            })
            .or(match name {
                JOBS => Some(Self::Jobs),
                MIN_BITRATE => Some(Self::MinBitrate),
                AGENT_MODEL => Some(Self::AgentModel),
                _ => None,
            })
    }
}

fn apply(defaults: &mut GuiDefaults, form: &[(String, String)]) -> Result<(), String> {
    let mut checked = HashSet::new();
    for (name, value) in form {
        let setting =
            Setting::named(name).ok_or_else(|| format!("The setting {name} is not known."))?;
        let value = value.trim();
        match setting {
            Setting::Path(field) => field.set(defaults, value)?,
            Setting::Choice(choice) => choice.set(defaults, value)?,
            Setting::Flag(flag) => {
                checked.insert(flag);
            }
            Setting::Jobs => {
                defaults.jobs = value
                    .parse()
                    .map_err(|_| "Jobs must be a whole number.".to_owned())?;
            }
            Setting::MinBitrate => {
                defaults.min_bitrate = value
                    .parse()
                    .map_err(|_| "Min bitrate must be a whole number.".to_owned())?;
            }
            Setting::AgentModel => {
                if !value.is_empty() {
                    value.clone_into(&mut defaults.agent_model);
                }
            }
        }
    }
    for flag in Flag::ALL {
        flag.set(defaults, checked.contains(&flag));
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Control {
    Text(String),
    Number(String),
    Choice { value: String, options: Vec<String> },
    Flag(bool),
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Field {
    name: &'static str,
    label: &'static str,
    control: Control,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Group {
    title: &'static str,
    note: &'static str,
    fields: Vec<Field>,
}

fn path_field(defaults: &GuiDefaults, field: PathField) -> Field {
    Field {
        name: field.name(),
        label: field.label(),
        control: Control::Text(field.get(defaults).display().to_string()),
    }
}

fn choice_field(defaults: &GuiDefaults, choice: Choice) -> Field {
    let value = choice.get(defaults);
    let mut options: Vec<String> = choice
        .options()
        .iter()
        .map(|option| (*option).to_owned())
        .collect();
    if !options.contains(&value) {
        options.push(value.clone());
    }
    Field {
        name: choice.name(),
        label: choice.label(),
        control: Control::Choice { value, options },
    }
}

const fn flag_field(defaults: &GuiDefaults, flag: Flag) -> Field {
    Field {
        name: flag.name(),
        label: flag.label(),
        control: Control::Flag(flag.get(defaults)),
    }
}

fn groups(defaults: &GuiDefaults) -> Vec<Group> {
    let paths = PathField::ALL
        .into_iter()
        .map(|field| path_field(defaults, field))
        .collect();
    let sources = SOURCE_CHOICES
        .into_iter()
        .map(|choice| choice_field(defaults, choice))
        .collect();
    let mut processing = vec![
        Field {
            name: JOBS,
            label: "Jobs",
            control: Control::Number(defaults.jobs.to_string()),
        },
        Field {
            name: MIN_BITRATE,
            label: "Min bitrate",
            control: Control::Number(defaults.min_bitrate.to_string()),
        },
    ];
    processing.extend(
        PROCESSING_FLAGS
            .into_iter()
            .map(|flag| flag_field(defaults, flag)),
    );
    let agent = vec![
        Field {
            name: AGENT_MODEL,
            label: "Model",
            control: Control::Text(defaults.agent_model.clone()),
        },
        flag_field(defaults, Flag::AutoDecide),
    ];
    let after_import = IMPORT_FLAGS
        .into_iter()
        .map(|flag| flag_field(defaults, flag))
        .collect();
    let playlists = vec![
        flag_field(defaults, Flag::Playlists),
        choice_field(defaults, Choice::PlaylistFormat),
    ];
    vec![
        Group {
            title: "Destinations",
            note: "",
            fields: paths,
        },
        Group {
            title: "Sources and quality",
            note: "",
            fields: sources,
        },
        Group {
            title: "Processing",
            note: "Workflow uses these settings for each run.",
            fields: processing,
        },
        Group {
            title: "AI decisions",
            note: "Muzik asks this Codex model to pick album matches and Soulseek downloads. It asks you when the model is not sure.",
            fields: agent,
        },
        Group {
            title: "After each import",
            note: "Muzik does these tasks for each album that it imports.",
            fields: after_import,
        },
        Group {
            title: "Playlists",
            note: "Muzik writes a playlist file for each watchlist source to the Playlists folder of the library and updates it after each watchlist job. Each source on the Watchlist page has its own switch. Most players read m3u8. Use m3u for a player or device that does not accept m3u8.",
            fields: playlists,
        },
    ]
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Soulseek {
    username: String,
    host: String,
    port: String,
    has_password: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Bandcamp {
    user: String,
    logged_in: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Spotify {
    client_id: String,
    redirect_uri: String,
    connected: bool,
    account: String,
    error: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ServiceRow {
    name: String,
    detail: String,
    state: ServiceState,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ServiceState {
    Available,
    Optional,
    Unavailable,
    NotSet,
}

impl ServiceState {
    const fn of(service: &ServiceStatus) -> Self {
        match service.available {
            Some(true) => Self::Available,
            Some(false) if service.optional => Self::Optional,
            Some(false) => Self::Unavailable,
            None => Self::NotSet,
        }
    }

    const fn word(self) -> &'static str {
        match self {
            Self::Available => "Available",
            Self::Optional => "Optional",
            Self::Unavailable => "Unavailable",
            Self::NotSet => "Not set up",
        }
    }

    const fn tone(self) -> &'static str {
        match self {
            Self::Available => "tag ok",
            Self::Optional => "tag warn",
            Self::Unavailable => "tag bad",
            Self::NotSet => "tag mute",
        }
    }
}

fn soulseek_of(profile: &SoulseekProfile) -> Soulseek {
    Soulseek {
        username: profile.username.clone(),
        host: profile.server_host.clone(),
        port: profile.server_port.to_string(),
        has_password: profile.has_password,
    }
}

fn spotify_of(status: &SpotifyStatus) -> Spotify {
    Spotify {
        client_id: status.client_id.clone(),
        redirect_uri: status.redirect_uri.clone(),
        connected: status.connected,
        account: status
            .account_name
            .clone()
            .unwrap_or_else(|| "Spotify account".to_owned()),
        error: status.error.clone().unwrap_or_default(),
    }
}

fn service_rows(checked: &[ServiceStatus]) -> Vec<ServiceRow> {
    checked
        .iter()
        .map(|service| ServiceRow {
            name: truncate(service.name, 60),
            detail: truncate(&service.detail, 200),
            state: ServiceState::of(service),
        })
        .collect()
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Screen {
    groups: Vec<Group>,
    soulseek: Option<Soulseek>,
    bandcamp: Bandcamp,
    spotify: Option<Spotify>,
    services: Option<Vec<ServiceRow>>,
}

#[query_params(error = bad_request)]
struct SettingsQuery {
    notice: Option<String>,
    error: Option<String>,
}

fn back(cx: &Cx, outcome: Result<String, String>) -> SeeOther {
    see_other(href!(page).query(&Notice::from(outcome)).resolve(cx))
}

async fn screen(cx: &Cx) -> Result<Screen> {
    let (defaults, soulseek, bandcamp, spotify) = backend(cx)
        .run(|service| {
            (
                service.defaults(),
                service.soulseek_account(),
                service.bandcamp(),
                service.spotify_status(),
            )
        })
        .await?;
    Ok(Screen {
        groups: groups(&defaults),
        soulseek: soulseek.ok().map(|account| soulseek_of(&account)),
        bandcamp: Bandcamp {
            user: bandcamp.user,
            logged_in: bandcamp.logged_in,
        },
        spotify: spotify.ok().map(|status| spotify_of(&status)),
        services: hub(cx).services().map(|checked| service_rows(&checked)),
    })
}

#[page("/settings")]
pub async fn page(cx: &Cx) -> Result<impl View> {
    auth::require(cx).await?;
    let query = query_params::<SettingsQuery>(cx)?;
    let message = query.error.clone().or_else(|| query.notice.clone());
    let failed = query.error.is_some();
    let data = screen(cx).await?;
    Ok(view! {
        <h1>"Settings"</h1>
        <p class=(if failed { "notice error" } else { "notice" })>(message)</p>
        defaults_form(groups: data.groups)
        soulseek_card(account: data.soulseek)
        bandcamp_card(account: data.bandcamp)
        spotify_card(account: data.spotify)
        services_card(services: data.services)
        library_card()
    })
}

#[component]
async fn defaults_form(cx: &Cx, groups: Vec<Group>) -> Result<impl View> {
    let action = href!(save_defaults).resolve(cx);
    Ok(view! {
        <form method="post" action=(action)>
            for group in groups {
                group_card(group: group)
            }
            <div class="savebar">
                <button class="primary" type="submit">"Save config"</button>
            </div>
        </form>
    })
}

#[component]
async fn group_card(cx: &Cx, group: Group) -> Result<impl View> {
    let has_note = !group.note.is_empty();
    Ok(view! {
        <section class="card setting">
            <h2>(group.title)</h2>
            if has_note {
                <p class="meta note">(group.note)</p>
            }
            for field in group.fields {
                field_row(field: field)
            }
        </section>
    })
}

#[component]
async fn field_row(cx: &Cx, field: Field) -> Result<impl View> {
    let id = format!("f-{}", field.name);
    Ok(view! {
        <div class="frow">
            <label for=(id.clone())>(field.label)</label>
            match field.control {
                Control::Text(value) => {
                    <input type="text" id=(id) name=(field.name) value=(value) autocomplete="off">
                }
                Control::Number(value) => {
                    <input type="number" min="0" id=(id) name=(field.name) value=(value)>
                }
                Control::Choice { value, options } => {
                    <select id=(id) name=(field.name)>
                        for option in options {
                            <option value=(option.clone()) selected=(option == value)>(option.clone())</option>
                        }
                    </select>
                }
                Control::Flag(on) => {
                    <input type="checkbox" id=(id) name=(field.name) checked=(on)>
                }
            }
        </div>
    })
}

#[component]
async fn soulseek_card(cx: &Cx, account: Option<Soulseek>) -> Result<impl View> {
    let action = href!(save_soulseek).resolve(cx);
    let account = account.unwrap_or(Soulseek {
        username: String::new(),
        host: String::new(),
        port: String::new(),
        has_password: false,
    });
    let placeholder = if account.has_password {
        "Saved. Type a new one to change it."
    } else {
        "Password"
    };
    Ok(view! {
        <form method="post" action=(action)>
            <section class="card setting">
                <h2>"Soulseek"</h2>
                <p class="meta note">"Your Soulseek account. Workflow uses it when Audio source is soulseek or as a fallback."</p>
                <div class="frow">
                    <label for="soulseek-username">"Username"</label>
                    <input type="text" id="soulseek-username" name="username" value=(account.username) autocomplete="off">
                </div>
                <div class="frow">
                    <label for="soulseek-password">"Password"</label>
                    <input
                        type="password"
                        id="soulseek-password"
                        name="password"
                        placeholder=(placeholder)
                        autocomplete="new-password"
                    >
                </div>
                <div class="frow">
                    <label for="soulseek-host">"Server"</label>
                    <input type="text" id="soulseek-host" name="server_host" value=(account.host) autocomplete="off">
                </div>
                <div class="frow">
                    <label for="soulseek-port">"Port"</label>
                    <input type="number" min="1" max="65535" id="soulseek-port" name="server_port" value=(account.port)>
                </div>
                <div class="frow actions">
                    <button type="submit">"Save Soulseek account"</button>
                </div>
            </section>
        </form>
    })
}

#[component]
async fn bandcamp_card(cx: &Cx, account: Bandcamp) -> Result<impl View> {
    let save = href!(save_bandcamp).resolve(cx);
    let logout = href!(logout_bandcamp).resolve(cx);
    let placeholder = if account.logged_in {
        "Saved. Paste new cookies to change them."
    } else {
        "Paste the identity cookie value"
    };
    Ok(view! {
        <section class="card setting">
            <h2>"Bandcamp"</h2>
            <p class="meta note">(BANDCAMP_HELP)</p>
            <form method="post" action=(save)>
                <div class="frow">
                    <label for="bandcamp-user">"User name"</label>
                    <input
                        type="text"
                        id="bandcamp-user"
                        name="user"
                        value=(account.user)
                        placeholder="Found automatically"
                        autocomplete="off"
                    >
                </div>
                <div class="frow tall">
                    <label for="bandcamp-cookies">"Login cookie"</label>
                    <textarea id="bandcamp-cookies" name="cookies" placeholder=(placeholder) autocomplete="off"></textarea>
                </div>
                <div class="frow actions">
                    <button type="submit">"Save Bandcamp login"</button>
                    <span class="meta">(if account.logged_in { "Logged in" } else { "" })</span>
                </div>
            </form>
            if account.logged_in {
                <form class="frow actions" method="post" action=(logout)>
                    <button type="submit">"Log out"</button>
                </form>
            }
        </section>
    })
}

#[component]
async fn spotify_card(cx: &Cx, account: Option<Spotify>) -> Result<impl View> {
    let save = href!(save_spotify_client).resolve(cx);
    let logout = href!(logout_spotify).resolve(cx);
    let known = account.is_some();
    let account = account.unwrap_or(Spotify {
        client_id: String::new(),
        redirect_uri: String::new(),
        connected: false,
        account: String::new(),
        error: String::new(),
    });
    let has_client = !account.client_id.trim().is_empty();
    Ok(view! {
        <section class="card setting">
            <h2>"Spotify"</h2>
            <p class="meta note">"Your Spotify account. Muzik reads your playlists and Liked Songs with it. Create an application in Spotify, add this redirect URI, then save its client ID here."</p>
            if !known {
                <p class="notice error">"The Spotify settings cannot be read."</p>
            }
            <div class="frow">
                <label>"Redirect URI"</label>
                <code>(account.redirect_uri)</code>
            </div>
            <form method="post" action=(save)>
                <div class="frow">
                    <label for="spotify-client">"Client ID"</label>
                    <input type="text" id="spotify-client" name="client_id" value=(account.client_id) autocomplete="off">
                </div>
                <div class="frow actions">
                    <button type="submit">"Save client ID"</button>
                </div>
            </form>
            <p class="notice error">(account.error)</p>
            if account.connected {
                <form class="frow actions" method="post" action=(logout)>
                    <span>"Connected account: " (account.account)</span>
                    <button type="submit">"Disconnect"</button>
                </form>
            } else if has_client {
                <div class="frow note-row">
                    <span class="meta">"To connect, run muzik spotify login on the machine that runs muzik-server. The login opens a browser and listens for the redirect on that machine, so a browser on another machine cannot finish it from this page."</span>
                </div>
            } else {
                <div class="frow note-row">
                    <span class="meta">"Save a client ID to connect your account."</span>
                </div>
            }
        </section>
    })
}

#[component]
async fn services_card(cx: &Cx, services: Option<Vec<ServiceRow>>) -> Result<impl View> {
    let action = href!(check_services).resolve(cx);
    let missing = services.as_ref().map_or(0, |rows| {
        rows.iter()
            .filter(|row| row.state == ServiceState::Unavailable)
            .count()
    });
    let rows = services.clone().unwrap_or_default();
    let checked = services.is_some();
    Ok(view! {
        <section class="card setting">
            <h2>"Services"</h2>
            <p class="meta note">"The tools muzik can use. Checking connects to Soulseek when an account is saved."</p>
            if missing > 0 {
                <p class="notice error">(missing) " required service(s) unavailable. Jobs that need them will fail."</p>
            }
            if !checked {
                <div class="frow note-row"><span class="meta">"No service checks yet."</span></div>
            }
            for row in rows {
                <div class="frow service">
                    <span class="title">(row.name)</span>
                    <span class="meta">(row.detail)</span>
                    <span class=(row.state.tone())>(row.state.word())</span>
                </div>
            }
            <form class="frow actions" method="post" action=(action)>
                <button type="submit">"Check services"</button>
            </form>
        </section>
    })
}

#[component]
async fn library_card(cx: &Cx) -> Result<impl View> {
    let action = href!(start_task).resolve(cx);
    Ok(view! {
        <section class="card setting">
            <h2>"Library maintenance"</h2>
            <p class="meta note">"Each task runs over the full library. Preview shows in Activity what the task would change and changes nothing."</p>
            for task in LibraryTask::ALL {
                <form class="frow" method="post" action=(action.clone())>
                    <span>(task.title())</span>
                    <input type="hidden" name="task" value=(task.as_str())>
                    <button type="submit" name="dry_run" value="true">"Preview"</button>
                    <button type="submit">"Start"</button>
                </form>
            }
        </section>
    })
}

#[route(POST "/settings/defaults")]
pub async fn save_defaults(cx: &Cx, Form(form): Form<Vec<(String, String)>>) -> Result<SeeOther> {
    auth::require(cx).await?;
    let outcome = backend(cx)
        .run(move |service| {
            let mut defaults = service.defaults();
            apply(&mut defaults, &form)?;
            done(service.save_defaults(&defaults), "Config saved")
        })
        .await?;
    Ok(back(cx, outcome))
}

#[derive(Deserialize)]
struct SoulseekFields {
    #[serde(default)]
    username: String,
    #[serde(default)]
    password: String,
    #[serde(default)]
    server_host: String,
    #[serde(default)]
    server_port: String,
}

#[route(POST "/settings/soulseek")]
pub async fn save_soulseek(cx: &Cx, Form(form): Form<SoulseekFields>) -> Result<SeeOther> {
    auth::require(cx).await?;
    let outcome = match SoulseekForm::from_fields(
        &form.username,
        &form.password,
        &form.server_host,
        &form.server_port,
    ) {
        Ok(form) => {
            backend(cx)
                .run(move |service| service.save_soulseek(&form))
                .await?
        }
        Err(error) => Err(error),
    };
    Ok(back(cx, done(outcome, "Soulseek account saved")))
}

#[derive(Deserialize)]
struct BandcampFields {
    #[serde(default)]
    user: String,
    #[serde(default)]
    cookies: String,
}

#[route(POST "/settings/bandcamp")]
pub async fn save_bandcamp(cx: &Cx, Form(form): Form<BandcampFields>) -> Result<SeeOther> {
    auth::require(cx).await?;
    let outcome = backend(cx)
        .run(move |service| service.save_bandcamp(form.user.trim(), &form.cookies))
        .await?;
    if outcome.is_ok() {
        crate::watchlist::reload(cx).await?;
    }
    Ok(back(cx, done(outcome, "Bandcamp login saved")))
}

#[route(POST "/settings/bandcamp/logout")]
pub async fn logout_bandcamp(cx: &Cx) -> Result<SeeOther> {
    auth::require(cx).await?;
    let outcome = backend(cx).run(Service::logout_bandcamp).await?;
    Ok(back(cx, done(outcome, "Bandcamp login removed")))
}

#[derive(Deserialize)]
struct SpotifyFields {
    #[serde(default)]
    client_id: String,
}

#[route(POST "/settings/spotify")]
pub async fn save_spotify_client(cx: &Cx, Form(form): Form<SpotifyFields>) -> Result<SeeOther> {
    auth::require(cx).await?;
    let outcome = backend(cx)
        .run(move |service| service.set_spotify_client_id(&form.client_id))
        .await?;
    Ok(back(cx, done(outcome, "Spotify client ID saved")))
}

#[route(POST "/settings/spotify/logout")]
pub async fn logout_spotify(cx: &Cx) -> Result<SeeOther> {
    auth::require(cx).await?;
    let outcome = backend(cx).run(Service::spotify_logout).await?;
    Ok(back(cx, done(outcome, "Spotify disconnected")))
}

#[route(POST "/settings/services")]
pub async fn check_services(cx: &Cx) -> Result<SeeOther> {
    auth::require(cx).await?;
    let checked = backend(cx).run(Service::services).await?;
    hub(cx).store_services(checked);
    Ok(back(cx, Ok("Services checked".into())))
}

#[derive(Deserialize)]
struct TaskFields {
    task: String,
    #[serde(default)]
    dry_run: bool,
}

#[route(POST "/settings/library-task")]
pub async fn start_task(cx: &Cx, Form(form): Form<TaskFields>) -> Result<SeeOther> {
    auth::require(cx).await?;
    let Some(task) = LibraryTask::parse(&form.task) else {
        return Ok(back(cx, Err("The task is not valid.".into())));
    };
    let outcome = backend(cx)
        .run(move |service| service.start_library_task(task, form.dry_run))
        .await?;
    Ok(back(cx, describe(outcome, |id| format!("Queued {id}"))))
}

#[cfg(test)]
mod tests {
    use super::apply;
    use muzik_core::app_config::GuiDefaults;

    fn form(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect()
    }

    #[test]
    fn a_form_changes_the_named_fields_and_clears_the_flags_it_leaves_out() {
        let mut defaults = GuiDefaults::default();
        assert!(defaults.interactive);
        apply(
            &mut defaults,
            &form(&[
                ("jobs", " 3 "),
                ("min_bitrate", "192"),
                ("audio_source", "soulseek"),
                ("review", "on"),
                ("agent_model", "other-model"),
            ]),
        )
        .unwrap();
        assert_eq!(defaults.jobs, 3);
        assert_eq!(defaults.min_bitrate, 192);
        assert_eq!(defaults.audio_source.to_string(), "soulseek");
        assert!(defaults.review);
        assert!(!defaults.interactive);
        assert_eq!(defaults.agent_model, "other-model");
    }

    #[test]
    fn a_value_that_is_not_valid_names_its_field() {
        let mut defaults = GuiDefaults::default();
        assert_eq!(
            apply(&mut defaults, &form(&[("fallback", "nonsense")])),
            Err("Fallback is not valid.".to_owned())
        );
        assert_eq!(
            apply(&mut defaults, &form(&[("jobs", "many")])),
            Err("Jobs must be a whole number.".to_owned())
        );
    }

    #[test]
    fn a_field_that_is_not_known_is_refused() {
        let mut defaults = GuiDefaults::default();
        assert_eq!(
            apply(&mut defaults, &form(&[("audiosource", "soulseek")])),
            Err("The setting audiosource is not known.".to_owned())
        );
    }

    #[test]
    fn a_destination_must_be_a_real_path_but_the_beets_config_may_be_unset() {
        let mut defaults = GuiDefaults::default();
        for form_pairs in [
            form(&[("output", "  ")]),
            form(&[("splits", "")]),
            form(&[("output", "a\0b")]),
            form(&[("config", "a\0b")]),
        ] {
            assert!(apply(&mut defaults, &form_pairs).is_err());
        }
        assert_eq!(defaults, GuiDefaults::default());
        apply(&mut defaults, &form(&[("config", " ")])).unwrap();
        assert!(defaults.config.as_os_str().is_empty());
    }
}
