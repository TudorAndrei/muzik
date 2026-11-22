use std::{
    borrow::Cow,
    collections::HashMap,
    net::SocketAddr,
    sync::Arc,
    time::{Duration, SystemTime},
};

use parking_lot::Mutex;
use subtle::ConstantTimeEq;
use topcoat::{
    Result,
    context::{Cx, context},
    cookie::{Cookie, Cookies, SameSite},
    router::{
        Body, Next,
        error::{RouterErrorExt, redirect, unauthorized},
        href, layer, request,
        response::Response,
    },
    session::{self, Session, Token, TokenHash, TokenStore, TokenStoreFuture},
};

pub const MIN_TOKEN_LENGTH: usize = 16;
pub const SESSION_LIFETIME: Duration = Duration::from_hours(168);
const FAILURE_DELAY: Duration = Duration::from_millis(400);

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

pub struct Auth {
    token: Option<String>,
    secure_cookie: bool,
    sessions: Arc<Mutex<Sessions>>,
}

type Sessions = HashMap<TokenHash, SystemTime>;

pub struct SessionWatch {
    sessions: Arc<Mutex<Sessions>>,
    hash: Option<TokenHash>,
}

impl SessionWatch {
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.hash.as_ref().is_none_or(|hash| {
            self.sessions
                .lock()
                .get(hash)
                .is_some_and(|expires_at| *expires_at > SystemTime::now())
        })
    }
}

impl Auth {
    #[must_use]
    pub fn new(token: Option<String>, secure_cookie: bool) -> Self {
        Self {
            token,
            secure_cookie,
            sessions: Arc::default(),
        }
    }

    #[must_use]
    pub const fn required(&self) -> bool {
        self.token.is_some()
    }

    #[must_use]
    pub fn verify(&self, candidate: &str) -> bool {
        self.token
            .as_deref()
            .is_some_and(|token| token.as_bytes().ct_eq(candidate.as_bytes()).unwrap_u8() == 1)
    }

    #[must_use]
    pub const fn token_store(&self) -> SessionCookie {
        SessionCookie {
            secure: self.secure_cookie,
        }
    }

    fn remember(&self, session: &Session) {
        let mut sessions = self.sessions.lock();
        let now = SystemTime::now();
        sessions.retain(|_, expires_at| *expires_at > now);
        sessions.insert(session.token_hash.clone(), session.expires_at);
    }

    fn forget(&self, hash: &TokenHash) {
        self.sessions.lock().remove(hash);
    }

    fn knows(&self, hash: &TokenHash) -> bool {
        self.sessions
            .lock()
            .get(hash)
            .is_some_and(|expires_at| *expires_at > SystemTime::now())
    }

    #[must_use]
    pub fn session_count(&self) -> usize {
        self.sessions.lock().len()
    }
}

pub struct SessionCookie {
    secure: bool,
}

impl SessionCookie {
    const fn name(&self) -> &'static str {
        if self.secure {
            "__Host-muzik-session"
        } else {
            "muzik-session"
        }
    }

    fn cookie(&self, value: String) -> Cookie<'static> {
        Cookie::build((Cow::Borrowed(self.name()), value))
            .path("/")
            .http_only(true)
            .secure(self.secure)
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
    Ok(auth.knows(&hash))
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

/// # Errors
/// Returns an error when the session cannot be started.
pub async fn login(cx: &Cx, candidate: &str) -> Result<bool> {
    let auth = auth(cx);
    if !auth.verify(candidate) {
        tokio::time::sleep(FAILURE_DELAY).await;
        return Ok(false);
    }
    let session = session::start(cx).await?;
    auth.remember(&session);
    Ok(true)
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
    use super::{Auth, TokenError, resolve_token};
    use std::net::SocketAddr;

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
        let auth = Auth::new(Some(token.clone()), true);
        assert!(auth.verify(&token));
        assert!(!auth.verify(&token.replacen('x', "y", 1)));
        assert!(!auth.verify(&"x".repeat(15)));
        assert!(!auth.verify(""));
        assert!(!Auth::new(None, true).verify(""));
    }
}
