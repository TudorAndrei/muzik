import os
import time
from pathlib import Path

import pytest
import typer

from muzik.commands import cache as cache_cmd
from muzik.core import cache as cache_mod


@pytest.mark.parametrize("key", ["", "..", "a/b", "a\\b", "/absolute"])
def test_cache_rejects_unsafe_keys(key: str, tmp_path: Path, monkeypatch) -> None:
    monkeypatch.setattr(cache_mod, "CACHE_DIR", tmp_path / "cache")

    with pytest.raises(cache_mod.CachePathError):
        cache_mod.set(key, "value")


def test_cache_validates_extensions_and_round_trips(
    tmp_path: Path, monkeypatch
) -> None:
    monkeypatch.setattr(cache_mod, "CACHE_DIR", tmp_path / "cache")

    cache_mod.set("split_abc-123", "value")

    assert cache_mod.get("split_abc-123") == "value"
    assert cache_mod._path("split_abc-123").is_relative_to(
        (tmp_path / "cache").resolve()
    )
    with pytest.raises(cache_mod.CachePathError):
        cache_mod.get("split_abc", ext="sqlite")


def test_cache_clear_rejects_traversal_without_touching_outside_file(
    tmp_path: Path, monkeypatch
) -> None:
    cache_dir = tmp_path / "cache"
    sentinel = tmp_path / "outside.txt"
    sentinel.write_text("keep")
    monkeypatch.setattr(cache_mod, "CACHE_DIR", cache_dir)

    with pytest.raises(typer.Exit) as result:
        cache_cmd.cache_clear("../outside")

    assert result.value.exit_code == 1
    assert sentinel.read_text() == "keep"
    assert not cache_dir.exists()


def test_cache_clear_all_requires_confirmation_and_removes_only_cache_files(
    tmp_path: Path, monkeypatch
) -> None:
    cache_dir = tmp_path / "cache"
    monkeypatch.setattr(cache_mod, "CACHE_DIR", cache_dir)
    cache_mod.set("entry", "value")
    monkeypatch.setattr(typer, "confirm", lambda *args, **kwargs: True)

    cache_cmd.cache_clear(key=None)

    assert cache_mod.list_all() == []


def test_cache_purge_is_limited_to_configured_roots(
    tmp_path: Path, monkeypatch
) -> None:
    cache_dir = tmp_path / "cache"
    downloads = tmp_path / "downloads"
    splits = tmp_path / "splits"
    unrelated = tmp_path / "unrelated"
    monkeypatch.setattr(cache_mod, "CACHE_DIR", cache_dir)
    monkeypatch.setattr(cache_cmd, "DEFAULT_DOWNLOAD_DIR", downloads)
    monkeypatch.setattr(cache_cmd, "DEFAULT_SPLITS_DIR", splits)
    monkeypatch.setattr(typer, "confirm", lambda *args, **kwargs: True)
    cache_mod.set("entry", "value")
    downloads.mkdir()
    splits.mkdir()
    unrelated.mkdir()
    (downloads / "audio.flac").write_bytes(b"audio")
    (splits / "album").mkdir()
    (unrelated / "keep.txt").write_text("keep")

    cache_cmd.cache_purge()

    assert cache_mod.list_all() == []
    assert downloads.exists() and list(downloads.iterdir()) == []
    assert splits.exists() and list(splits.iterdir()) == []
    assert (unrelated / "keep.txt").read_text() == "keep"


def test_cache_clean_removes_only_empty_and_expired_entries(
    tmp_path: Path, monkeypatch
) -> None:
    cache_dir = tmp_path / "cache"
    monkeypatch.setattr(cache_mod, "CACHE_DIR", cache_dir)
    cache_mod.set("fresh", "value")
    cache_mod.set("old", "value")
    cache_mod.set("empty", "")
    old_path = cache_mod._path("old")
    old_time = time.time() - 2 * 24 * 60 * 60
    os.utime(old_path, (old_time, old_time))

    assert cache_mod.clean(max_age_days=1) == 2
    assert cache_mod.get("fresh") == "value"
    assert cache_mod.get("old") is None
    assert cache_mod.get("empty") is None
