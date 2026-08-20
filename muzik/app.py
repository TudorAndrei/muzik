"""Root Typer application — registers all sub-commands."""

import typer

from muzik.commands import cache as cache_mod
from muzik.commands import config as config_mod
from muzik.commands.bandcamp import bandcamp_cmd
from muzik.commands.download import download_cmd
from muzik.commands.downloaded import downloaded_cmd
from muzik.commands.import_ import import_cmd
from muzik.commands.init import init_cmd
from muzik.commands.split import split_cmd
from muzik.commands.organize import organize_cmd
from muzik.commands import soulseek as soulseek_mod
from muzik.commands.workflow import workflow_cmd
from muzik.commands.archive import archive_cmd
from muzik.commands.validate import validate_cmd
from muzik.commands.desktop import install_app_cmd
from muzik.gui.app import gui_cmd

app = typer.Typer(
    name="muzik",
    help=(
        "Music organizer CLI — acquire, split, tag, and organize music.\n\n"
        "Supports YouTube, Soulseek, Bandcamp, local audio, and metadata-only "
        "Spotify playlist exports. Wraps yt-dlp, slskd, ffmpeg, and beets."
    ),
    add_completion=False,
    no_args_is_help=True,
)

# Single-command subcommands registered directly on the root app
app.command("init", help="Create app directories and configure beets.")(init_cmd)
app.command("import", help="Import an existing music library into beets.")(import_cmd)
app.command("bandcamp", help="Download a Bandcamp collection and organize with beets.")(
    bandcamp_cmd
)
app.command("download", help="Download audio from YouTube via yt-dlp.")(download_cmd)
app.command("downloaded", help="List audio already in the output folder.")(
    downloaded_cmd
)
app.command("split", help="Split audio file by chapters (with optional --review).")(
    split_cmd
)
app.command("organize", help="Tag/import audio with beets.")(organize_cmd)
app.command("workflow", help="Full pipeline: acquire → split → organize.")(workflow_cmd)
app.command("archive", help="Process existing downloaded files (split + organize).")(
    archive_cmd
)
app.command("validate", help="Validate audio files, chapters, and metadata.")(
    validate_cmd
)
app.command("gui", help="Open the DearPyGui workflow UI.")(gui_cmd)
app.command("install-app", help="Install a macOS app bundle into Applications.")(
    install_app_cmd
)
# Multi-command subcommand groups
app.add_typer(cache_mod.app, name="cache")
app.add_typer(config_mod.app, name="config")
app.add_typer(soulseek_mod.app, name="soulseek")
