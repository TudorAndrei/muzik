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


def get_slskd_settings(
    *,
    env: Mapping[str, str] = os.environ,
    config_path: Path = MUZIK_CONFIG_FILE,
) -> dict[str, str]:
    """Return slskd settings from environment, muzik config, then defaults."""
    config = load_muzik_config(config_path)
    return {
        "url": _env_or_config(
            env,
            "SLSKD_URL",
            config,
            "slskd",
            "url",
            "http://localhost:5030",
        ).rstrip("/"),
        "api_key": _env_or_config(
            env,
            "SLSKD_API_KEY",
            config,
            "slskd",
            "api_key",
            "",
        ),
        "download_dir": _env_or_config(
            env,
            "SLSKD_DOWNLOAD_DIR",
            config,
            "slskd",
            "download_dir",
            str(DEFAULT_SOULSEEK_DIR),
        ),
    }


# slskd/Soulseek backend settings. Env vars override muzik's config file.
_SLSKD_SETTINGS = get_slskd_settings()
SLSKD_URL = _SLSKD_SETTINGS["url"]
SLSKD_API_KEY = _SLSKD_SETTINGS["api_key"]
SLSKD_DOWNLOAD_DIR = _SLSKD_SETTINGS["download_dir"]

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
