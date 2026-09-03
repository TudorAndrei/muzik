"""Tests for the muzik_source beets plugin's per-item source-id capture."""

import os
from pathlib import Path

import beetsplug
from beets.library import Item

from muzik.core.beets.config import _PLUGIN_DIR
from muzik.core.metadata import write_muzik_metadata

# Make ``beetsplug.muzik_source`` importable the same way load_config does.
if str(_PLUGIN_DIR) not in beetsplug.__path__:
    beetsplug.__path__.append(str(_PLUGIN_DIR))

from beetsplug.muzik_source import FIELD_NAME, MuzikSourcePlugin  # noqa: E402


def _plugin() -> MuzikSourcePlugin:
    return MuzikSourcePlugin()


def test_records_the_source_id_from_the_sidecar_next_to_the_source(
    tmp_path: Path,
) -> None:
    source = tmp_path / "Title [abcdefghijk].opus"
    source.write_bytes(b"audio")
    write_muzik_metadata(source, {"source_id": "abcdefghijk"})
    item = Item()

    _plugin()._record_source_id(item, os.fsencode(str(source)), b"/library/dest.opus")

    assert item.get(FIELD_NAME) == "abcdefghijk"


def test_does_nothing_without_a_sidecar(tmp_path: Path) -> None:
    source = tmp_path / "Title [abcdefghijk].opus"
    source.write_bytes(b"audio")
    item = Item()

    _plugin()._record_source_id(item, os.fsencode(str(source)), b"/library/dest.opus")

    assert not item.get(FIELD_NAME)


def test_does_not_overwrite_an_already_recorded_id(tmp_path: Path) -> None:
    source = tmp_path / "Title [abcdefghijk].opus"
    source.write_bytes(b"audio")
    write_muzik_metadata(source, {"source_id": "different-id"})
    item = Item()
    item[FIELD_NAME] = "abcdefghijk"

    _plugin()._record_source_id(item, os.fsencode(str(source)), b"/library/dest.opus")

    assert item.get(FIELD_NAME) == "abcdefghijk"
