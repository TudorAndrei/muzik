mod archive;
mod bandcamp;
mod cache;
mod config;
mod download;
mod downloaded;
mod import;
mod init;
mod jobs;
mod organize;
mod soulseek;
mod split;
mod spotify;
mod sync;
use muzik_core::paths;
mod validate;
mod watchlist;
mod workflow;

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
    /// Split and organize audio files already on disk.
    Archive(Archive),
    /// Download the purchases of a Bandcamp collection.
    Bandcamp(Bandcamp),
    /// Manage cached data.
    Cache(Cache),
    /// Manage library and service settings.
    Config(Config),
    /// Download audio from YouTube with yt-dlp.
    Download(Download),
    /// Show downloaded audio files.
    Downloaded(Downloaded),
    /// Create app directories and library defaults.
    Init,
    /// Import audio into a beets-compatible library.
    Import(Import),
    /// Show, answer, cancel, and run queued jobs.
    Jobs(Jobs),
    /// Import audio by default, or write tags from the music library.
    Organize(Organize),
    /// Manage a Spotify account.
    Spotify(Spotify),
    /// Check, search, and download from Soulseek.
    Soulseek(Soulseek),
    /// Split an audio file at chapter markers.
    Split(Split),
    /// Copy library tracks to a device in formats the device plays.
    Sync(Sync),
    /// Check audio files and metadata sidecars.
    Validate(Validate),
    /// Manage watched playlists and queue their items.
    Watchlist(Watchlist),
    /// Download or process audio, split chapters, and organize tracks.
    Workflow(Workflow),
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
    /// What to do with an album that is already in the library.
    #[usage(long, value_enum, default = "skip")]
    duplicates: muzik_core::DuplicatePolicy,
    /// Beets-compatible library config file.
    #[usage(long, short = 'c')]
    config: Option<PathBuf>,
}

#[derive(Args)]
struct Organize {
    /// Directory containing audio tracks, or a library file or directory for --tag-only.
    directory: PathBuf,
    /// Import files into the music library (the default; kept for old commands).
    #[usage(long, short = 'i')]
    import: bool,
    /// Write tags from existing library records without moving files.
    #[usage(long, short = 't')]
    tag_only: bool,
    /// Show planned changes without writing them.
    #[usage(long, short = 'd')]
    dry_run: bool,
    /// Beets-compatible library config file.
    #[usage(long, short = 'c')]
    config: Option<PathBuf>,
}

#[derive(Args)]
struct Split {
    /// Audio file to split.
    path: PathBuf,
    /// Review chapter titles in an editor before splitting.
    #[usage(long, short = 'r')]
    review: bool,
    /// Number of parallel ffmpeg jobs (0 selects a default).
    #[usage(long, short = 'j', default = "0")]
    jobs: usize,
    /// Output directory.
    #[usage(long, short = 'o')]
    output: Option<PathBuf>,
    /// Keep the original audio and sidecars.
    #[usage(long)]
    keep_source: bool,
    /// Replace existing output and ignore the split cache.
    #[usage(long, short = 'f')]
    force: bool,
}

#[derive(Args)]
struct Sync {
    /// Sync target name from `muzik config set-sync-target`.
    target: String,
    /// Beets query that selects the tracks to sync.
    #[usage(long, short = 'q')]
    query: Option<String>,
    /// Delete audio and cover files on the target that are not in the selection.
    #[usage(long)]
    delete: bool,
    /// Show the plan without writing files.
    #[usage(long, short = 'd')]
    dry_run: bool,
    /// Number of parallel ffmpeg jobs (0 selects a default).
    #[usage(long, short = 'j', default = "0")]
    jobs: usize,
    /// Beets-compatible library config file.
    #[usage(long, short = 'c')]
    config: Option<PathBuf>,
}

#[derive(Args)]
struct Archive {
    /// Directory with downloaded audio files.
    directory: PathBuf,
    /// Root directory for split tracks.
    #[usage(long, short = 'o', default = "./splits")]
    output: PathBuf,
    /// Import tracks into the Beets library (the default; kept for old commands).
    #[usage(long, short = 'i')]
    import: bool,
    /// Write tags without moving library files.
    #[usage(long, short = 't')]
    tag_only: bool,
    /// Show the plan without writing files.
    #[usage(long, short = 'd')]
    dry_run: bool,
    /// Skip chapter splitting.
    #[usage(long)]
    skip_split: bool,
    /// Skip library organization.
    #[usage(long)]
    skip_organize: bool,
    /// Number of parallel ffmpeg jobs per file.
    #[usage(long, short = 'j', default = "0")]
    jobs: usize,
    /// Keep original audio and sidecars after splitting.
    #[usage(long)]
    keep_source: bool,
    /// Beets-compatible library config file.
    #[usage(long, short = 'c')]
    config: Option<PathBuf>,
}

