"""Central configuration — paths, constants, defaults.

Directory locations follow the XDG Base Directory Specification
(https://specifications.freedesktop.org/basedir/latest/).  Each XDG variable
is read from the environment; if unset, empty, or not absolute the spec-
mandated default is used instead.
"""

import os
from pathlib import Path
from typing import Mapping

import yaml


# ---------------------------------------------------------------------------
# XDG helpers
# ---------------------------------------------------------------------------


def _xdg(env_var: str, default: Path) -> Path:
    """Return the XDG directory for *env_var*, falling back to *default*.

    The spec requires the path to be absolute; relative or empty values are
    treated as unset.
    """
    raw = os.environ.get(env_var, "").strip()
    p = Path(raw) if raw else None
    return p if (p and p.is_absolute()) else default


XDG_CACHE_HOME = _xdg("XDG_CACHE_HOME", Path.home() / ".cache")
XDG_CONFIG_HOME = _xdg("XDG_CONFIG_HOME", Path.home() / ".config")
XDG_DATA_HOME = _xdg("XDG_DATA_HOME", Path.home() / ".local" / "share")


# ---------------------------------------------------------------------------
# Application paths
# ---------------------------------------------------------------------------

# Cache directory ($XDG_CACHE_HOME/muzik/)
CACHE_DIR = XDG_CACHE_HOME / "muzik"

# Bandcamp download-tracking cache (pipe-delimited, one entry per purchased item)
BANDCAMP_CACHE_FILE = CACHE_DIR / "bandcamp.cache"

# Default beets config location
BEETS_CONFIG = XDG_CONFIG_HOME / "beets" / "config.yaml"

# muzik config dir — stores per-service credentials (e.g. Bandcamp cookies)
MUZIK_CONFIG_DIR = XDG_CONFIG_HOME / "muzik"
MUZIK_CONFIG_FILE = MUZIK_CONFIG_DIR / "config.yaml"

# Default directories for downloaded audio and chapter-split tracks.
# Both live under $XDG_DATA_HOME/muzik/ so they are:
#   • persistent across runs (not in cache)
#   • out of the way of the working directory
#   • easy to locate on any XDG-compliant system
DEFAULT_DOWNLOAD_DIR = XDG_DATA_HOME / "muzik" / "downloads"
DEFAULT_BANDCAMP_DIR = XDG_DATA_HOME / "muzik" / "bandcamp"
DEFAULT_SOULSEEK_DIR = XDG_DATA_HOME / "muzik" / "soulseek"
DEFAULT_SPLITS_DIR = XDG_DATA_HOME / "muzik" / "splits"


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


# slskd/Soulseek backend settings. Env vars override $XDG_CONFIG_HOME/muzik/config.yaml.
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
