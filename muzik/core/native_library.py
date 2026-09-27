"""Python view of the native music library reader."""

from __future__ import annotations

import importlib
from pathlib import Path
from typing import Any

from muzik.config import LIBRARY_CONFIG


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
        _native = importlib.import_module("muzik._native")

        self._reader = _native.NativeLibrary(str(config_path or LIBRARY_CONFIG))
        self.directory: str = self._reader.directory

    def items(self, query: str | None = None) -> list[NativeItem]:
        return [NativeItem(data) for data in self._reader.items(query or "")]

    def albums(self, query: str = "") -> list[NativeAlbum]:
        return [NativeAlbum(data, self) for data in self._reader.albums(query)]

    def prune_missing_items(
        self, safety_fraction: float = 0.5
    ) -> tuple[int, tuple[int, int] | None]:
        return self._reader.prune_missing_items(safety_fraction)


def open_library_for_reads(
    config_path: Path | None = None,
) -> NativeLibrary:
    return NativeLibrary(config_path)
