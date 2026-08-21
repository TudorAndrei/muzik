"""Extract an album tracklist from a YouTube description.

Two stages, cheapest first:

1. A free regex parser that reads common tracklist lines
   (``0:00 - Twilight``, ``[0:00] Twilight``, ``1. Twilight 0:00`` ...).
2. A structured LLM agent, used only when the regex finds nothing. It reuses the
   configured ``MUZIK_TAG_BACKEND`` (openrouter / codex / opencode) and returns a
   validated ``TrackList``, so a messy description still yields real track names.
"""

from __future__ import annotations

from collections.abc import Callable
import json
import os
import re
import subprocess
from typing import cast

from pydantic import BaseModel, ValidationError

from muzik.core.beets.agent_decisions import (
    DEFAULT_BACKEND,
    DEFAULT_CODEX_MODEL,
    DEFAULT_OPENCODE_MODEL,
    DEFAULT_OPENROUTER_MODEL,
    _run_cli,
    extract_json_object,
)
from muzik.core.chapters import Chapter, _ts_to_secs


Logger = Callable[[str], None]

_TS = r"\d{1,2}:\d{2}(?::\d{2})?"
# Timestamp at the start of the line, optional "N." index and separators.
_LINE_START = re.compile(
    rf"^\s*(?:\d{{1,3}}[.)\-]\s*)?[\[(]?({_TS})[\])]?\s*[-–—•|:.)\]]*\s*(.+?)\s*$"
)
# Timestamp at the end of the line: "Title ... 0:00".
_LINE_END = re.compile(
    rf"^\s*(?:\d{{1,3}}[.)\-]\s*)?(.+?)\s*[-–—•|(\[]*\s*[\[(]?({_TS})[\])]?\s*$"
)
_ONLY_TS = re.compile(rf"^\s*[\[(]?{_TS}[\])]?\s*$")


class TrackEntry(BaseModel):
    """One track: a timestamp and a title."""

    time: str
    title: str


class TrackList(BaseModel):
    """A validated album tracklist."""

    tracks: list[TrackEntry]


_SYSTEM = (
    "You extract an album tracklist from a YouTube video description. "
    "Return only tracks that have an explicit timestamp. Keep the exact track "
    "titles; do not invent tracks."
)


def chapters_from_description(
    description: str, *, log: Logger | None = None
) -> list[Chapter] | None:
    """Return chapters parsed from a description, or None when there are none."""
    logger = log or (lambda _message: None)
    chapters = _regex_tracklist(description)
    if chapters:
        return chapters
    return _agent_tracklist(description, logger)


_TIMESTAMP_LINE = re.compile(rf"(?m)^\s*.*?{_TS}.*$")
_COMMENT_TIMEOUT = 120


def chapters_from_comments(
    video_id: str, *, log: Logger | None = None
) -> list[Chapter] | None:
    """Return a tracklist from the video's pinned/uploader comment, if any.

    Many album uploads leave the description empty and put the tracklist in a
    pinned comment. Fetch the top comments, pick the pinned or uploader one that
    has timestamps, and parse it like a description.
    """
    logger = log or (lambda _message: None)
    text = _fetch_tracklist_comment(video_id, logger)
    if not text:
        return None
    logger("found a timestamped tracklist in a comment")
    return chapters_from_description(text, log=logger)


def _fetch_tracklist_comment(video_id: str, log: Logger) -> str | None:
    from muzik.core.sources.youtube import cookie_args, js_runtime_args

    url = f"https://www.youtube.com/watch?v={video_id}"
    cmd = [
        "yt-dlp",
        *cookie_args(),
        *js_runtime_args(),
        "--dump-json",
        "--skip-download",
        "--write-comments",
        # Only the top ~50 top-level comments, no replies, so it stays quick.
        "--extractor-args",
        "youtube:max_comments=50,all,0,0;comment_sort=top",
        url,
    ]
    try:
        result = subprocess.run(
            cmd, capture_output=True, text=True, timeout=_COMMENT_TIMEOUT
        )
    except (FileNotFoundError, subprocess.TimeoutExpired) as exc:
        log(f"comment fetch failed: {exc}")
        return None
    if result.returncode != 0 or not result.stdout.strip():
        return None
    try:
        data = json.loads(result.stdout)
    except json.JSONDecodeError:
        return None
    return _best_tracklist_comment(data.get("comments") or [])


def _best_tracklist_comment(comments: list[dict]) -> str | None:
    best: str | None = None
    best_rank = -1
    for comment in comments:
        text = comment.get("text") or ""
        if len(_TIMESTAMP_LINE.findall(text)) < 2:
            continue
        rank = (2 if comment.get("is_pinned") else 0) + (
            1 if comment.get("author_is_uploader") else 0
        )
        if rank > best_rank:
            best = text
            best_rank = rank
    return best


