//! Native command-line entry point.

mod bandcamp;
mod cache;
mod config;
mod desktop;
mod download;
mod downloaded;
mod gui;
mod import;
mod init;
mod soulseek;
mod spotify;
use muzik_core::paths;
mod validate;

use std::path::PathBuf;

use usage::{Args, Cli, Subcommands};

/// Download, tag, and organize music.
#[derive(Cli)]
#[usage(bin = "muzik", version = env!("CARGO_PKG_VERSION"), completion)]
struct Muzik {
    #[usage(subcommand)]
    command: Command,
}

#[derive(Subcommands)]
enum Command {
    /// Download a Bandcamp collection with bandsnatch.
    Bandcamp(Bandcamp),
    /// Manage cached data.
    Cache(Cache),
    /// Manage library and service settings.
    Config(Config),
    /// Download audio from YouTube with yt-dlp.
    Download(Download),
    /// Show downloaded audio files.
    Downloaded(Downloaded),
    /// Open the desktop app.
    Gui,
    /// Create app directories and library defaults.
    Init,
    /// Install the macOS desktop app bundle.
    InstallApp(InstallApp),
    /// Import audio into a beets-compatible library.
    Import(Import),
    /// Manage a Spotify account.
    Spotify(Spotify),
    /// Check, search, and download from Soulseek.
    Soulseek(Soulseek),
    /// Check audio files and metadata sidecars.
    Validate(Validate),
}

#[derive(Args)]
struct Validate {
    /// File or directory to check.
    path: PathBuf,
    /// Check all files in subdirectories.
    #[usage(long, short = 'r')]
    recursive: bool,
    /// Show file details and warnings.
    #[usage(long, short = 'v')]
    verbose: bool,
}

#[derive(Args)]
struct Import {
    /// Audio file or directory to import.
    directory: Option<PathBuf>,
    /// Re-tag library items selected by a beets query.
    #[usage(long)]
    library: Option<String>,
    /// Copy source files instead of moving them.
    #[usage(long, short = 'C')]
    copy: bool,
    /// Link source files instead of moving them. Requires --nowrite.
    #[usage(long, short = 'L')]
    link: bool,
    /// Keep the source tags unchanged.
    #[usage(long)]
    nowrite: bool,
    /// Skip albums that need a match decision.
    #[usage(long, short = 'q')]
    quiet: bool,
    /// Show planned file paths without writing.
    #[usage(long, short = 'd')]
    dry_run: bool,
    /// Keep missing library rows after moving files.
    #[usage(long)]
    no_prune: bool,
    /// Beets-compatible library config file.
    #[usage(long, short = 'c')]
    config: Option<PathBuf>,
}

#[derive(Args)]
struct InstallApp {
    /// Install to ~/Applications instead of /Applications.
    #[usage(long)]
    user: bool,
}

#[derive(Args)]
struct Cache {
    #[usage(subcommand)]
    command: CacheCommand,
}

#[derive(Subcommands)]
enum CacheCommand {
    /// List cached files.
    List,
    /// Remove one key or all cached files.
    Clear(CacheClear),
    /// Show cache size.
    Size,
    /// Remove cache, downloads, and split files.
    Purge,
    /// Remove old or empty cache files.
    Clean(CacheClean),
}

#[derive(Args)]
struct CacheClear {
    /// Cache key to remove. Omit to clear all files.
    key: Option<String>,
}

#[derive(Args)]
struct CacheClean {
    /// Remove files older than this many days.
    #[usage(long, default = "30")]
    max_age: u64,
}

#[derive(Args)]
struct Config {
    #[usage(subcommand)]
    command: ConfigCommand,
}

#[derive(Subcommands)]
enum ConfigCommand {
    /// Show library and service settings.
    Show(ConfigFile),
    /// Set the music library directory.
    SetLibrary(SetLibrary),
    /// Set Soulseek connection settings.
    SetSoulseek(SetSoulseek),
    /// Open the library config in an editor.
    Edit(ConfigFile),
}

#[derive(Args)]
struct ConfigFile {
    /// Beets-compatible library config file.
    #[usage(long, short = 'c')]
    config: Option<PathBuf>,
}

