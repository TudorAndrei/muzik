"""Shared Rich console — all status/progress output goes to stderr."""

from rich.console import Console

# Single shared console instance (stderr so piped stdout stays clean)
console = Console(stderr=True)


def err(msg: str) -> None:
    """Print an error/status message to the shared console."""
    console.print(msg)
