"""Tests for the ftclean beets plugin's per-item transform."""

import beetsplug

from muzik.core.beets.config import _PLUGIN_DIR

# Make ``beetsplug.ftclean`` importable the same way load_config does at runtime.
if str(_PLUGIN_DIR) not in beetsplug.__path__:
    beetsplug.__path__.append(str(_PLUGIN_DIR))

from beetsplug.ftclean import FtCleanPlugin  # noqa: E402


class _Item:
    def __init__(self, title: str, artist: str) -> None:
        self.title = title
        self.artist = artist


def _plugin() -> FtCleanPlugin:
    return FtCleanPlugin()


def test_clean_moves_feat_into_artist() -> None:
    item = _Item("Song (feat. Guest)", "Main")
    changed = _plugin()._clean(item)
    assert changed is True
    assert item.title == "Song"
    assert item.artist == "Main feat. Guest"


def test_clean_leaves_plain_title() -> None:
    item = _Item("Plain Song", "Main")
    assert _plugin()._clean(item) is False
    assert item.title == "Plain Song"
    assert item.artist == "Main"


def test_clean_does_not_double_credit() -> None:
    # Artist already credits the feature; title is cleaned, artist untouched.
    item = _Item("Song (feat. Guest)", "Main feat. Guest")
    assert _plugin()._clean(item) is True
    assert item.title == "Song"
    assert item.artist == "Main feat. Guest"
