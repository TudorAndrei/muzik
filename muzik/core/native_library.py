"""Python view of the native beets-compatible library reader."""

from __future__ import annotations

import logging
from pathlib import Path
from typing import Any

from beets.library import Library

from muzik.config import BEETS_CONFIG, get_native_settings
from muzik.core.beets.config import open_library


log = logging.getLogger(__name__)


class NativeItem:
    def __init__(self, data: dict[str, Any]) -> None:
        self.id: int = data["id"]
        self.path: bytes = data["fields"]["path"]
        self._fields: dict[str, Any] = data["fields"]
        self._attributes: dict[str, Any] = data["attributes"]

    def __getattr__(self, name: str) -> Any:
        return self.get(name)

    def get(self, name: str, default: Any = None) -> Any:
        return self._fields.get(name, self._attributes.get(name, default))


class NativeAlbum:
    def __init__(self, data: dict[str, Any], library: NativeLibrary) -> None:
        self.id: int = data["id"]
        self._fields: dict[str, Any] = data["fields"]
        self._attributes: dict[str, Any] = data["attributes"]
        self._library = library

    def __getattr__(self, name: str) -> Any:
        return self._fields.get(name, self._attributes.get(name))

    def items(self) -> list[NativeItem]:
        return [
            NativeItem(data) for data in self._library._reader.items_for_album(self.id)
        ]


class NativeLibrary:
    def __init__(self, config_path: Path | None = None) -> None:
        from muzik import _native

        self._reader = _native.NativeLibrary(str(config_path or BEETS_CONFIG))
        self.directory: str = self._reader.directory

    def items(self, query: str | None = None) -> list[NativeItem]:
        return [NativeItem(data) for data in self._reader.items(query or "")]

    def albums(self, query: str = "") -> list[NativeAlbum]:
        return [NativeAlbum(data, self) for data in self._reader.albums(query)]


class ShadowLibrary:
    def __init__(self, beets: Library, native: NativeLibrary | None) -> None:
        self._beets = beets
        self._native = native
        self.directory = beets.directory

    def items(self, query: str | None = None) -> list[Any]:
        items = list(self._beets.items(query))
        if self._native is not None:
            try:
                native = self._native.items(query)
                beets_rows = [(item.id, item.path, item.title) for item in items]
                native_rows = [(item.id, item.path, item.title) for item in native]
                if beets_rows != native_rows:
                    log.warning("Native library item results differ from beets")
            except Exception:
                log.warning(
                    "Native library item read failed in shadow mode", exc_info=True
                )
        return items

    def albums(self, query: str = "") -> list[Any]:
        albums = list(self._beets.albums(query))
        if self._native is not None:
            try:
                native = self._native.albums(query)
                beets_rows = [
                    (album.id, album.albumartist, album.album) for album in albums
                ]
                native_rows = [
                    (album.id, album.albumartist, album.album) for album in native
                ]
                if beets_rows != native_rows:
                    log.warning("Native library album results differ from beets")
            except Exception:
                log.warning(
                    "Native library album read failed in shadow mode", exc_info=True
                )
        return albums


def open_library_for_reads(
    config_path: Path | None = None,
) -> Library | NativeLibrary | ShadowLibrary:
    mode = get_native_settings()["library"]
    if mode == "native":
        return NativeLibrary(config_path)
    beets = open_library(config_path)
    if mode == "beets":
        return beets
    try:
        native = NativeLibrary(config_path)
    except Exception:
        log.warning("Native library open failed in shadow mode", exc_info=True)
        native = None
    return ShadowLibrary(beets, native)
