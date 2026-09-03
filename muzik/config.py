"""Central configuration: paths, constants, defaults.

Directory locations are resolved with platformdirs. On Linux this follows the
XDG Base Directory Specification, while macOS and Windows use their native
per-user application directories.
"""

import os
from pathlib import Path
from typing import Mapping

from beets import config as beets_config
from platformdirs import PlatformDirs
import yaml


# ---------------------------------------------------------------------------
# Application paths
# ---------------------------------------------------------------------------

_APP_DIRS = PlatformDirs("muzik", appauthor=False)
CACHE_DIR = _APP_DIRS.user_cache_path

# Bandcamp download-tracking cache (pipe-delimited, one entry per purchased item)
BANDCAMP_CACHE_FILE = CACHE_DIR / "bandcamp.cache"

# Default beets config location. Use beets' own helper so muzik matches beet.
BEETS_CONFIG = Path(beets_config.user_config_path())

# muzik config dir — stores per-service credentials (e.g. Bandcamp cookies)
MUZIK_CONFIG_DIR = _APP_DIRS.user_config_path
MUZIK_CONFIG_FILE = MUZIK_CONFIG_DIR / "config.yaml"
MUZIK_WATCHLIST_FILE = MUZIK_CONFIG_DIR / "watchlist.json"

# Default directories for downloaded audio and chapter-split tracks.
# These live under the platform-specific user data directory so they are:
#   • persistent across runs (not in cache)
#   • out of the way of the working directory
#   • easy to locate on each supported OS
_DATA_DIR = _APP_DIRS.user_data_path
DEFAULT_DOWNLOAD_DIR = _DATA_DIR / "downloads"
DEFAULT_BANDCAMP_DIR = _DATA_DIR / "bandcamp"
DEFAULT_SOULSEEK_DIR = _DATA_DIR / "soulseek"
DEFAULT_SPLITS_DIR = _DATA_DIR / "splits"


def load_muzik_config(path: Path = MUZIK_CONFIG_FILE) -> dict:
    """Load muzik's own config file, returning an empty dict when absent."""
    if not path.exists():
        return {}
    try:
        data = yaml.safe_load(path.read_text(encoding="utf-8")) or {}
    except yaml.YAMLError:
        return {}
    return data if isinstance(data, dict) else {}


def _env_or_config(
    env: Mapping[str, str],
    env_name: str,
    config: dict,
    section: str,
    key: str,
    default: str,
) -> str:
    raw = env.get(env_name, "").strip()
    if raw:
        return raw
    section_data = config.get(section) or {}
    if isinstance(section_data, dict):
        value = section_data.get(key)
        if value is not None and str(value).strip():
            return str(value).strip()
    return default


def get_seakarr_settings(
    *,
    env: Mapping[str, str] = os.environ,
    config_path: Path = MUZIK_CONFIG_FILE,
) -> dict[str, str]:
    """Return Seakarr/Soulseek settings from environment, muzik config, then defaults."""
    config = load_muzik_config(config_path)
    return {
        "username": _env_or_config(
            env, "MUZIK_SOULSEEK_USERNAME", config, "soulseek", "username", ""
        ),
        "password": _env_or_config(
            env, "MUZIK_SOULSEEK_PASSWORD", config, "soulseek", "password", ""
        ),
        "server_host": _env_or_config(
            env,
            "MUZIK_SOULSEEK_SERVER_HOST",
            config,
            "soulseek",
            "server_host",
            "server.slsknet.org",
        ),
        "server_port": _env_or_config(
            env, "MUZIK_SOULSEEK_SERVER_PORT", config, "soulseek", "server_port", "2416"
        ),
        "listen_port": _env_or_config(
            env, "MUZIK_SOULSEEK_LISTEN_PORT", config, "soulseek", "listen_port", "2234"
        ),
        "search_limit": _env_or_config(
            env, "MUZIK_SOULSEEK_SEARCH_LIMIT", config, "soulseek", "search_limit", "20"
        ),
        "search_timeout": _env_or_config(
            env,
            "MUZIK_SOULSEEK_SEARCH_TIMEOUT",
            config,
            "soulseek",
            "search_timeout",
            "15",
        ),
        "download_timeout": _env_or_config(
            env,
            "MUZIK_SOULSEEK_DOWNLOAD_TIMEOUT",
            config,
            "soulseek",
            "download_timeout",
            "600",
        ),
        "download_dir": _env_or_config(
            env,
            "MUZIK_SOULSEEK_DOWNLOAD_DIR",
            config,
            "soulseek",
            "download_dir",
            str(DEFAULT_SOULSEEK_DIR),
        ),
    }


# Seakarr/Soulseek backend settings. Env vars override muzik's config file.
# Never print SEAKARR_PASSWORD — status output and error messages must not
# show it.
_SEAKARR_SETTINGS = get_seakarr_settings()
SEAKARR_USERNAME = _SEAKARR_SETTINGS["username"]
SEAKARR_PASSWORD = _SEAKARR_SETTINGS["password"]
SEAKARR_SERVER_HOST = _SEAKARR_SETTINGS["server_host"]
SEAKARR_SERVER_PORT = int(_SEAKARR_SETTINGS["server_port"])
SEAKARR_LISTEN_PORT = int(_SEAKARR_SETTINGS["listen_port"])
SEAKARR_SEARCH_LIMIT = int(_SEAKARR_SETTINGS["search_limit"])
SEAKARR_SEARCH_TIMEOUT = float(_SEAKARR_SETTINGS["search_timeout"])
SEAKARR_DOWNLOAD_TIMEOUT = float(_SEAKARR_SETTINGS["download_timeout"])
SEAKARR_DOWNLOAD_DIR = _SEAKARR_SETTINGS["download_dir"]

# ---------------------------------------------------------------------------
# Constants
# ---------------------------------------------------------------------------

# Supported audio file extensions
AUDIO_EXTENSIONS = {".flac", ".mp3", ".m4a", ".opus", ".wav", ".aac"}

# yt-dlp output template — embeds YouTube ID so bash cache keys stay compatible
YTDLP_OUTPUT_TEMPLATE = "%(title)s [%(id)s].%(ext)s"

# yt-dlp base download flags
YTDLP_FLAGS = [
    "--format",
    "bestaudio",
    "--extract-audio",
    "--audio-quality",
    "0",
    "--embed-metadata",
    "--add-metadata",
    "--write-info-json",
    "--embed-chapters",
    "--no-playlist-reverse",
]
