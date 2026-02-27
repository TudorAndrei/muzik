"""Root Typer application — registers all sub-commands."""

import typer

from muzik.commands import cache as cache_mod
from muzik.commands import config as config_mod
from muzik.commands.bandcamp import bandcamp_cmd
from muzik.commands.download import download_cmd
from muzik.commands.import_ import import_cmd
from muzik.commands.init import init_cmd
from muzik.commands.split import split_cmd
from muzik.commands.organize import organize_cmd
from muzik.commands.workflow import workflow_cmd
from muzik.commands.archive import archive_cmd
from muzik.commands.validate import validate_cmd

app = typer.Typer(
    name="muzik",
    help=(
        "Music organizer CLI — download, split, and organize music from YouTube.\n\n"
        "Wraps yt-dlp, ffmpeg, and beets with better progress feedback and "
        "an interactive chapter editor."
    ),
    add_completion=False,
    no_args_is_help=True,
)

# Single-command subcommands registered directly on the root app
app.command("init", help="Create XDG directories and configure beets.")(init_cmd)
app.command("import", help="Import an existing music library into beets.")(import_cmd)
app.command("bandcamp", help="Download Bandcamp collection via bandsnatch + organize with beets.")(bandcamp_cmd)
app.command("download", help="Download audio from YouTube via yt-dlp.")(download_cmd)
app.command("split", help="Split audio file by chapters (with optional --review).")(
    split_cmd
)
app.command("organize", help="Tag/import audio with beets.")(organize_cmd)
app.command("workflow", help="Full pipeline: download → split → organize.")(
    workflow_cmd
)
app.command("archive", help="Process existing downloaded files (split + organize).")(
    archive_cmd
)
app.command("validate", help="Validate audio files, chapters, and metadata.")(
    validate_cmd
)

# Multi-command subcommand groups
app.add_typer(cache_mod.app, name="cache")
app.add_typer(config_mod.app, name="config")
