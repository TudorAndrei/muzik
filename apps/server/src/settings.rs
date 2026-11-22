use std::{collections::HashMap, path::PathBuf};

use muzik_core::{
    AudioFallback, AudioSource, DuplicatePolicy, MetadataSource, PlaylistFormat, PreferredAudio,
    QualityPolicy, app_config::GuiDefaults,
};
use muzik_runner::{
    library_tasks::LibraryTask,
    service::{Service, SoulseekForm, SpotifyStatus},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
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
    state::{backend, hub},
};

const NOTICE_LIMIT: usize = 200;

const PATHS: [(&str, &str); 3] = [
    ("output", "Downloads"),
    ("splits", "Splits"),
    ("config", "Beets config"),
];

const CHOICES: [(&str, &str); 7] = [
    ("audio_source", "Audio source"),
    ("metadata_source", "Metadata"),
    ("prefer", "Prefer"),
    ("fallback", "Fallback"),
    ("quality_policy", "Quality policy"),
    ("duplicates", "Album already in library"),
    ("playlist_format", "Playlist format"),
];

const PROCESSING_FLAGS: [(&str, &str); 9] = [
    ("review", "Review chapters"),
    ("no_split", "No split"),
    ("no_organize", "No organize"),
    ("import", "Import"),
    ("tag_only", "Tag only"),
    ("dry_run", "Dry run"),
    ("keep_source", "Keep source"),
    ("force", "Force"),
    ("interactive", "Interactive"),
];

const OTHER_FLAGS: [(&str, &str); 4] = [
    ("auto_decide", "Choose automatically"),
    ("replaygain_after_import", "Calculate ReplayGain"),
    ("covers_after_import", "Add cover files"),
    ("playlists", "Write playlist files"),
];

const BANDCAMP_HELP: &str = "Muzik uses your Bandcamp login to read your collection and download your purchases in FLAC. To get the login cookie: log in to bandcamp.com in your browser, open the developer tools, open Storage (Firefox, Zen) or Application (Chrome), select Cookies, then https://bandcamp.com, and copy the Value of the identity row. Paste it below and select Save Bandcamp login. Muzik finds your user name. A full Cookie header or a cookies.txt file also works. The cookie stays on the server. Do not share it.";

const fn choice_options(name: &str) -> &'static [&'static str] {
    match name.as_bytes() {
        b"audio_source" => AudioSource::CHOICES,
        b"metadata_source" => MetadataSource::CHOICES,
        b"prefer" => PreferredAudio::CHOICES,
        b"fallback" => AudioFallback::CHOICES,
        b"quality_policy" => QualityPolicy::CHOICES,
        b"duplicates" => DuplicatePolicy::CHOICES,
        b"playlist_format" => PlaylistFormat::CHOICES,
        _ => &[],
    }
}

fn choice_current(defaults: &GuiDefaults, name: &str) -> String {
    match name {
        "audio_source" => defaults.audio_source.to_string(),
        "metadata_source" => defaults.metadata_source.to_string(),
        "prefer" => defaults.prefer.to_string(),
        "fallback" => defaults.fallback.to_string(),
        "quality_policy" => defaults.quality_policy.to_string(),
        "duplicates" => defaults.duplicates.to_string(),
        "playlist_format" => defaults.playlist_format.to_string(),
        _ => String::new(),
    }
}

fn set_choice(defaults: &mut GuiDefaults, name: &str, value: &str) -> bool {
    match name {
        "audio_source" => value
            .parse()
            .map(|value| defaults.audio_source = value)
            .is_ok(),
        "metadata_source" => value
            .parse()
            .map(|value| defaults.metadata_source = value)
            .is_ok(),
        "prefer" => value.parse().map(|value| defaults.prefer = value).is_ok(),
        "fallback" => value.parse().map(|value| defaults.fallback = value).is_ok(),
        "quality_policy" => value
            .parse()
            .map(|value| defaults.quality_policy = value)
            .is_ok(),
        "duplicates" => value
            .parse()
            .map(|value| defaults.duplicates = value)
            .is_ok(),
        "playlist_format" => value
            .parse()
            .map(|value| defaults.playlist_format = value)
            .is_ok(),
        _ => false,
    }
}

