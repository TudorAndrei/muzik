"""Extract a chapter/track list from a YouTube video description using an LLM.

Uses PydanticAI with an OpenRouter-hosted model.
Requires the ``OPENROUTER_API_KEY`` environment variable.
"""

import asyncio
import json
import os
import re
from pathlib import Path
from typing import Optional

from pydantic_ai import Agent
from pydantic_ai.models.openrouter import OpenRouterModel
from pydantic_ai.providers.openrouter import OpenRouterProvider

from muzik.core.chapters import Chapter, _ts_to_secs


_SYSTEM_PROMPT = """\
You are a music metadata extractor.
Given a YouTube video description, extract the track listing.
Output ONLY the tracks, one per line, in this exact format:
HH:MM:SS Track title
or
MM:SS Track title

Rules:
- Only include tracks that have explicit timestamps in the description.
- Do not add any other text, headers, or explanation.
- If there are no timestamps, output nothing.
"""

# Matches lines like "00:04:32 Angels" or "4:32 Angels"
_LINE_RE = re.compile(r"^(\d{1,2}:\d{2}(?::\d{2})?)\s+(.+)$")


# ---------------------------------------------------------------------------
# LLM extraction
# ---------------------------------------------------------------------------


def _make_agent() -> Optional[Agent]:
    api_key = os.environ.get("OPENROUTER_API_KEY")
    if not api_key:
        return None
    model = OpenRouterModel(
        "z-ai/glm-4.5-air:free",
        provider=OpenRouterProvider(api_key=api_key),
    )
    return Agent(model, system_prompt=_SYSTEM_PROMPT)


async def _extract_async(description: str) -> str:
    agent = _make_agent()
    if agent is None:
        return ""
    result = await agent.run(
        f"Extract the track listing from this video description:\n\n{description}"
    )
    return result.output


def description_has_timestamps(description: str) -> bool:
    """Return True if the description contains at least one timestamp line."""
    return any(_LINE_RE.search(line) for line in description.splitlines())


def extract_chapters_from_description(
    description: str,
) -> tuple[Optional[list[Chapter]], Optional[str]]:
    """Parse chapters from a video description using an LLM.

    Returns ``(chapters, None)`` on success, ``(None, error_message)`` on failure.
    """
    try:
        text = asyncio.run(_extract_async(description))
    except Exception as exc:
        return None, str(exc)

    if not text:
        return None, None

    raw: list[tuple[int, str]] = []
    for line in text.splitlines():
        m = _LINE_RE.match(line.strip())
        if m:
            try:
                raw.append((_ts_to_secs(m.group(1)), m.group(2).strip()))
            except ValueError:
                continue

    if not raw:
        return None, None

    chapters: list[Chapter] = []
    for idx, (start, title) in enumerate(raw):
        end = raw[idx + 1][0] if idx + 1 < len(raw) else None
        chapters.append(Chapter(index=idx + 1, start=start, end=end, title=title))
    return chapters, None


def get_description_from_info_json(info_json_path: Path) -> Optional[str]:
    """Read the ``description`` field from a yt-dlp ``.info.json`` file."""
    try:
        data = json.loads(info_json_path.read_text(encoding="utf-8", errors="replace"))
        return data.get("description") or None
    except Exception:
        return None
