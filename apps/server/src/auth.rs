use std::{
    borrow::Cow,
    collections::HashMap,
    net::SocketAddr,
    sync::Arc,
    time::{Duration, Instant, SystemTime},
};

use parking_lot::Mutex;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use topcoat::{
    Result,
    context::{Cx, context},
    cookie::{Cookie, Cookies, SameSite},
    router::{
        Body, Next,
        error::{RouterErrorExt, redirect, too_many_requests, unauthorized},
        href, layer,
        request::{self, client_ip},
        response::Response,
    },
    session::{self, Session, Token, TokenHash, TokenStore, TokenStoreFuture},
};

pub const MIN_TOKEN_LENGTH: usize = 16;
pub const SESSION_LIFETIME: Duration = Duration::from_hours(168);
const FAILURE_DELAY: Duration = Duration::from_millis(400);
const FAILURE_LIMIT: u32 = 10;
const FAILURE_WINDOW: Duration = Duration::from_mins(1);

#[derive(Debug, PartialEq, Eq)]
pub enum TokenError {
    Missing,
    TooShort,
}

impl std::fmt::Display for TokenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Missing => write!(
                f,
                "MUZIK_SERVER_TOKEN is not set. Set it to a secret of at least {MIN_TOKEN_LENGTH} characters, or bind to a loopback address."
            ),
            Self::TooShort => write!(
                f,
                "MUZIK_SERVER_TOKEN must have at least {MIN_TOKEN_LENGTH} characters unless the server binds to a loopback address."
            ),
        }
    }
}

impl std::error::Error for TokenError {}

/// # Errors
/// Returns an error when the address is not loopback and the token is missing or too short.
pub fn resolve_token(
    token: Option<String>,
    bind: SocketAddr,
) -> Result<Option<String>, TokenError> {
    let token = token.filter(|token| !token.is_empty());
    if bind.ip().is_loopback() {
        return Ok(token);
    }
    match token {
        None => Err(TokenError::Missing),
        Some(token) if token.chars().count() < MIN_TOKEN_LENGTH => Err(TokenError::TooShort),
        Some(token) => Ok(Some(token)),
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CookieMode {
    #[default]
    Secure,
    Insecure,
}

impl CookieMode {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Secure => "__Host-muzik-session",
            Self::Insecure => "muzik-session",
        }
    }

    #[must_use]
    pub const fn secure(self) -> bool {
        matches!(self, Self::Secure)
    }
}

#[derive(Default)]
struct Sessions(HashMap<TokenHash, SystemTime>);

impl Sessions {
    fn remember(&mut self, hash: TokenHash, expires_at: SystemTime, now: SystemTime) {
        self.0.retain(|_, expires_at| *expires_at > now);
        self.0.insert(hash, expires_at);
    }

    fn knows(&self, hash: &TokenHash, now: SystemTime) -> bool {
        self.0.get(hash).is_some_and(|expires_at| *expires_at > now)
    }

    fn forget(&mut self, hash: &TokenHash) {
        self.0.remove(hash);
    }

    fn len(&self) -> usize {
        self.0.len()
    }
}

#[derive(Debug, Default)]
struct Failures {
    window_start: Option<Instant>,
    count: u32,
}

impl Failures {
    fn roll(&mut self, now: Instant) {
        if self
            .window_start
            .is_some_and(|start| now.saturating_duration_since(start) >= FAILURE_WINDOW)
        {
            *self = Self::default();
        }
    }

    fn retry_after(&mut self, now: Instant) -> Option<Duration> {
        self.roll(now);
        let start = self.window_start.filter(|_| self.count >= FAILURE_LIMIT)?;
        Some(FAILURE_WINDOW.saturating_sub(now.saturating_duration_since(start)))
    }