fn flag<'a>(defaults: &'a mut GuiDefaults, name: &str) -> Option<&'a mut bool> {
    Some(match name {
        "review" => &mut defaults.review,
        "no_split" => &mut defaults.no_split,
        "no_organize" => &mut defaults.no_organize,
        "import" => &mut defaults.import,
        "tag_only" => &mut defaults.tag_only,
        "dry_run" => &mut defaults.dry_run,
        "keep_source" => &mut defaults.keep_source,
        "force" => &mut defaults.force,
        "interactive" => &mut defaults.interactive,
        "auto_decide" => &mut defaults.auto_decide,
        "replaygain_after_import" => &mut defaults.replaygain_after_import,
        "covers_after_import" => &mut defaults.covers_after_import,
        "playlists" => &mut defaults.playlists,
        _ => return None,
    })
}

fn apply(defaults: &mut GuiDefaults, form: &HashMap<String, String>) -> Result<(), String> {
    for (name, label) in PATHS {
        let Some(value) = form.get(name) else {
            continue;
        };
        let path = PathBuf::from(value.trim());
        match name {
            "output" => defaults.output = path,
            "splits" => defaults.splits = path,
            _ => defaults.config = path,
        }
        if value.contains('\0') {
            return Err(format!("{label} is not valid."));
        }
    }
    for (name, label) in CHOICES {
        if let Some(value) = form.get(name)
            && !set_choice(defaults, name, value.trim())
        {
            return Err(format!("{label} is not valid."));
        }
    }
    if let Some(value) = form.get("jobs") {
        defaults.jobs = value
            .trim()
            .parse()
            .map_err(|_| "Jobs must be a whole number.".to_owned())?;
    }
    if let Some(value) = form.get("min_bitrate") {
        defaults.min_bitrate = value
            .trim()
            .parse()
            .map_err(|_| "Min bitrate must be a whole number.".to_owned())?;
    }
    if let Some(value) = form.get("agent_model") {
        let model = value.trim();
        if !model.is_empty() {
            model.clone_into(&mut defaults.agent_model);
        }
    }
    for (name, _) in PROCESSING_FLAGS.into_iter().chain(OTHER_FLAGS) {
        if let Some(field) = flag(defaults, name) {
            *field = form.contains_key(name);
        }
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

fn choice_field(defaults: &GuiDefaults, name: &'static str, label: &'static str) -> Field {
    let value = choice_current(defaults, name);
    let mut options: Vec<String> = choice_options(name)
        .iter()
        .map(|option| (*option).to_owned())
        .collect();
    if !options.contains(&value) {
        options.push(value.clone());
    }
    Field {
        name,
        label,
        control: Control::Choice { value, options },
    }
}

fn flag_fields(defaults: &GuiDefaults, names: &[(&'static str, &'static str)]) -> Vec<Field> {
    let mut copy = defaults.clone();
    names
        .iter()
        .filter_map(|(name, label)| {
            Some(Field {
                name,
                label,
                control: Control::Flag(*flag(&mut copy, name)?),
            })
        })
        .collect()
}

fn groups(defaults: &GuiDefaults) -> Vec<Group> {
    let paths = [&defaults.output, &defaults.splits, &defaults.config]
        .into_iter()
        .zip(PATHS)
        .map(|(value, (name, label))| Field {
            name,
            label,
            control: Control::Text(value.display().to_string()),
        })
        .collect();
    let sources = CHOICES
        .iter()
        .take(6)
        .map(|(name, label)| choice_field(defaults, name, label))
        .collect();
    let mut processing = vec![
        Field {
            name: "jobs",
            label: "Jobs",
            control: Control::Number(defaults.jobs.to_string()),
        },
        Field {
            name: "min_bitrate",
            label: "Min bitrate",
            control: Control::Number(defaults.min_bitrate.to_string()),
        },
    ];
    processing.extend(flag_fields(defaults, &PROCESSING_FLAGS));
    let mut agent = vec![Field {
        name: "agent_model",
        label: "Model",
        control: Control::Text(defaults.agent_model.clone()),
    }];
    agent.extend(flag_fields(defaults, &OTHER_FLAGS[..1]));
    let mut playlists = flag_fields(defaults, &OTHER_FLAGS[3..]);
    playlists.push(choice_field(defaults, "playlist_format", "Playlist format"));
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
            fields: flag_fields(defaults, &OTHER_FLAGS[1..3]),
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
    word: &'static str,
    tone: &'static str,
}

fn text_of(value: &Value, key: &str) -> String {
    match value.get(key) {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Number(number)) => number.to_string(),
        _ => String::new(),
    }
}

fn soulseek_of(value: &Value) -> Soulseek {
    Soulseek {
        username: text_of(value, "username"),
        host: text_of(value, "server_host"),
        port: text_of(value, "server_port"),
        has_password: value.get("has_password") == Some(&Value::Bool(true)),
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

fn service_rows(checked: &Value) -> Vec<ServiceRow> {
    checked
        .get("services")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|service| {
            let (word, tone) = match service.get("available").and_then(Value::as_bool) {
                Some(true) => ("Available", "tag ok"),
                Some(false) if service.get("optional") == Some(&Value::Bool(true)) => {
                    ("Optional", "tag warn")
                }
                Some(false) => ("Unavailable", "tag bad"),
                None => ("Not set up", "tag mute"),
            };
            ServiceRow {
                name: truncate(&text_of(service, "name"), 60),
                detail: truncate(&text_of(service, "detail"), 200),
                word,
                tone,
            }
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

#[derive(Serialize)]
struct Outcome {
    #[serde(skip_serializing_if = "Option::is_none")]
    notice: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

fn back(cx: &Cx, outcome: Result<String, String>) -> SeeOther {
    let outcome = match outcome {
        Ok(text) => Outcome {
            notice: Some(truncate(&text, NOTICE_LIMIT)),
            error: None,
        },
        Err(text) => Outcome {
            notice: None,
            error: Some(truncate(&text, NOTICE_LIMIT)),
        },
    };
    see_other(href!(page).query(&outcome).resolve(cx))
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
            user: text_of(&bandcamp, "user"),
            logged_in: bandcamp.get("logged_in") == Some(&Value::Bool(true)),
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
        rows.iter().filter(|row| row.word == "Unavailable").count()
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
                    <span class=(row.tone)>(row.word)</span>
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
            <p class="meta note">"Each task runs over the full library. Activity shows its progress."</p>
            for task in LibraryTask::ALL {
                <form class="frow" method="post" action=(action.clone())>
                    <span>(task.title())</span>
                    <input type="hidden" name="task" value=(task.as_str())>
                    <button type="submit">"Start"</button>
                </form>
            }
        </section>
    })
}

#[route(POST "/settings/defaults")]
pub async fn save_defaults(cx: &Cx, Form(form): Form<HashMap<String, String>>) -> Result<SeeOther> {
    auth::require(cx).await?;
    let outcome = backend(cx)
        .run(move |service| {
            let mut defaults = service.defaults();
            apply(&mut defaults, &form)?;
            service
                .save_defaults(&defaults)
                .map(|_| "Config saved".to_owned())
                .map_err(|error| error.to_string())
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
    let outcome = backend(cx)
        .run(move |service| {
            service.save_soulseek(&SoulseekForm {
                username: form.username,
                password: form.password,
                server_host: form.server_host,
                server_port: form.server_port,
            })
        })
        .await?;
    Ok(back(
        cx,
        outcome
            .map(|_| "Soulseek account saved".to_owned())
            .map_err(|error| error.to_string()),
    ))
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
    Ok(back(
        cx,
        outcome
            .map(|_| "Bandcamp login saved".to_owned())
            .map_err(|error| error.to_string()),
    ))
}

#[route(POST "/settings/bandcamp/logout")]
pub async fn logout_bandcamp(cx: &Cx) -> Result<SeeOther> {
    auth::require(cx).await?;
    let outcome = backend(cx).run(Service::logout_bandcamp).await?;
    Ok(back(
        cx,
        outcome
            .map(|_| "Bandcamp login removed".to_owned())
            .map_err(|error| error.to_string()),
    ))
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
    Ok(back(
        cx,
        outcome
            .map(|_| "Spotify client ID saved".to_owned())
            .map_err(|error| error.to_string()),
    ))
}

#[route(POST "/settings/spotify/logout")]
pub async fn logout_spotify(cx: &Cx) -> Result<SeeOther> {
    auth::require(cx).await?;
    let outcome = backend(cx).run(Service::spotify_logout).await?;
    Ok(back(
        cx,
        outcome
            .map(|_| "Spotify disconnected".to_owned())
            .map_err(|error| error.to_string()),
    ))
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
}

#[route(POST "/settings/library-task")]
pub async fn start_task(cx: &Cx, Form(form): Form<TaskFields>) -> Result<SeeOther> {
    auth::require(cx).await?;
    let Some(task) = LibraryTask::parse(&form.task) else {
        return Ok(back(cx, Err("The task is not valid.".into())));
    };
    let outcome = backend(cx)
        .run(move |service| service.start_library_task(task))
        .await?;
    Ok(back(
        cx,
        outcome
            .map(|id| format!("Queued {id}"))
            .map_err(|error| error.to_string()),
    ))
}

#[cfg(test)]
mod tests {
    use super::apply;
    use muzik_core::app_config::GuiDefaults;
    use std::collections::HashMap;

    fn form(pairs: &[(&str, &str)]) -> HashMap<String, String> {
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
}
