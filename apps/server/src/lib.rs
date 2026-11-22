pub mod activity;
pub mod api;
pub mod assets;
pub mod auth;
pub mod hub;
pub mod library;
pub mod notice;
pub mod pages;
pub mod settings;
pub mod state;
pub mod ui;
pub mod watchlist;

use std::sync::Arc;

use muzik_core::paths::Paths;
use muzik_runner::{
    AppEvent,
    service::{Service, ServiceOptions},
};
use parking_lot::Mutex;
use tokio::sync::broadcast;
use topcoat::{
    asset::RouterBuilderAssetExt,
    cookie::CookieLayer,
    router::Router,
    runtime::RuntimeLayer,
    session::{SessionLifetime, SessionTokenStore},
};

use crate::{
    auth::{Auth, CookieMode},
    hub::Hub,
    state::{Backend, EVENT_BUFFER, Events, Shared},
};

pub struct Config {
    pub token: Option<String>,
    pub cookies: CookieMode,
    pub event_buffer: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            token: None,
            cookies: CookieMode::Secure,
            event_buffer: EVENT_BUFFER,
        }
    }
}

#[derive(Clone)]
pub struct Bus {
    hub: Arc<Hub>,
    sender: broadcast::Sender<AppEvent>,
    order: Arc<Mutex<()>>,
}

impl Bus {
    pub fn publish(&self, event: AppEvent) {
        let _order = self.order.lock();
        self.hub.apply(&event);
        let _ = self.sender.send(event);
    }

    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<AppEvent> {
        self.sender.subscribe()
    }
}

pub struct Server {
    pub router: Router,
    pub bus: Bus,
}

/// # Errors
/// Returns an error when the job queue cannot be opened or the runner cannot start.
pub fn build(paths: Paths, config: Config) -> muzik_runner::Result<Server> {
    build_with(paths, ServiceOptions::default(), config)
}

/// # Errors
/// Returns an error when the job queue cannot be opened or the runner cannot start.
pub fn build_with(
    paths: Paths,
    options: ServiceOptions,
    config: Config,
) -> muzik_runner::Result<Server> {
    let Config {
        token,
        cookies,
        event_buffer,
    } = config;
    let bus = Bus {
        hub: Arc::new(Hub::new()),
        sender: broadcast::channel::<AppEvent>(event_buffer).0,
        order: Arc::default(),
    };
    let service = Service::start_with(paths, options, {
        let bus = bus.clone();
        Arc::new(move |event| bus.publish(event))
    })?;
    let auth = Auth::new(token.as_deref(), cookies);
    let tokens = SessionTokenStore::new(auth.token_store());
    let router = Router::builder()
        .with(Backend(Arc::new(service)))
        .with(Events::new(bus.clone()))
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
        .page(settings::page)
        .route(settings::save_defaults)
        .route(settings::save_soulseek)
        .route(settings::save_bandcamp)
        .route(settings::logout_bandcamp)
        .route(settings::save_spotify_client)
        .route(settings::logout_spotify)
        .route(settings::check_services)
        .route(settings::start_task)
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