    fn record(&mut self, now: Instant) {
        self.roll(now);
        self.window_start.get_or_insert(now);
        self.count = self.count.saturating_add(1);
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Attempt {
    Accepted,
    Rejected,
    Limited(Duration),
}

pub struct Auth {
    digest: Option<[u8; 32]>,
    cookies: CookieMode,
    sessions: Arc<Mutex<Sessions>>,
    failures: Mutex<Failures>,
}

pub struct SessionWatch {
    sessions: Arc<Mutex<Sessions>>,
    hash: Option<TokenHash>,
}

impl SessionWatch {
    #[must_use]
    pub fn is_active(&self, now: SystemTime) -> bool {
        self.hash
            .as_ref()
            .is_none_or(|hash| self.sessions.lock().knows(hash, now))
    }
}

fn digest(text: &str) -> [u8; 32] {
    Sha256::digest(text.as_bytes()).0
}

impl Auth {
    #[must_use]
    pub fn new(token: Option<&str>, cookies: CookieMode) -> Self {
        Self {
            digest: token.map(digest),
            cookies,
            sessions: Arc::default(),
            failures: Mutex::default(),
        }
    }

    #[must_use]
    pub const fn required(&self) -> bool {
        self.digest.is_some()
    }

    #[must_use]
    pub fn verify(&self, candidate: &str) -> bool {
        self.digest
            .is_some_and(|expected| expected.ct_eq(&digest(candidate)).unwrap_u8() == 1)
    }

    pub fn attempt(&self, candidate: &str, now: Instant) -> Attempt {
        let mut failures = self.failures.lock();
        if let Some(wait) = failures.retry_after(now) {
            return Attempt::Limited(wait);
        }
        if self.verify(candidate) {
            Attempt::Accepted
        } else {
            failures.record(now);
            Attempt::Rejected
        }
    }

    #[must_use]
    pub const fn token_store(&self) -> SessionCookie {
        SessionCookie {
            cookies: self.cookies,
        }
    }

    fn remember(&self, session: &Session, now: SystemTime) {
        self.sessions
            .lock()
            .remember(session.token_hash.clone(), session.expires_at, now);
    }

    fn forget(&self, hash: &TokenHash) {
        self.sessions.lock().forget(hash);
    }

    fn knows(&self, hash: &TokenHash, now: SystemTime) -> bool {
        self.sessions.lock().knows(hash, now)
    }

    #[must_use]
    pub fn session_count(&self) -> usize {
        self.sessions.lock().len()
    }
}

pub struct SessionCookie {
    cookies: CookieMode,
}

impl SessionCookie {
    const fn name(&self) -> &'static str {
        self.cookies.name()
    }

    fn cookie(&self, value: String) -> Cookie<'static> {
        Cookie::build((Cow::Borrowed(self.name()), value))
            .path("/")
            .http_only(true)
            .secure(self.cookies.secure())
            .same_site(SameSite::Lax)
            .build()
    }
}

impl TokenStore for SessionCookie {
    fn read<'a>(&'a self, cx: &'a Cx) -> TokenStoreFuture<'a, Option<Token>> {
        Box::pin(async move {
            let cookies = topcoat::cookie::cookies(cx);
            let Some(cookie) = cookies.get(self.name()) else {
                return Ok(None);
            };
            Ok(Token::decode(cookie.value_trimmed()).ok())
        })
    }

    fn write<'a>(
        &'a self,
        cx: &'a Cx,
        token: Token,
        max_age: Duration,
    ) -> TokenStoreFuture<'a, ()> {
        Box::pin(async move {
            let max_age = topcoat::cookie::time::Duration::try_from(max_age)?;
            let mut cookie = self.cookie(token.encode());
            cookie.set_max_age(max_age);
            topcoat::cookie::cookies(cx).add(cookie);
            Ok(())
        })
    }

    fn delete<'a>(&'a self, cx: &'a Cx) -> TokenStoreFuture<'a, ()> {
        Box::pin(async move {
            topcoat::cookie::cookies(cx).remove(self.cookie(String::new()));
            Ok(())
        })
    }
}

#[must_use]
pub fn auth(cx: &Cx) -> &Auth {
    context::<Auth>(cx)
}

/// # Errors
/// Returns an error when the session token cannot be read.
pub async fn authenticated(cx: &Cx) -> Result<bool> {
    let auth = auth(cx);
    if !auth.required() {
        return Ok(true);
    }
    let Some(hash) = session::token_hash(cx).await? else {
        return Ok(false);
    };
    Ok(auth.knows(&hash, SystemTime::now()))
}

/// # Errors
/// Returns an error when the session token cannot be read.
pub async fn watch(cx: &Cx) -> Result<SessionWatch> {
    let auth = auth(cx);
    let hash = if auth.required() {
        Some(session::token_hash(cx).await?.ok_or_unauthorized()?)
    } else {
        None
    };
    Ok(SessionWatch {
        sessions: Arc::clone(&auth.sessions),
        hash,
    })
}

/// # Errors
/// Returns an unauthorized error when the request has no valid session.
pub async fn require(cx: &Cx) -> Result<()> {
    authenticated(cx)
        .await?
        .then_some(())
        .ok_or_unauthorized()?;
    Ok(())
}

fn peer(cx: &Cx) -> String {
    client_ip(cx).map_or_else(|| "unknown".to_owned(), |ip| ip.to_string())
}

/// # Errors
/// Returns an error when the session cannot be started, or a too-many-requests error while
/// logins are refused after repeated failures.
pub async fn login(cx: &Cx, candidate: &str) -> Result<bool> {
    let auth = auth(cx);
    match auth.attempt(candidate, Instant::now()) {
        Attempt::Accepted => {
            let session = session::start(cx).await?;
            auth.remember(&session, SystemTime::now());
            Ok(true)
        }
        Attempt::Rejected => {
            tracing::warn!(client_ip = %peer(cx), "failed login");
            tokio::time::sleep(FAILURE_DELAY).await;
            Ok(false)
        }
        Attempt::Limited(wait) => {
            tracing::warn!(client_ip = %peer(cx), "login refused after repeated failures");
            Err(too_many_requests(wait.as_secs().max(1)).into())
        }
    }
}

/// # Errors
/// Returns an error when the session cannot be stopped.
pub async fn logout(cx: &Cx) -> Result<()> {
    if let Some(hash) = session::stop(cx).await? {
        auth(cx).forget(&hash);
    }
    Ok(())
}

