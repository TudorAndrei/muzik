"""Smoke test for the compiled ``muzik._native`` extension.

The extension is built from ``rust/crates/muzik-py/`` with ``maturin develop``
The test skips when the extension has not been built locally.
"""

import pytest

_native = pytest.importorskip("muzik._native")


def test_module_exposes_the_documented_classes_and_exception() -> None:
    assert hasattr(_native, "SeakarrSession")
    assert hasattr(_native, "SeakarrJob")
    assert hasattr(_native, "SeakarrError")
    assert issubclass(_native.SeakarrError, Exception)


def test_session_has_the_documented_lifecycle_methods() -> None:
    session = _native.SeakarrSession
    assert callable(session.connect)
    for name in ("start_track_search", "start_download", "close"):
        assert hasattr(session, name)


def test_job_has_the_documented_polling_methods() -> None:
    job = _native.SeakarrJob
    for name in ("poll", "cancel", "result"):
        assert hasattr(job, name)
