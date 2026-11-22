#![expect(
    clippy::unwrap_used,
    reason = "the helpers of this test file panic when the setup fails"
)]

use futures_util::StreamExt;
use muzik_core::{DecisionKind, paths::Paths};
use muzik_runner::AppEvent;
use muzik_server::{Bus, Config, build};
use serde_json::Value;
use tempfile::TempDir;
use topcoat::router::{
    Body, Router, StatusCode, header, request::Request, response::Response, to_bytes,
};

const TOKEN: &str = "a-long-enough-secret";

struct App {
    router: Router,
    bus: Bus,
    home: TempDir,
}

fn app(token: Option<&str>) -> App {
    let home = tempfile::tempdir().unwrap();
    let paths = Paths::under(home.path());
    for dir in [&paths.data, &paths.config, &paths.cache] {
        std::fs::create_dir_all(dir).unwrap();
    }
    let server = build(
        paths,
        Config {
            token: token.map(str::to_owned),
            secure_cookie: false,
        },
    )
    .unwrap();
    App {
        router: server.router,
        bus: server.bus,
        home,
    }
}

fn get(path: &str, cookie: Option<&str>) -> Request {
    let mut request = Request::builder().method("GET").uri(path);
    if let Some(cookie) = cookie {
        request = request.header(header::COOKIE, cookie);
    }
    request.body(Body::empty()).unwrap()
}

fn post(path: &str, form: &str, cookie: Option<&str>) -> Request {
    let mut request = Request::builder()
        .method("POST")
        .uri(path)
        .header(header::HOST, "localhost")
        .header(header::ORIGIN, "http://localhost")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
    if let Some(cookie) = cookie {
        request = request.header(header::COOKIE, cookie);
    }
    request.body(Body::from(form.to_owned())).unwrap()
}

fn location(response: &Response) -> &str {
    response
        .headers()
        .get(header::LOCATION)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
}

fn session_cookie(response: &Response) -> Option<String> {
    response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .find(|value| value.starts_with("muzik-session=") && !value.contains("Max-Age=0"))
        .and_then(|value| value.split(';').next())
        .map(str::to_owned)
}

async fn text(response: Response) -> String {
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    String::from_utf8(bytes.to_vec()).unwrap()
}

async fn sign_in(app: &App) -> String {
    let response = app
        .router
        .handle(post("/login", &format!("token={TOKEN}"), None))
        .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&response), "/");
    session_cookie(&response).unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_request_without_a_session_goes_to_the_login_page() {
    let app = app(Some(TOKEN));
    for path in ["/", "/watchlist", "/library", "/settings"] {
        let response = app.router.handle(get(path, None)).await;
        assert!(response.status().is_redirection(), "{path}");
        assert_eq!(location(&response), "/login", "{path}");
    }
    let response = app.router.handle(get("/login", None)).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(text(response).await.contains("name=\"token\""));
}

