"""Beets plugin: move a "feat." credit from a track title into the artist.

Runs on every muzik import (ASIS, agent-picked, or ``--library`` re-tag), so a
title like "Song (feat. X)" becomes title "Song" with artist "Main feat. X" —
the form beets and MusicBrainz use. The workflow already does this at split
time; this covers files imported straight into beets. The transform is
idempotent: a title with no credit is left untouched.

It listens on ``album_imported``/``item_imported`` (which fire for every import,
unlike ``import_task_apply`` which fires only for an applied match), so it must
re-write and re-store each changed item after beets has already placed it.
"""

from __future__ import annotations

from beets import config as beets_config
from beets.plugins import BeetsPlugin

from muzik.core.chapters import strip_featured


class FtCleanPlugin(BeetsPlugin):
    def __init__(self) -> None:
        super().__init__()
        self.register_listener("album_imported", self.on_album)
        self.register_listener("item_imported", self.on_item)

    def on_album(self, lib, album) -> None:  # noqa: ANN001 - beets types
        for item in album.items():
            if self._clean(item):
                self._persist(item)

    def on_item(self, lib, item) -> None:  # noqa: ANN001 - beets types
        if self._clean(item):
            self._persist(item)

    def _clean(self, item) -> bool:  # noqa: ANN001 - beets Item
        """Rewrite the item's title/artist in memory; return whether it changed."""
        clean, featured = strip_featured(item.title or "")
        if not featured:
            return False
        item.title = clean
        artist = item.artist or ""
        if "feat" not in artist.lower() and "ft." not in artist.lower():
            item.artist = f"{artist} feat. {', '.join(featured)}".strip()
        return True

    def _persist(self, item) -> None:  # noqa: ANN001 - beets Item
        """Save the change to the library and, when moving/writing, to the file."""
        item.store()
        import_cfg = beets_config["import"]
        if import_cfg["move"].get(bool) or import_cfg["copy"].get(bool):
            try:
                item.move()
                item.store()
            except Exception as exc:  # keep a title-rename failure non-fatal
                self._log.debug("ftclean move failed: {}", exc)
        if import_cfg["write"].get(bool):
            try:
                item.try_write()
            except Exception as exc:
                self._log.debug("ftclean write failed: {}", exc)
