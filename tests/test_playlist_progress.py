"""Tests for playlist-wide progress: dry-Soulseek disabling and failure summary."""

from muzik.core.workflow.service import _PlaylistProgress


def test_soulseek_disables_after_threshold_dry_runs() -> None:
    p = _PlaylistProgress(soulseek_dry_threshold=3)
    p.note_soulseek_dry()
    p.note_soulseek_dry()
    assert p.soulseek_disabled is False
    p.note_soulseek_dry()
    assert p.soulseek_disabled is True


def test_a_hit_resets_the_dry_streak() -> None:
    p = _PlaylistProgress(soulseek_dry_threshold=3)
    p.note_soulseek_dry()
    p.note_soulseek_dry()
    p.note_soulseek_hit()  # a successful Soulseek acquire breaks the streak
    p.note_soulseek_dry()
    p.note_soulseek_dry()
    assert p.soulseek_disabled is False
    p.note_soulseek_dry()
    assert p.soulseek_disabled is True


def test_failed_downloads_are_collected() -> None:
    p = _PlaylistProgress()
    assert p.failed == []
    p.note_failed("abc123")
    p.note_failed("def456")
    assert p.failed == ["abc123", "def456"]