# -- regex stage ----------------------------------------------------------


def _clean_title(title: str) -> str:
    # Strip only separator punctuation and space; keep brackets that belong to
    # the title (e.g. "Mind Drifting (w/B.Young)").
    return title.strip().strip("-–—•|:. \t").strip()


def _regex_tracklist(description: str) -> list[Chapter] | None:
    entries: list[tuple[int, str]] = []
    for line in description.splitlines():
        if not line.strip():
            continue
        parsed = _parse_line(line)
        if parsed is not None:
            entries.append(parsed)
    return _to_chapters(entries)


def _parse_line(line: str) -> tuple[int, str] | None:
    match = _LINE_START.match(line)
    if match:
        title = _clean_title(match.group(2))
        if title and not _ONLY_TS.match(title):
            return _ts_to_secs(match.group(1)), title
    match = _LINE_END.match(line)
    if match:
        title = _clean_title(match.group(1))
        if title and not _ONLY_TS.match(title):
            return _ts_to_secs(match.group(2)), title
    return None


def _to_chapters(entries: list[tuple[int, str]]) -> list[Chapter] | None:
    seen: set[int] = set()
    unique: list[tuple[int, str]] = []
    for secs, title in sorted(entries, key=lambda item: item[0]):
        if secs in seen:
            continue
        seen.add(secs)
        unique.append((secs, title))
    if len(unique) < 2:
        return None
    chapters: list[Chapter] = []
    for index, (secs, title) in enumerate(unique):
        end = unique[index + 1][0] if index + 1 < len(unique) else None
        chapters.append(Chapter(index=index + 1, start=secs, end=end, title=title))
    return chapters


# -- agent stage ----------------------------------------------------------


def _tracklist_to_chapters(tracklist: TrackList) -> list[Chapter] | None:
    entries: list[tuple[int, str]] = []
    for entry in tracklist.tracks:
        title = entry.title.strip()
        if not title:
            continue
        try:
            entries.append((_ts_to_secs(entry.time), title))
        except ValueError, AttributeError:
            continue
    return _to_chapters(entries)


def _agent_prompt(description: str) -> str:
    return (
        _SYSTEM
        + "\n\nDescription:\n"
        + description
        + "\n\nRespond with ONLY a JSON object, no prose or code fences:\n"
        + '{"tracks": [{"time": "M:SS", "title": "..."}, ...]}'
    )


def _agent_tracklist(description: str, log: Logger) -> list[Chapter] | None:
    backend = os.environ.get("MUZIK_TAG_BACKEND", DEFAULT_BACKEND)
    model = os.environ.get("MUZIK_TAG_MODEL")
    try:
        if backend == "codex":
            tracklist = _cli_tracklist(
                [
                    "codex",
                    "exec",
                    "--skip-git-repo-check",
                    "--model",
                    model or DEFAULT_CODEX_MODEL,
                ],
                description,
                stdin=True,
                log=log,
            )
        elif backend == "opencode":
            tracklist = _cli_tracklist(
                ["opencode", "run", "--model", model or DEFAULT_OPENCODE_MODEL],
                description,
                stdin=False,
                log=log,
            )
        else:
            tracklist = _openrouter_tracklist(
                model or DEFAULT_OPENROUTER_MODEL, description
            )
    except Exception as exc:  # noqa: BLE001 - a fallback must never crash the split
        log(f"tracklist agent error: {exc}")
        return None
    if tracklist is None:
        return None
    return _tracklist_to_chapters(tracklist)


def _cli_tracklist(
    cmd: list[str], description: str, *, stdin: bool, log: Logger
) -> TrackList | None:
    prompt = _agent_prompt(description)
    full = cmd if stdin else [*cmd, prompt]
    output = _run_cli(full, prompt if stdin else None, log)
    if not output:
        return None
    obj = extract_json_object(output, "tracks")
    if obj is None:
        return None
    try:
        return TrackList.model_validate(obj)
    except ValidationError as exc:
        log(f"invalid tracklist JSON: {exc}")
        return None


def _openrouter_tracklist(model_name: str, description: str) -> TrackList | None:
    api_key = os.environ.get("OPENROUTER_API_KEY")
    if not api_key:
        return None
    from pydantic_ai import Agent
    from pydantic_ai.models.openrouter import OpenRouterModel
    from pydantic_ai.providers.openrouter import OpenRouterProvider

    model = OpenRouterModel(model_name, provider=OpenRouterProvider(api_key=api_key))
    agent = Agent(model, output_type=TrackList, system_prompt=_SYSTEM)
    result = agent.run_sync(
        f"Extract the tracklist from this description:\n\n{description}"
    )
    return cast(TrackList, result.output)
