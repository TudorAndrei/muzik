"""Beets plugin: record the source YouTube video ID on import.

Reads the ``.muzik.json`` sidecar muzik writes next to a downloaded (or,
since the splitter also writes one per track, a split) audio file, and
saves its ``source_id`` as a flexible field on the imported item — before
beets moves/copies/links the file away from where the sidecar lives.

Runs for every operation beets can be configured to use (move, copy, link,
hardlink, reflink) via the matching "before/after the file op" event for
each; muzik's own imports always use move. Nothing calls ``item.store()``
here — the field is set before the batched store that already happens for
every item once the whole move/copy pass finishes (see
``beets.importer.manipulate_files``), so it's picked up for free.

This is what lets the Watchlist page later find an already-organized album
by an exact video-id match instead of guessing from the video's title
(``muzik/core/beets/lookup.py``). It cannot help anything imported before
this plugin existed — there's no id recorded anywhere for those.
"""

from __future__ import annotations

import os
from pathlib import Path
from typing import Any

from beets.plugins import BeetsPlugin

from muzik.core.metadata import find_muzik_metadata


FIELD_NAME = "muzik_source_id"

_MOVE_EVENTS = (
    "before_item_moved",
    "item_copied",
    "item_linked",
    "item_hardlinked",
    "item_reflinked",
)


class MuzikSourcePlugin(BeetsPlugin):
    def __init__(self) -> None:
        super().__init__()
        for event in _MOVE_EVENTS:
            self.register_listener(event, self._record_source_id)

    def _record_source_id(
        self,
        item: Any,
        source: bytes,
        destination: bytes,
    ) -> None:
        del destination
        if item.get(FIELD_NAME):
            return
        metadata = find_muzik_metadata(Path(os.fsdecode(source)))
        source_id = metadata.get("source_id") if metadata else None
        if source_id:
            item[FIELD_NAME] = source_id