#[layer("/")]
pub async fn guard(cx: &Cx, body: Body, next: Next<'_>) -> Result<Response> {
    let path = request::uri(cx).path();
    let open = path == "/login" || !auth(cx).required();
    if open || authenticated(cx).await? {
        return next.run(cx, body).await;
    }
    if path == "/api" || path.starts_with("/api/") {
        return Err(unauthorized().into());
    }
    Err(redirect(href!(crate::pages::login).resolve(cx)).into())
}

#[cfg(test)]
mod tests {
    use super::{Attempt, Auth, CookieMode, FAILURE_LIMIT, Sessions, TokenError, resolve_token};
    use std::{
        net::SocketAddr,
        time::{Duration, Instant, SystemTime},
    };
    use topcoat::session::TokenHash;

    fn address(text: &str) -> SocketAddr {
        text.parse().unwrap()
    }

    #[test]
    fn a_public_address_needs_a_long_token() {
        let public = address("0.0.0.0:8080");
        assert_eq!(resolve_token(None, public), Err(TokenError::Missing));
        assert_eq!(
            resolve_token(Some(String::new()), public),
            Err(TokenError::Missing)
        );
        assert_eq!(
            resolve_token(Some("short".into()), public),
            Err(TokenError::TooShort)
        );
        let token = "x".repeat(16);
        assert_eq!(resolve_token(Some(token.clone()), public), Ok(Some(token)));
    }

    #[test]
    fn a_loopback_address_allows_no_token_and_keeps_any_token() {
        for text in ["127.0.0.1:8080", "[::1]:8080"] {
            assert_eq!(resolve_token(None, address(text)), Ok(None));
            assert_eq!(
                resolve_token(Some("short".into()), address(text)),
                Ok(Some("short".into()))
            );
        }
    }

    #[test]
    fn only_the_exact_token_is_accepted() {
        let token = "x".repeat(16);
        let auth = Auth::new(Some(&token), CookieMode::Secure);
        assert!(auth.verify(&token));
        assert!(!auth.verify(&token.replacen('x', "y", 1)));
        assert!(!auth.verify(&"x".repeat(15)));
        assert!(!auth.verify(&"x".repeat(17)));
        assert!(!auth.verify(""));
        assert!(!Auth::new(None, CookieMode::Secure).verify(""));
    }

    fn hash(number: u8) -> TokenHash {
        TokenHash::new([number; 32])
    }

    fn later(start: SystemTime, seconds: u64) -> SystemTime {
        start.checked_add(Duration::from_secs(seconds)).unwrap()
    }

    #[test]
    fn a_session_is_refused_after_it_expires() {
        let start = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);
        let mut sessions = Sessions::default();
        sessions.remember(hash(1), later(start, 100), start);
        assert!(sessions.knows(&hash(1), later(start, 99)));
        assert!(!sessions.knows(&hash(1), later(start, 100)));
        assert!(!sessions.knows(&hash(2), start));
        sessions.forget(&hash(1));
        assert!(!sessions.knows(&hash(1), start));
    }

    #[test]
    fn an_expired_session_is_pruned_when_the_next_one_is_remembered() {
        let start = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);
        let mut sessions = Sessions::default();
        sessions.remember(hash(1), later(start, 100), start);
        sessions.remember(hash(2), later(start, 500), later(start, 50));
        assert_eq!(sessions.len(), 2);
        sessions.remember(hash(3), later(start, 900), later(start, 200));
        assert_eq!(sessions.len(), 2);
        assert!(!sessions.knows(&hash(1), start));
        assert!(sessions.knows(&hash(2), later(start, 200)));
        assert!(sessions.knows(&hash(3), later(start, 200)));
    }

    #[test]
    fn logins_are_refused_after_repeated_failures_until_the_minute_ends() {
        let token = "x".repeat(16);
        let auth = Auth::new(Some(&token), CookieMode::Secure);
        let start = Instant::now();
        for _ in 0..FAILURE_LIMIT {
            assert_eq!(auth.attempt("wrong", start), Attempt::Rejected);
        }
        assert_eq!(
            auth.attempt(&token, start + Duration::from_secs(20)),
            Attempt::Limited(Duration::from_secs(40))
        );
        assert_eq!(
            auth.attempt("wrong", start + Duration::from_secs(59)),
            Attempt::Limited(Duration::from_secs(1))
        );
        assert_eq!(
            auth.attempt(&token, start + Duration::from_secs(60)),
            Attempt::Accepted
        );
    }

    #[test]
    fn failures_below_the_limit_do_not_block_a_login() {
        let token = "x".repeat(16);
        let auth = Auth::new(Some(&token), CookieMode::Secure);
        let start = Instant::now();
        for _ in 0..FAILURE_LIMIT - 1 {
            assert_eq!(auth.attempt("wrong", start), Attempt::Rejected);
        }
        assert_eq!(auth.attempt(&token, start), Attempt::Accepted);
        assert_eq!(
            auth.attempt("wrong", start + Duration::from_secs(61)),
            Attempt::Rejected
        );
    }
}
