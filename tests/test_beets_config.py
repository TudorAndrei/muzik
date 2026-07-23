from pathlib import Path

from muzik.core.beets import config as beets_service


def test_open_library_uses_beets_212_library_constructor(monkeypatch) -> None:
    calls: list[object] = []

    class FakeValue:
        def __init__(self, value: str) -> None:
            self.value = value

        def as_filename(self) -> str:
            return self.value

    class FakeConfig:
        def set_file(self, value: str) -> None:
            calls.append(("set_file", value))

        def __getitem__(self, key: str) -> FakeValue:
            return FakeValue({"library": "library.blb", "directory": "music"}[key])

    class FakeLibrary:
        def __init__(self, path: str, directory: str) -> None:
            calls.append(("library", path, directory))

    monkeypatch.setattr(beets_service, "config", FakeConfig())
    monkeypatch.setattr(beets_service, "Library", FakeLibrary)
    monkeypatch.setattr(
        beets_service.plugins, "load_plugins", lambda: calls.append("load")
    )
    monkeypatch.setattr(
        beets_service.plugins,
        "send",
        lambda event, **kwargs: calls.append((event, kwargs["lib"])),
    )

    library = beets_service.open_library(Path("beets.yaml"))

    assert calls == [
        ("set_file", "beets.yaml"),
        "load",
        ("library", "library.blb", "music"),
        ("library_opened", library),
    ]