#[tokio::test(flavor = "multi_thread")]
async fn the_api_answers_unauthorized_without_a_session() {
    let app = app(Some(TOKEN));
    for path in ["/api/jobs", "/api/events"] {
        let response = app.router.handle(get(path, None)).await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{path}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_wrong_token_does_not_start_a_session() {
    let app = app(Some(TOKEN));
    let response = app
        .router
        .handle(post("/login", "token=a-long-enough-secreT", None))
        .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&response), "/login?failed=1");
    assert!(session_cookie(&response).is_none());
    let response = app.router.handle(get("/", None)).await;
    assert_eq!(location(&response), "/login");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_right_token_opens_the_activity_page_until_the_user_signs_out() {
    let app = app(Some(TOKEN));
    let cookie = sign_in(&app).await;

    let response = app.router.handle(get("/", Some(&cookie))).await;
    assert_eq!(response.status(), StatusCode::OK);
    let page = text(response).await;
    assert!(page.contains("Activity"));
    assert!(page.contains("Watchlist"));
    assert!(page.contains("Library"));
    assert!(page.contains("Settings"));
    assert!(page.contains("name=\"raw\""));
    assert!(page.contains("No jobs yet."));

    let response = app.router.handle(post("/logout", "", Some(&cookie))).await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let response = app.router.handle(get("/", Some(&cookie))).await;
    assert_eq!(location(&response), "/login");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_jobs_api_returns_the_queue_as_json() {
    let app = app(Some(TOKEN));
    let cookie = sign_in(&app).await;
    let response = app.router.handle(get("/api/jobs", Some(&cookie))).await;
    assert_eq!(response.status(), StatusCode::OK);
    let jobs: Value = serde_json::from_str(&text(response).await).unwrap();
    assert_eq!(jobs["open"], serde_json::json!([]));
    assert_eq!(jobs["waiting"], serde_json::json!([]));
}

#[tokio::test(flavor = "multi_thread")]
async fn an_empty_download_request_is_refused_without_queueing_a_job() {
    let app = app(Some(TOKEN));
    let cookie = sign_in(&app).await;
    let response = app
        .router
        .handle(post("/activity/start", "raw=+++", Some(&cookie)))
        .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert!(location(&response).starts_with("/?error="));
    let response = app.router.handle(get("/api/jobs", Some(&cookie))).await;
    let jobs: Value = serde_json::from_str(&text(response).await).unwrap();
    assert_eq!(jobs["open"], serde_json::json!([]));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_server_without_a_token_needs_no_login() {
    let app = app(None);
    let response = app.router.handle(get("/", None)).await;
    assert_eq!(response.status(), StatusCode::OK);
    let response = app.router.handle(get("/api/jobs", None)).await;
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_cross_site_form_post_is_refused() {
    let app = app(Some(TOKEN));
    let cookie = sign_in(&app).await;
    let request = Request::builder()
        .method("POST")
        .uri("/activity/start")
        .header(header::HOST, "localhost")
        .header(header::ORIGIN, "http://evil.example")
        .header(header::COOKIE, cookie)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from("raw=something"))
        .unwrap();
    let response = app.router.handle(request).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

fn decision_request() -> AppEvent {
    AppEvent::DecisionRequest {
        job_id: "queue-1".into(),
        decision_id: "decision-1".into(),
        kind: DecisionKind::ImportDuplicate,
        payload: serde_json::json!({}),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pending_decision_appears_on_the_activity_page_with_its_choices() {
    let app = app(Some(TOKEN));
    let cookie = sign_in(&app).await;
    app.bus.publish(decision_request());
    let response = app.router.handle(get("/", Some(&cookie))).await;
    let page = text(response).await;
    assert!(page.contains("This album is already in the library"));
    assert!(page.contains("Skip the new files"));
    assert!(page.contains("Keep both"));
    assert!(page.contains("Replace the old files"));
    assert!(page.contains("value=\"live:decision-1\""));
}

#[tokio::test(flavor = "multi_thread")]
async fn an_answer_to_a_decision_that_is_not_open_is_reported() {
    let app = app(Some(TOKEN));
    let cookie = sign_in(&app).await;
    let response = app
        .router
        .handle(post(
            "/activity/decision",
            "target=live%3Amissing&choice=0",
            Some(&cookie),
        ))
        .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert!(location(&response).starts_with("/?error="));
    let response = app
        .router
        .handle(post(
            "/activity/decision",
            "target=live%3Adecision-1&choice=9",
            Some(&cookie),
        ))
        .await;
    assert!(location(&response).starts_with("/?error="));
}

#[tokio::test(flavor = "multi_thread")]
async fn the_event_stream_sends_each_event_as_json() {
    let app = app(Some(TOKEN));
    let cookie = sign_in(&app).await;
    let response = app.router.handle(get("/api/events", Some(&cookie))).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("text/event-stream")
    );
    let mut stream = response.into_body().into_data_stream();
    app.bus.publish(AppEvent::JobCancelled {
        job_id: "queue-9".into(),
    });
    let chunk = tokio::time::timeout(std::time::Duration::from_secs(5), stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let frame = String::from_utf8(chunk.to_vec()).unwrap();
    let data = frame
        .lines()
        .find_map(|line| line.strip_prefix("data:"))
        .unwrap();
    let event: Value = serde_json::from_str(data.trim()).unwrap();
    assert_eq!(
        event,
        serde_json::json!({"type": "job_cancelled", "data": {"job_id": "queue-9"}})
    );
}

const PLAYLIST_FORM: &str = "url=https%3A%2F%2Fwww.youtube.com%2Fplaylist%3Flist%3DPLabcdefgh12345";

async fn page_text(app: &App, path: &str, cookie: &str) -> String {
    let response = app.router.handle(get(path, Some(cookie))).await;
    assert_eq!(response.status(), StatusCode::OK, "{path}");
    text(response).await
}

#[tokio::test(flavor = "multi_thread")]
async fn each_page_renders_after_sign_in() {
    let app = app(Some(TOKEN));
    let cookie = sign_in(&app).await;
    for (path, heading) in [("/watchlist", "Watchlist"), ("/", "Activity")] {
        let page = page_text(&app, path, &cookie).await;
        assert!(page.contains(&format!("<h1>{heading}</h1>")), "{path}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_source_is_added_renamed_and_removed() {
    let app = app(Some(TOKEN));
    let cookie = sign_in(&app).await;
    let empty = page_text(&app, "/watchlist", &cookie).await;
    assert!(empty.contains("No playlists yet."));

    let response = app
        .router
        .handle(post("/watchlist/add", PLAYLIST_FORM, Some(&cookie)))
        .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let target = location(&response).to_owned();
    assert!(target.starts_with("/watchlist?"), "{target}");
    assert!(target.contains("source=PLabcdefgh12345"), "{target}");
    assert!(target.contains("notice="), "{target}");

    let listed = page_text(&app, "/watchlist?source=PLabcdefgh12345", &cookie).await;
    assert!(listed.contains("id=\"source-PLabcdefgh12345\""));
    assert!(listed.contains("not checked"));

    let response = app
        .router
        .handle(post(
            "/watchlist/rename?source=PLabcdefgh12345",
            "playlist_id=PLabcdefgh12345&title=Road+trip",
            Some(&cookie),
        ))
        .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert!(location(&response).contains("notice="));
    let renamed = page_text(&app, "/watchlist?source=PLabcdefgh12345", &cookie).await;
    assert!(renamed.contains("Road trip"));

    let response = app
        .router
        .handle(post(
            "/watchlist/remove?source=PLabcdefgh12345",
            "playlist_id=PLabcdefgh12345",
            Some(&cookie),
        ))
        .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert!(!location(&response).contains("source=PL"));
    let removed = page_text(&app, "/watchlist", &cookie).await;
    assert!(removed.contains("No playlists yet."));
    assert!(!removed.contains("Road trip"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_source_that_is_not_a_playlist_link_is_refused() {
    let app = app(Some(TOKEN));
    let cookie = sign_in(&app).await;
    for form in ["url=+++", "url=not+a+playlist"] {
        let response = app
            .router
            .handle(post("/watchlist/add", form, Some(&cookie)))
            .await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER, "{form}");
        assert!(location(&response).contains("error="), "{form}");
    }
    let page = page_text(&app, "/watchlist", &cookie).await;
    assert!(page.contains("No playlists yet."));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_thumbnail_is_served_only_from_the_cache_by_a_valid_id() {
    let app = app(Some(TOKEN));
    let cookie = sign_in(&app).await;
    let cache = app.home.path().join("cache");
    std::fs::write(
        cache.join("yt_thumbnail_abcdefghijk.jpg"),
        b"\xff\xd8\xffdata",
    )
    .unwrap();
    std::fs::write(app.home.path().join("secret.txt"), b"secret").unwrap();

    let response = app
        .router
        .handle(get("/watchlist/thumbnail/abcdefghijk", Some(&cookie)))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("image/jpeg")
    );

    for id in [
        "abcdefghijl",
        "short",
        "..%2Fsecret.txt",
        "..%2F..%2Fsecret",
        "%2E%2E%2Fsecret",
        "abcdefghijk.jpg",
        "abcdefghijk%2F..%2F..%2Fsecret.txt",
    ] {
        let response = app
            .router
            .handle(get(&format!("/watchlist/thumbnail/{id}"), Some(&cookie)))
            .await;
        assert!(response.status().is_client_error(), "{id}");
    }

    let response = app
        .router
        .handle(get("/watchlist/thumbnail/abcdefghijk", None))
        .await;
    assert!(response.status().is_redirection());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_command_for_an_item_that_is_not_saved_is_refused_without_queueing_a_job() {
    let app = app(Some(TOKEN));
    let cookie = sign_in(&app).await;
    let response = app
        .router
        .handle(post(
            "/watchlist/item",
            "playlist_id=PLmissing&key=abcdefghijk&action=run",
            Some(&cookie),
        ))
        .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert!(location(&response).contains("error="));
    let response = app
        .router
        .handle(post(
            "/watchlist/item",
            "playlist_id=PLmissing&key=abcdefghijk&action=explode",
            Some(&cookie),
        ))
        .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let response = app.router.handle(get("/api/jobs", Some(&cookie))).await;
    let jobs: Value = serde_json::from_str(&text(response).await).unwrap();
    assert_eq!(jobs["open"], serde_json::json!([]));
}

fn save_library(app: &App) {
    use muzik_library::{Fields, Library, SqlValue};
    let root = app.home.path();
    let database = root.join("library.db");
    let music = root.join("music");
    std::fs::create_dir_all(root.join("beets")).unwrap();
    std::fs::write(
        root.join("beets/config.yaml"),
        format!(
            "library: {}\ndirectory: {}\n",
            database.display(),
            music.display()
        ),
    )
    .unwrap();
    let fields = |pairs: &[(&str, SqlValue)]| -> Fields {
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_owned(), value.clone()))
            .collect()
    };
    Library::open_or_create(&database)
        .unwrap()
        .transaction(|library| {
            for (artist, title, year, songs) in [
                ("Phoniks", "Down To Earth", 2015, ["First", "Second"]),
                (
                    "Tommy Guerrero",
                    "Road to Knowhere",
                    2011,
                    ["Alpha", "Beta"],
                ),
            ] {
                let album_id = library.insert_album(
                    &fields(&[
                        ("albumartist", SqlValue::Text(artist.into())),
                        ("album", SqlValue::Text(title.into())),
                        ("year", SqlValue::Integer(year)),
                    ]),
                    &Fields::new(),
                )?;
                for (number, song) in (1..).zip(songs) {
                    library.insert_item(
                        &fields(&[
                            ("album_id", SqlValue::Integer(album_id)),
                            ("track", SqlValue::Integer(number)),
                            ("title", SqlValue::Text(song.into())),
                            ("artist", SqlValue::Text(artist.into())),
                            ("length", SqlValue::Real(60.)),
                            ("format", SqlValue::Text("FLAC".into())),
                            (
                                "path",
                                SqlValue::Text(format!("{}/{artist}/{song}.flac", music.display())),
                            ),
                        ]),
                        &Fields::new(),
                    )?;
                }
            }
            Ok(())
        })
        .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn the_library_page_reports_a_library_that_cannot_be_read() {
    let app = app(Some(TOKEN));
    let cookie = sign_in(&app).await;
    let page = page_text(&app, "/library", &cookie).await;
    assert!(page.contains("<h1>Library</h1>"));
    assert!(page.contains("Muzik cannot read the library database."));
}

#[tokio::test(flavor = "multi_thread")]
async fn the_library_page_lists_filters_and_opens_albums_without_any_command() {
    let app = app(Some(TOKEN));
    save_library(&app);
    let cookie = sign_in(&app).await;

    let all = page_text(&app, "/library", &cookie).await;
    assert!(all.contains("Down To Earth"));
    assert!(all.contains("Road to Knowhere"));
    assert!(all.contains("2 albums"));
    assert!(all.contains("4 tracks"));
    assert!(all.contains("Select an album to see its tracks."));
    assert_eq!(all.matches("method=\"post\"").count(), 1);
    assert!(all.contains("action=\"/logout\""));

    let found = page_text(&app, "/library?q=tommy", &cookie).await;
    assert!(found.contains("Road to Knowhere"));
    assert!(!found.contains("Down To Earth"));
    assert!(found.contains("1 of 2 albums"));

    let none = page_text(&app, "/library?q=nothing+like+it", &cookie).await;
    assert!(none.contains("No albums match."));

    let sorted = page_text(&app, "/library?sort=year", &cookie).await;
    let older = sorted.find("Road to Knowhere").unwrap();
    let newer = sorted.find("Down To Earth").unwrap();
    assert!(older < newer);

    let opened = page_text(&app, "/library?album=0", &cookie).await;
    assert!(opened.contains("First"));
    assert!(opened.contains("Second"));
    assert!(opened.contains("Folder"));
}

#[tokio::test(flavor = "multi_thread")]
async fn the_settings_page_shows_every_section() {
    let app = app(Some(TOKEN));
    let cookie = sign_in(&app).await;
    let page = page_text(&app, "/settings", &cookie).await;
    assert!(page.contains("<h1>Settings</h1>"));
    for section in [
        "Destinations",
        "Sources and quality",
        "Processing",
        "AI decisions",
        "After each import",
        "Playlists",
        "Soulseek",
        "Bandcamp",
        "Spotify",
        "Services",
        "Library maintenance",
    ] {
        assert!(page.contains(&format!("<h2>{section}</h2>")), "{section}");
    }
    assert!(page.contains("No service checks yet."));
}

#[tokio::test(flavor = "multi_thread")]
async fn saved_defaults_come_back_on_the_settings_page() {
    let app = app(Some(TOKEN));
    let cookie = sign_in(&app).await;
    let response = app
        .router
        .handle(post(
            "/settings/defaults",
            "jobs=3&min_bitrate=192&agent_model=test-model&audio_source=soulseek&review=on",
            Some(&cookie),
        ))
        .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert!(location(&response).contains("notice=Config+saved"));
    let page = page_text(&app, "/settings", &cookie).await;
    assert!(page.contains("name=\"jobs\" value=\"3\""));
    assert!(page.contains("name=\"min_bitrate\" value=\"192\""));
    assert!(page.contains("name=\"agent_model\" value=\"test-model\""));
    assert!(page.contains("<option value=\"soulseek\" selected=\"\">"));
    assert!(page.contains("name=\"review\" checked=\"\""));
    assert!(!page.contains("name=\"interactive\" checked"));
}

#[tokio::test(flavor = "multi_thread")]
async fn defaults_that_are_not_valid_are_refused_and_not_saved() {
    let app = app(Some(TOKEN));
    let cookie = sign_in(&app).await;
    for form in ["fallback=nonsense", "jobs=many", "min_bitrate=-4"] {
        let response = app
            .router
            .handle(post("/settings/defaults", form, Some(&cookie)))
            .await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER, "{form}");
        assert!(location(&response).contains("error="), "{form}");
    }
    let page = page_text(&app, "/settings", &cookie).await;
    assert!(!page.contains("name=\"jobs\" value=\"many\""));
}

#[tokio::test(flavor = "multi_thread")]
async fn the_soulseek_password_is_saved_but_never_sent_back() {
    let app = app(Some(TOKEN));
    let cookie = sign_in(&app).await;
    let password = "hunter2-very-secret";

    let response = app
        .router
        .handle(post(
            "/settings/soulseek",
            "username=listener&password=&server_host=&server_port=2416",
            Some(&cookie),
        ))
        .await;
    assert!(location(&response).contains("error="));

    let form = format!(
        "username=listener&password={password}&server_host=server.example&server_port=2416"
    );
    let response = app
        .router
        .handle(post("/settings/soulseek", &form, Some(&cookie)))
        .await;
    assert!(location(&response).contains("notice="));
    let page = page_text(&app, "/settings", &cookie).await;
    assert!(page.contains("value=\"listener\""));
    assert!(page.contains("value=\"server.example\""));
    assert!(page.contains("Saved. Type a new one to change it."));
    assert!(!page.contains(password));
    assert!(!location(&response).contains(password));
}

#[tokio::test(flavor = "multi_thread")]
async fn the_spotify_page_tells_the_user_to_log_in_on_the_server() {
    let app = app(Some(TOKEN));
    let cookie = sign_in(&app).await;
    let page = page_text(&app, "/settings", &cookie).await;
    assert!(page.contains("Save a client ID to connect your account."));
    let response = app
        .router
        .handle(post(
            "/settings/spotify",
            "client_id=abc123clientid",
            Some(&cookie),
        ))
        .await;
    assert!(location(&response).contains("notice="));
    let page = page_text(&app, "/settings", &cookie).await;
    assert!(page.contains("value=\"abc123clientid\""));
    assert!(page.contains("muzik spotify login"));
    assert!(page.contains("http://127.0.0.1:8888/callback"));
}

#[tokio::test(flavor = "multi_thread")]
async fn the_service_check_runs_only_when_asked_and_a_task_that_is_unknown_is_refused() {
    let app = app(Some(TOKEN));
    let cookie = sign_in(&app).await;
    let response = app
        .router
        .handle(post("/settings/services", "", Some(&cookie)))
        .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let page = page_text(&app, "/settings", &cookie).await;
    assert!(!page.contains("No service checks yet."));
    assert!(page.contains("ffmpeg"));
    assert!(page.contains("Not set up"));

    let response = app
        .router
        .handle(post(
            "/settings/library-task",
            "task=explode",
            Some(&cookie),
        ))
        .await;
    assert!(location(&response).contains("error="));
    let response = app.router.handle(get("/api/jobs", Some(&cookie))).await;
    let jobs: Value = serde_json::from_str(&text(response).await).unwrap();
    assert_eq!(jobs["open"], serde_json::json!([]));
}
