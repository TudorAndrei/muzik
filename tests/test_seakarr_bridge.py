"""Smoke test for the compiled ``muzik._seakarr`` extension.

The extension is built from ``rust/seakarr_bridge/`` with ``maturin develop``
(Phase 7 wires that into ``mise run check``; until then it is a manual step).
Skipped entirely when it has not been built, so the rest of the suite stays
green for anyone who has not built the Rust bridge locally.
"""

import pytest

_seakarr = pytest.importorskip("muzik._seakarr")


def test_module_exposes_the_documented_classes_and_exception() -> None:
    assert hasattr(_seakarr, "SeakarrSession")
    assert hasattr(_seakarr, "SeakarrJob")
    assert hasattr(_seakarr, "SeakarrError")
    assert issubclass(_seakarr.SeakarrError, Exception)


def test_session_has_the_documented_lifecycle_methods() -> None:
    session = _seakarr.SeakarrSession
    assert callable(session.connect)
    for name in ("start_track_search", "start_download", "close"):
        assert hasattr(session, name)


def test_job_has_the_documented_polling_methods() -> None:
    job = _seakarr.SeakarrJob
    for name in ("poll", "cancel", "result"):
        assert hasattr(job, name)