#[derive(Args)]
struct SetLibrary {
    /// Music library directory.
    directory: PathBuf,
    /// Library SQLite database path.
    #[usage(long)]
    db: Option<PathBuf>,
    /// Beets-compatible library config file.
    #[usage(long, short = 'c')]
    config: Option<PathBuf>,
}

#[derive(Args)]
struct SetSoulseek {
    /// Soulseek username.
    #[usage(long)]
    username: Option<String>,
    /// Soulseek password.
    #[usage(long)]
    password: Option<String>,
    /// Soulseek server hostname.
    #[usage(long)]
    server_host: Option<String>,
    /// Soulseek server port.
    #[usage(long)]
    server_port: Option<u16>,
    /// Completed download directory.
    #[usage(long)]
    download_dir: Option<PathBuf>,
}

#[derive(Args)]
struct Downloaded {
    /// Folder to inspect.
    #[usage(long, short = 'o')]
    output: Option<PathBuf>,
}

#[derive(Args)]
struct Download {
    /// YouTube video or playlist URL.
    url: String,
    /// Folder for downloaded audio.
    #[usage(long, short = 'o')]
    output: Option<PathBuf>,
    /// yt-dlp format selector.
    #[usage(long, short = 'f', default = "bestaudio")]
    format: String,
    /// Audio quality passed to yt-dlp.
    #[usage(long, short = 'q', default = "0")]
    quality: String,
    /// Skip chapter data.
    #[usage(long)]
    no_chapters: bool,
    /// yt-dlp download archive.
    #[usage(long, hide)]
    archive_file: Option<PathBuf>,
}

#[derive(Args)]
struct Bandcamp {
    /// Bandcamp username.
    user: Option<String>,
    /// Folder for downloaded releases.
    #[usage(long, short = 'o')]
    output: Option<PathBuf>,
    /// Audio format.
    #[usage(
        long,
        short = 'f',
        default = "flac",
        choices(
            "flac",
            "wav",
            "aac-hi",
            "mp3-320",
            "aiff-lossless",
            "vorbis",
            "mp3-v0",
            "alac"
        )
    )]
    format: String,
    /// Path to a Bandcamp cookie file.
    #[usage(long, short = 'c')]
    cookies: Option<PathBuf>,
    /// Number of download jobs.
    #[usage(long, short = 'j', default = "4")]
    jobs: u8,
    /// List releases without downloading them.
    #[usage(long, short = 'd')]
    dry_run: bool,
    /// Download releases that are already in the bandsnatch cache.
    #[usage(long, short = 'F')]
    force: bool,
}

#[derive(Args)]
struct Spotify {
    #[usage(subcommand)]
    command: SpotifyCommand,
}

#[derive(Subcommands)]
enum SpotifyCommand {
    /// Connect a Spotify account in your browser.
    Login(SpotifyLogin),
    /// Show the Spotify client ID, redirect URI, and account connection.
    Status,
    /// List Liked Songs and your Spotify playlists.
    Playlists,
    /// Read a Spotify playlist or album as JSON metadata.
    Export(SpotifyExport),
    /// Add a Spotify playlist or album to the watchlist.
    Watch(SpotifyWatch),
    /// Save the client ID of your Spotify application.
    SetClientId(SetSpotifyClientId),
    /// Remove saved Spotify tokens.
    Logout,
}

#[derive(Args)]
struct SpotifyLogin {
    /// Loopback port for the browser redirect.
    #[usage(long, short = 'p')]
    port: Option<u16>,
}

#[derive(Args)]
struct SpotifyExport {
    /// Spotify playlist or album URI, or spotify:liked.
    uri: String,
    /// Write the JSON document to this file instead of standard output.
    #[usage(long, short = 'o')]
    output: Option<PathBuf>,
}

#[derive(Args)]
struct SpotifyWatch {
    /// Spotify playlist or album URI or link, or liked.
    reference: String,
}

#[derive(Args)]
struct Soulseek {
    #[usage(subcommand)]
    command: SoulseekCommand,
}

#[derive(Subcommands)]
enum SoulseekCommand {
    /// Check the Soulseek account and server connection.
    Check,
    /// Search peer audio files and rank the results.
    Search(SoulseekSearch),
    /// Download a Soulseek result and organize its audio files.
    Download(SoulseekDownload),
}

