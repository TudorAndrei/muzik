"""LLM-backed beets import decisions.

An agent picks among the candidate releases beets already found for an album.
It never invents tags: it only chooses a listed candidate, keeps the files
as-is, or skips them. Strong matches are auto-applied without any LLM call.

Policy: auto-apply confident matches, skip the uncertain ones for manual
review. Requires ``OPENROUTER_API_KEY``; without it, only the strong-match
fast path applies and everything else is skipped.
"""

from __future__ import annotations

from collections.abc import Callable
import os
from pathlib import Path
from typing import Any, Literal, cast

from pydantic import BaseModel

from muzik.core.beets.decisions import (
    BeetsDuplicateDecision,
    BeetsMatchDecision,
)
from muzik.core.beets.views import BeetsDuplicateView, BeetsMatchView, BeetsTaskView


DEFAULT_MODEL = "z-ai/glm-5.2:free"

# Distance is 0 (perfect) .. 1 (poor). At or below this, apply without the LLM.
STRONG_DISTANCE = 0.10
# The LLM must be at least this sure before we apply its pick.
MIN_CONFIDENCE = 0.65

_SYSTEM_PROMPT = """\
You help a music tagging tool choose the correct release for one album folder.
You receive the source folder name, its track filenames, and a numbered list of
candidate releases. Each candidate has a distance from 0.0 (perfect match) to
1.0 (poor match).

Choose one action:
- "pick" with candidate_index: one candidate clearly matches this album.
- "as_is": no candidate matches, but the existing folder and filenames already
  look correctly tagged.
- "skip": you are unsure, or the tags are messy and no candidate fits.

Only pick a candidate from the list. Never invent an artist, album, or index.
Give a confidence from 0.0 to 1.0 and a one-line reason.
"""


class MatchDecision(BaseModel):
    """Structured decision returned by the agent."""

    action: Literal["pick", "as_is", "skip"]
    candidate_index: int | None = None
    confidence: float = 0.0
    reason: str = ""


# A chooser turns a task view into a decision, or None when unavailable.
Chooser = Callable[[BeetsTaskView], MatchDecision | None]
Logger = Callable[[str], None]


class AgentBeetsDecisions:
    """Beets decisions driven by an LLM, with a strong-match fast path."""

    def __init__(
        self,
        *,
        model_name: str | None = None,
        strong_distance: float = STRONG_DISTANCE,
        min_confidence: float = MIN_CONFIDENCE,
        chooser: Chooser | None = None,
        log: Logger | None = None,
        duplicate_decision: BeetsDuplicateDecision = BeetsDuplicateDecision.SKIP,
    ) -> None:
        self.model_name = model_name or os.environ.get("MUZIK_TAG_MODEL", DEFAULT_MODEL)
        self.strong_distance = strong_distance
        self.min_confidence = min_confidence
        self._chooser = chooser
        self._log = log or (lambda _message: None)
        self.duplicate_decision = duplicate_decision

    # -- BeetsDecisions protocol ------------------------------------------

    def should_resume_beets_import(self, path: Path) -> bool:
        return False

    def choose_beets_album_match(self, task: BeetsTaskView) -> Any:
        return self._decide(task, kind="album")

    def choose_beets_track_match(self, task: BeetsTaskView) -> Any:
        return self._decide(task, kind="track")

    def resolve_beets_duplicate(
        self,
        task: BeetsTaskView,
        duplicates: list[BeetsDuplicateView],
    ) -> BeetsDuplicateDecision:
        return self.duplicate_decision

    # -- decision logic ---------------------------------------------------

    def _decide(self, task: BeetsTaskView, *, kind: str) -> Any:
        label = _source_label(task)
        matches = task.matches
        if not matches:
            self._log(f"skip: no candidates for {label}")
            return None  # -> beets SKIP

        best = min(matches, key=_distance_key)
        if best.distance is not None and best.distance <= self.strong_distance:
            self._log(
                f"apply: strong match d={best.distance:.3f} "
                f"{_match_label(best)} for {label}"
            )
            return best.candidate_id

        decision = self._choose(task)
        if decision is None:
            self._log(f"skip: no agent available for {label}")
            return None

        return self._resolve(decision, matches, label)

    def _resolve(
        self,
        decision: MatchDecision,
        matches: list[BeetsMatchView],
        label: str,
    ) -> Any:
        if decision.action == "pick":
            index = decision.candidate_index
            if index is None or not (0 <= index < len(matches)):
                self._log(f"skip: agent picked an invalid index for {label}")
                return None
            if decision.confidence < self.min_confidence:
                self._log(
                    f"skip: low confidence {decision.confidence:.2f} for {label} "
                    f"({decision.reason})"
                )
                return None
            chosen = matches[index]
            self._log(
                f"apply: agent picked {_match_label(chosen)} "
                f"conf={decision.confidence:.2f} for {label} ({decision.reason})"
            )
            return chosen.candidate_id
        if decision.action == "as_is":
            self._log(f"as-is: {label} ({decision.reason})")
            return BeetsMatchDecision.AS_IS
        self._log(f"skip: agent skipped {label} ({decision.reason})")
        return None

    def _choose(self, task: BeetsTaskView) -> MatchDecision | None:
        if self._chooser is None:
            self._chooser = _build_chooser(self.model_name, self._log)
        try:
            return self._chooser(task)
        except Exception as exc:  # noqa: BLE001 - fall back to a safe skip
            self._log(f"agent error: {exc}")
            return None


def _distance_key(match: BeetsMatchView) -> float:
    return match.distance if match.distance is not None else 1.0


def _source_label(task: BeetsTaskView) -> str:
    if task.paths:
        parent = task.paths[0].parent.name or task.paths[0].name
        return parent
    return task.task_id


def _match_label(match: BeetsMatchView) -> str:
    artist = match.artist or "?"
    title = match.album or match.title or "?"
    return f"{artist} - {title}"


def build_prompt(task: BeetsTaskView) -> str:
    """Build the user prompt describing the folder and candidate releases."""
    lines = [f"Source folder: {_source_label(task)}", "", "Track files:"]
    for path in task.paths[:25]:
        lines.append(f"- {path.name}")
    if len(task.paths) > 25:
        lines.append(f"- ... and {len(task.paths) - 25} more")
    lines += ["", "Candidates:"]
    for index, match in enumerate(task.matches):
        distance = "?" if match.distance is None else f"{match.distance:.3f}"
        lines.append(f"[{index}] {_match_label(match)} (distance {distance})")
    return "\n".join(lines)


def _build_chooser(model_name: str, log: Logger) -> Chooser:
    api_key = os.environ.get("OPENROUTER_API_KEY")
    if not api_key:
        log("agent disabled: OPENROUTER_API_KEY is not set")
        return lambda _task: None

    from pydantic_ai import Agent
    from pydantic_ai.models.openrouter import OpenRouterModel
    from pydantic_ai.providers.openrouter import OpenRouterProvider

    model = OpenRouterModel(model_name, provider=OpenRouterProvider(api_key=api_key))
    agent = Agent(model, output_type=MatchDecision, system_prompt=_SYSTEM_PROMPT)

    def choose(task: BeetsTaskView) -> MatchDecision | None:
        result = agent.run_sync(build_prompt(task))
        return cast(MatchDecision, result.output)

    return choose