#[derive(Args)]
struct Workflow {
    /// YouTube video URL, local audio path, or search text.
    raw: String,
    /// Directory for downloaded audio.
    #[usage(long, short = 'o')]
    output: Option<PathBuf>,
    /// Directory for chapter-split tracks.
    #[usage(long)]
    splits: Option<PathBuf>,
    /// Review chapters in an editor before splitting.
    #[usage(long, short = 'r')]
    review: bool,
    /// Keep chaptered audio as one file.
    #[usage(long)]
    no_split: bool,
    /// Skip library organization.
    #[usage(long)]
    no_organize: bool,
    /// Import audio into the Beets library (the default; kept for old commands).
    #[usage(long, short = 'i')]
    import: bool,
    /// Write tags without moving library files.
    #[usage(long, short = 't')]
    tag_only: bool,
    /// Show the planned operations without writing files.
    #[usage(long, short = 'd')]
    dry_run: bool,
    /// Number of parallel ffmpeg jobs per file.
    #[usage(long, short = 'j', default = "0")]
    jobs: usize,
    /// Beets-compatible library config file.
    #[usage(long, short = 'c')]
    config: Option<PathBuf>,
    /// Keep original audio after splitting.
    #[usage(long)]
    keep_source: bool,
    /// Replace output and reprocess source files.
    #[usage(long, short = 'f')]
    force: bool,
    /// Interpret chapter titles as artist and song pairs.
    #[usage(long)]
    compilation: bool,
    /// Audio source for search and Spotify export tracks.
    #[usage(long, value_enum, default = "youtube")]
    audio_source: muzik_core::AudioSource,
    /// Source for chapter metadata.
    #[usage(long, value_enum, default = "auto")]
    metadata_source: muzik_core::MetadataSource,
    /// Check YouTube audio and select safe Soulseek replacements.
    #[usage(long, value_enum, default = "off")]
    quality_policy: muzik_core::QualityPolicy,
    /// Minimum acceptable lossy bitrate in kbps.
    #[usage(long, default = "256")]
    min_bitrate: u32,
    /// Preferred Soulseek audio quality.
    #[usage(long, default = "lossless")]
    prefer: muzik_core::PreferredAudio,
    /// Source to try if Soulseek has no result.
    #[usage(long, value_enum, default = "youtube")]
    fallback: muzik_core::AudioFallback,
    /// Select the highest-ranked Soulseek result without a prompt.
    #[usage(long)]
    no_interactive: bool,
    /// Use the shared job queue (every run uses it; kept for old commands).
    #[usage(long)]
    queue: bool,
}

#[derive(Args)]
struct Jobs {
    #[usage(subcommand)]
    command: JobsCommand,
}

#[derive(Subcommands)]
enum JobsCommand {
    /// List queued, running, and waiting jobs.
    List,
    /// Show the choices of a waiting job.
    Show(JobId),
    /// Answer a waiting job and put it back in the queue.
    Answer(JobAnswer),
    /// Remove a queued job or stop a running one.
    Cancel(JobId),
    /// Run queued jobs until the queue is empty.
    Run,
}

#[derive(Args)]
struct JobId {
    /// Job ID, such as queue-12.
    id: String,
}

#[derive(Args)]
struct JobAnswer {
    /// Job ID, such as queue-12.
    id: String,
    /// Number of the choice shown by `muzik jobs show`.
    choice: Option<usize>,
    /// Answer value as JSON, instead of a choice number.
    #[usage(long)]
    value: Option<String>,
}

#[derive(Args)]
struct Watchlist {
    #[usage(subcommand)]
    command: WatchlistCommand,
}

#[derive(Subcommands)]
enum WatchlistCommand {
    /// List watched playlists and the state of their items.
    List(WatchlistList),
    /// Add a YouTube playlist, a Spotify playlist or album, or liked.
    Add(WatchlistAdd),
    /// Remove a playlist from the watchlist.
    Remove(WatchlistRemove),
    /// Check all playlists and queue their pending items.
    Refresh(WatchlistRefresh),
    /// Queue a command for one item.
    Item(WatchlistItem),
}

#[derive(Args)]
struct WatchlistList {
    /// Show each item.
    #[usage(long, short = 'i')]
    items: bool,
}

#[derive(Args)]
struct WatchlistAdd {
    /// Playlist URL or Spotify reference.
    url: String,
}

#[derive(Args)]
struct WatchlistRemove {
    /// Playlist ID shown by `muzik watchlist list`.
    playlist_id: String,
}

#[derive(Args)]
struct WatchlistRefresh {
    /// Only add the jobs to the queue.
    #[usage(long)]
    queue_only: bool,
}

