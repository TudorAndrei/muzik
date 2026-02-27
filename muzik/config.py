"""Central configuration — paths, constants, defaults.

Directory locations follow the XDG Base Directory Specification
(https://specifications.freedesktop.org/basedir/latest/).  Each XDG variable
is read from the environment; if unset, empty, or not absolute the spec-
mandated default is used instead.
"""

import os
from pathlib import Path


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

# Cache directory shared with the bash scripts (yt_<id>.txt, split_<hash>.txt …)
CACHE_DIR = XDG_CACHE_HOME / "music-scripts"

# muzik-specific cache dir ($XDG_CACHE_HOME/muzik/)
MUZIK_CACHE_DIR = XDG_CACHE_HOME / "muzik"

# Bandcamp download-tracking cache (pipe-delimited, one entry per purchased item)
BANDCAMP_CACHE_FILE = MUZIK_CACHE_DIR / "bandcamp.cache"

# Default beets config location
BEETS_CONFIG = XDG_CONFIG_HOME / "beets" / "config.yaml"

# muzik config dir — stores per-service credentials (e.g. Bandcamp cookies)
MUZIK_CONFIG_DIR = XDG_CONFIG_HOME / "muzik"

# Default directories for downloaded audio and chapter-split tracks.
# Both live under $XDG_DATA_HOME/muzik/ so they are:
#   • persistent across runs (not in cache)
#   • out of the way of the working directory
#   • easy to locate on any XDG-compliant system
DEFAULT_DOWNLOAD_DIR = XDG_DATA_HOME / "muzik" / "downloads"
DEFAULT_BANDCAMP_DIR = XDG_DATA_HOME / "muzik" / "bandcamp"
DEFAULT_SPLITS_DIR = XDG_DATA_HOME / "muzik" / "splits"

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
    "--audio-format",
    "flac",
    "--audio-quality",
    "0",
    "--embed-metadata",
    "--add-metadata",
    "--write-info-json",
    "--embed-chapters",
    "--no-playlist-reverse",
]
