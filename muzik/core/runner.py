"""Subprocess helpers for external tools.

Three strategies:
- run_streaming()   — yt-dlp: Popen + thread reader + Rich Live (last-line display)
- run_passthrough() — beet import: inherit stdin/stdout so interactive prompts work
- run_silent()      — ffprobe queries: capture all output, return CompletedProcess
"""

import subprocess
import threading
from pathlib import Path
from typing import Optional

from rich.live import Live
from rich.text import Text

from muzik.ui.console import console


# ---------------------------------------------------------------------------
# Internal: mutable renderable for Live display
# ---------------------------------------------------------------------------


class _LastLine:
    """Rich renderable that always shows the latest output line."""

    def __init__(self) -> None:
        self.text = ""

    def __rich__(self) -> Text:
        return Text(self.text[:200], overflow="fold", style="dim")


# ---------------------------------------------------------------------------
# Public runners
# ---------------------------------------------------------------------------


def run_streaming(
    cmd: list[str],
    cwd: Optional[Path] = None,
    label: str = "",
) -> int:
    """Run *cmd*, showing the last output line via Rich Live.

    stdout and stderr are merged so yt-dlp progress (on stderr) is captured.
    Handles carriage-return progress lines from yt-dlp correctly.
    Returns the process exit code.
    """
    last = _LastLine()

    try:
        proc = subprocess.Popen(
            cmd,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            cwd=cwd,
            bufsize=0,
        )
    except FileNotFoundError:
        from muzik.ui.console import err

        err(
            f"[red]Command not found:[/red] [bold]{cmd[0]}[/bold] — is it installed and on PATH?"
        )
        return 127

    def _reader() -> None:
        assert proc.stdout is not None
        buf = b""
        while True:
            chunk = proc.stdout.read(512)
            if not chunk:
                break
            buf += chunk
            # Split on \r and \n; show the last non-empty segment
            parts = buf.replace(b"\r", b"\n").split(b"\n")
            buf = parts[-1]  # keep incomplete last part for next iteration
            for part in reversed(parts[:-1]):
                text = part.decode("utf-8", errors="replace").strip()
                if text:
                    last.text = text
                    break

    t = threading.Thread(target=_reader, daemon=True)
    t.start()

    with Live(last, console=console, refresh_per_second=10, transient=True):
        proc.wait()
        t.join()

    return proc.returncode


def run_passthrough(
    cmd: list[str],
    cwd: Optional[Path] = None,
) -> int:
    """Run *cmd* with stdin/stdout/stderr fully inherited.

    Use this for beet import so interactive prompts reach the terminal.
    Returns the process exit code.
    """
    try:
        result = subprocess.run(cmd, cwd=cwd)
        return result.returncode
    except FileNotFoundError:
        from muzik.ui.console import err

        err(
            f"[red]Command not found:[/red] [bold]{cmd[0]}[/bold] — is it installed and on PATH?"
        )
        return 127


def run_silent(
    cmd: list[str],
    cwd: Optional[Path] = None,
) -> subprocess.CompletedProcess:
    """Run *cmd*, capturing all output. Raises nothing.

    Use this for ffprobe queries where you want the raw stdout/stderr.
    Returns the CompletedProcess (check .returncode, .stdout, .stderr).
    """
    return subprocess.run(
        cmd,
        capture_output=True,
        text=True,
        cwd=cwd,
    )