#[derive(Args)]
struct WatchlistItem {
    /// Playlist ID shown by `muzik watchlist list`.
    playlist_id: String,
    /// Item position shown by `muzik watchlist list --items`.
    position: u64,
    /// Command: run, retry, download_again, check_quality_again, parse_again, split_again, organize_again, or run_all_again.
    #[usage(long, short = 'a', default = "run")]
    action: String,
    /// Only add the job to the queue.
    #[usage(long)]
    queue_only: bool,
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
    /// Add or change a device folder for `muzik sync`.
    SetSyncTarget(SetSyncTarget),
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
struct SetSyncTarget {
    /// Target name, such as snowsky or phone.
    name: String,
    /// Device music folder.
    path: PathBuf,
    /// Audio formats for the device.
    #[usage(long, short = 'p', value_enum, default = "echo-mini")]
    preset: muzik_core::SyncPreset,
    /// Bitrate in kbps for converted files (echo-mini and mp3: 320 by default; opus: 192 by default).
    #[usage(long, short = 'b')]
    bitrate: Option<u32>,
    /// Do not copy album cover images.
    #[usage(long)]
    no_covers: bool,
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
    /// Replace existing download and metadata files.
    #[usage(long)]
    force_overwrites: bool,
}

#[derive(Args)]
struct Bandcamp {
    /// Bandcamp username (only with --cookies; muzik finds it if you leave it out).
    user: Option<String>,
    /// Folder for downloaded releases.
    #[usage(long, short = 'o')]
    output: Option<PathBuf>,
    /// Audio format.
    #[usage(long, short = 'f', value_enum, default = "flac")]
    format: muzik_bandcamp::BandcampFormat,
    /// Path to a Bandcamp cookie file. Muzik saves the login for later runs.
    #[usage(long, short = 'c')]
    cookies: Option<PathBuf>,
    /// List releases without downloading them.
    #[usage(long, short = 'd')]
    dry_run: bool,
    /// Download releases that are already in the output folder again.
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
    /// Measure library audio and suggest safe Soulseek replacements.
    CheckLibrary(SoulseekCheckLibrary),
    /// Search peer audio files and rank the results.
    Search(SoulseekSearch),
    /// Download a Soulseek result and organize its audio files.
    Download(SoulseekDownload),
}

#[derive(Args)]
struct SoulseekCheckLibrary {
    /// Beets query that limits the library scan.
    #[usage(long, short = 'q')]
    query: Option<String>,
    /// Minimum acceptable lossy bitrate in kbps.
    #[usage(long, default = "256")]
    min_bitrate: u32,
    /// Preferred replacement quality.
    #[usage(long, default = "lossless")]
    prefer: muzik_core::PreferredAudio,
    /// Maximum number of low-quality tracks to search.
    #[usage(long, short = 'n', default = "20")]
    limit: usize,
    /// Beets-compatible library config file.
    #[usage(long, short = 'c')]
    config: Option<PathBuf>,
}

#[derive(Args)]
struct SoulseekSearch {
    /// Artist, album, or track search text.
    query: String,
    /// Preferred audio quality.
    #[usage(long, default = "lossless")]
    prefer: muzik_core::PreferredAudio,
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
    #[usage(long, default = "lossless")]
    prefer: muzik_core::PreferredAudio,
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

fn run(command: Command) -> Result<(), String> {
    match command {
        Command::Archive(args) => archive::run(&args),
        Command::Bandcamp(args) => bandcamp::download(&args),
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
            ConfigCommand::SetSyncTarget(args) => return sync::set_target(&args),
        }
        .map_err(|error| error.to_string()),
        Command::Download(args) => download::run(&args),
        Command::Downloaded(args) => {
            let output = args.output.unwrap_or_else(paths::download_dir);
            downloaded::list(&output).map_err(|error| error.to_string())
        }
        Command::Init => init::run().map_err(|error| error.to_string()),
        Command::Import(args) => import::run(&args),
        Command::Jobs(args) => match args.command {
            JobsCommand::List => jobs::list(),
            JobsCommand::Show(args) => jobs::show(&args.id),
            JobsCommand::Answer(args) => jobs::answer(&args.id, args.choice, args.value.as_deref()),
            JobsCommand::Cancel(args) => jobs::cancel(&args.id),
            JobsCommand::Run => jobs::run(),
        },
        Command::Organize(args) => organize::run(&args),
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
            SoulseekCommand::CheckLibrary(args) => soulseek::check_library(&args),
            SoulseekCommand::Search(args) => {
                soulseek::search(&args.query, args.prefer, args.limit, args.json)
            }
            SoulseekCommand::Download(args) => soulseek::download(&args),
        },
        Command::Split(args) => split::run(&args).map(|_| ()),
        Command::Sync(args) => sync::run(&args),
        Command::Validate(args) => validate::run(&args),
        Command::Watchlist(args) => match args.command {
            WatchlistCommand::List(args) => watchlist::list(args.items),
            WatchlistCommand::Add(args) => watchlist::add(&args.url),
            WatchlistCommand::Remove(args) => watchlist::remove(&args.playlist_id),
            WatchlistCommand::Refresh(args) => watchlist::refresh(args.queue_only),
            WatchlistCommand::Item(args) => watchlist::item(
                &args.playlist_id,
                args.position,
                &args.action,
                args.queue_only,
            ),
        },
        Command::Workflow(args) => workflow::run(&args),
    }
}

fn main() -> std::process::ExitCode {
    if let Err(error) = run(Muzik::parse().command) {
        eprintln!("error: {error}");
        std::process::ExitCode::FAILURE
    } else {
        std::process::ExitCode::SUCCESS
    }
}