#[derive(Args)]
struct SoulseekSearch {
    /// Artist, album, or track search text.
    query: String,
    /// Preferred audio quality.
    #[usage(
        long,
        default = "lossless",
        choices("flac", "lossless", "mp3-320", "any")
    )]
    prefer: String,
    /// Maximum number of results to show.
    #[usage(long, short = 'n', default = "20")]
    limit: usize,
    /// Print structured JSON results.
    #[usage(long)]
    json: bool,
}

#[derive(Args)]
struct SoulseekDownload {
    /// Search text. Use --candidate to select a saved search result.
    query: Option<String>,
    /// Candidate ID shown by `muzik soulseek search`.
    #[usage(long)]
    candidate: Option<String>,
    /// Preferred audio quality for a new search.
    #[usage(
        long,
        default = "lossless",
        choices("flac", "lossless", "mp3-320", "any")
    )]
    prefer: String,
    /// Maximum number of results to consider.
    #[usage(long, short = 'n', default = "10")]
    limit: usize,
    /// Root directory for Soulseek downloads.
    #[usage(long, short = 'o')]
    output: Option<PathBuf>,
    /// Select the highest-ranked result without a prompt.
    #[usage(long)]
    no_interactive: bool,
    /// Keep downloaded files out of the music library.
    #[usage(long)]
    no_organize: bool,
    /// Show the selected result without downloading.
    #[usage(long, short = 'd')]
    dry_run: bool,
}

#[derive(Args)]
struct SetSpotifyClientId {
    /// Client ID of your Spotify application.
    client_id: String,
}

async fn run(command: Command) -> Result<(), String> {
    match command {
        Command::Bandcamp(args) => bandcamp::download(&args).map_err(|error| error.to_string()),
        Command::Cache(args) => match args.command {
            CacheCommand::List => cache::list(),
            CacheCommand::Clear(args) => cache::clear(args.key.as_deref()),
            CacheCommand::Size => cache::size(),
            CacheCommand::Purge => cache::purge(),
            CacheCommand::Clean(args) => cache::clean(args.max_age),
        }
        .map_err(|error| error.to_string()),
        Command::Config(args) => match args.command {
            ConfigCommand::Show(args) => config::show(args.config.as_deref()),
            ConfigCommand::SetLibrary(args) => {
                config::set_library(&args.directory, args.db.as_deref(), args.config.as_deref())
            }
            ConfigCommand::SetSoulseek(args) => config::set_soulseek(&args),
            ConfigCommand::Edit(args) => config::edit(args.config.as_deref()),
        }
        .map_err(|error| error.to_string()),
        Command::Download(args) => download::run(&args).await,
        Command::Downloaded(args) => {
            let output = args.output.unwrap_or_else(paths::download_dir);
            downloaded::list(&output).map_err(|error| error.to_string())
        }
        Command::Gui => gui::open().map_err(|error| error.to_string()),
        Command::Init => init::run().map_err(|error| error.to_string()),
        Command::InstallApp(args) => desktop::install(args.user).map_err(|error| error.to_string()),
        Command::Import(args) => import::run(&args),
        Command::Spotify(args) => match args.command {
            SpotifyCommand::Login(args) => spotify::login(args.port),
            SpotifyCommand::Status => spotify::status(),
            SpotifyCommand::Playlists => spotify::playlists(),
            SpotifyCommand::Export(args) => spotify::export(&args.uri, args.output.as_deref()),
            SpotifyCommand::Watch(args) => spotify::watch(&args.reference),
            SpotifyCommand::SetClientId(args) => spotify::set_client_id(&args.client_id),
            SpotifyCommand::Logout => spotify::logout(),
        },
        Command::Soulseek(args) => match args.command {
            SoulseekCommand::Check => soulseek::check(),
            SoulseekCommand::Search(args) => {
                soulseek::search(&args.query, &args.prefer, args.limit, args.json)
            }
            SoulseekCommand::Download(args) => soulseek::download(&args),
        },
        Command::Validate(args) => validate::run(&args),
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> std::process::ExitCode {
    if let Err(error) = run(Muzik::parse().command).await {
        eprintln!("error: {error}");
        std::process::ExitCode::FAILURE
    } else {
        std::process::ExitCode::SUCCESS
    }
}
