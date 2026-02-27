"""Central configuration — paths, constants, defaults."""

from pathlib import Path

# Cache directory shared with bash scripts
CACHE_DIR = Path.home() / ".cache" / "music-scripts"

# Supported audio file extensions
AUDIO_EXTENSIONS = {".flac", ".mp3", ".m4a", ".opus", ".wav", ".aac"}

# Default beets config location
BEETS_CONFIG = Path.home() / ".config" / "beets" / "config.yaml"

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
