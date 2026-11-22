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
    _home: TempDir,
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
        _home: home,
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
