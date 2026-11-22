use std::{net::SocketAddr, process::ExitCode};

use muzik_runner::service::Service;
use muzik_server::{Config, auth::resolve_token, build};
use usage_rs::Cli;

const DEFAULT_BIND: &str = "127.0.0.1:8080";

/// Serve the muzik web app.
#[derive(Cli)]
#[usage(bin = "muzik-server", version = env!("CARGO_PKG_VERSION"))]
struct Server {
    /// Address to listen on. Defaults to `MUZIK_SERVER_BIND`, then 127.0.0.1:8080.
    #[usage(long)]
    bind: Option<String>,
    /// Send the session cookie without the Secure attribute, for plain HTTP on a trusted network.
    #[usage(long)]
    insecure_cookie: bool,
}

fn setup(options: &Server) -> Result<(SocketAddr, Config), String> {
    let from_environment = std::env::var("MUZIK_SERVER_BIND").ok();
    let bind = options
        .bind
        .as_deref()
        .or(from_environment.as_deref())
        .unwrap_or(DEFAULT_BIND);
    let address: SocketAddr = bind
        .parse()
        .map_err(|error| format!("The bind address {bind} is not valid: {error}"))?;
    let token = resolve_token(std::env::var("MUZIK_SERVER_TOKEN").ok(), address)
        .map_err(|error| error.to_string())?;
    if token.is_none() {
        tracing::warn!(
            "MUZIK_SERVER_TOKEN is not set; anyone who can reach {address} has full access"
        );
    }
    Ok((
        address,
        Config {
            token,
            secure_cookie: !options.insecure_cookie,
            ..Config::default()
        },
    ))
}

async fn run(options: Server) -> Result<(), String> {
    let (address, config) = setup(&options)?;
    let paths = Service::user_paths().map_err(|error| error.to_string())?;
    let router = build(paths, config)
        .map_err(|error| error.to_string())?
        .router;
    let listener = tokio::net::TcpListener::bind(address)
        .await
        .map_err(|error| format!("Cannot listen on {address}: {error}"))?;
    tracing::info!("listening on http://{address}");
    topcoat::serve(listener, router)
        .await
        .map_err(|error| error.to_string())
}

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("MUZIK_LOG")
                .unwrap_or_else(|_| "warn,muzik_server=info".into()),
        )
        .with_writer(std::io::stderr)
        .init();
    match run(Server::parse()).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::FAILURE
        }
    }
}
