pub mod activity;
pub mod api;
pub mod assets;
pub mod auth;
pub mod hub;
pub mod library;
pub mod pages;
pub mod state;
pub mod ui;
pub mod watchlist;

use std::sync::Arc;

use muzik_core::paths::Paths;
use muzik_runner::{AppEvent, service::Service};
use tokio::sync::broadcast;
use topcoat::{
    asset::RouterBuilderAssetExt,
    cookie::CookieLayer,
    router::Router,
    runtime::RuntimeLayer,
    session::{SessionLifetime, SessionTokenStore},
};

use crate::{
    auth::Auth,
    hub::Hub,
    state::{Backend, EVENT_BUFFER, Events, Shared},
};

pub struct Config {
    pub token: Option<String>,
    pub secure_cookie: bool,
}

#[derive(Clone)]
pub struct Bus {
    hub: Arc<Hub>,
    sender: broadcast::Sender<AppEvent>,
}

impl Bus {
    pub fn publish(&self, event: AppEvent) {
        self.hub.apply(&event);
        let _ = self.sender.send(event);
    }
}

pub struct Server {
    pub router: Router,
    pub bus: Bus,
}

/// # Errors
/// Returns an error when the job queue cannot be opened or the runner cannot start.
pub fn build(paths: Paths, config: Config) -> muzik_runner::Result<Server> {
    let bus = Bus {
        hub: Arc::new(Hub::new()),
        sender: broadcast::channel::<AppEvent>(EVENT_BUFFER).0,
    };
    let service = Service::start(paths, {
        let bus = bus.clone();
        Arc::new(move |event| bus.publish(event))
    })?;
    let auth = Auth::new(config.token, config.secure_cookie);
    let tokens = SessionTokenStore::new(auth.token_store());
    let router = Router::builder()
        .with(Backend(Arc::new(service)))
        .with(Events(bus.sender.clone()))
        .with(Shared(Arc::clone(&bus.hub)))
        .with(auth)
        .with(tokens)
        .with(SessionLifetime(auth::SESSION_LIFETIME))
        .assets(assets::config())
        .layout(ui::shell)
        .layer(auth::guard)
        .layer(CookieLayer::new())
        .layer(RuntimeLayer::new())
        .page(pages::login)
        .route(pages::login_submit)
        .route(pages::logout)
        .page(watchlist::page)
        .route(watchlist::add)
        .route(watchlist::rename)
        .route(watchlist::remove)
        .route(watchlist::playlist_file)
        .route(watchlist::refresh)
        .route(watchlist::run_item)
        .route(watchlist::retry_failed)
        .route(watchlist::thumbnail)
        .route(watchlist::live_board)
        .page(library::page)
        .page(pages::settings)
        .page(activity::page)
        .route(activity::start_download)
        .route(activity::cancel)
        .route(activity::decide)
        .route(activity::panel)
        .route(api::jobs)
        .route(api::events_stream)
        .route(assets::runtime_script)
        .build();
    Ok(Server { router, bus })
}
