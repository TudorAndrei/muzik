import importlib
from pathlib import Path

import platformdirs
import yaml
from beets import config as beets_config

from muzik import config
from muzik.commands import config as config_cmd


def test_paths_use_platformdirs_and_beets_config_helper(
    tmp_path: Path, monkeypatch
) -> None:
    original_platform_dirs = platformdirs.PlatformDirs
    original_beets_user_config_path = beets_config.user_config_path
    calls = []

    class FakePlatformDirs:
        def __init__(self, appname: str, appauthor: str | bool | None = None) -> None:
            calls.append((appname, appauthor))
            self.user_cache_path = tmp_path / "cache" / appname
            self.user_config_path = tmp_path / "config" / appname
            self.user_data_path = tmp_path / "data" / appname

    monkeypatch.setattr(platformdirs, "PlatformDirs", FakePlatformDirs)
    monkeypatch.setattr(
        beets_config,
        "user_config_path",
        lambda: str(tmp_path / "beets" / "config.yaml"),
    )

    try:
        reloaded = importlib.reload(config)

        assert calls == [("muzik", False)]
        assert reloaded.CACHE_DIR == tmp_path / "cache" / "muzik"
        assert reloaded.BANDCAMP_CACHE_FILE == reloaded.CACHE_DIR / "bandcamp.cache"
        assert reloaded.MUZIK_CONFIG_DIR == tmp_path / "config" / "muzik"
        assert reloaded.MUZIK_CONFIG_FILE == reloaded.MUZIK_CONFIG_DIR / "config.yaml"
        assert (
            reloaded.MUZIK_WATCHLIST_FILE
            == reloaded.MUZIK_CONFIG_DIR / "watchlist.json"
        )
        assert reloaded.BEETS_CONFIG == tmp_path / "beets" / "config.yaml"
        assert (
            reloaded.DEFAULT_DOWNLOAD_DIR == tmp_path / "data" / "muzik" / "downloads"
        )
        assert reloaded.DEFAULT_BANDCAMP_DIR == tmp_path / "data" / "muzik" / "bandcamp"
        assert reloaded.DEFAULT_SOULSEEK_DIR == tmp_path / "data" / "muzik" / "soulseek"
        assert reloaded.DEFAULT_SPLITS_DIR == tmp_path / "data" / "muzik" / "splits"
    finally:
        monkeypatch.setattr(platformdirs, "PlatformDirs", original_platform_dirs)
        monkeypatch.setattr(
            beets_config, "user_config_path", original_beets_user_config_path
        )
        importlib.reload(config)


def test_seakarr_settings_read_from_muzik_config(tmp_path: Path) -> None:
    cfg = tmp_path / "config.yaml"
    cfg.write_text(
        yaml.dump(
            {
                "soulseek": {
                    "username": "from-config",
                    "password": "config-secret",
                    "server_host": "config.slsknet.org",
                    "server_port": "1234",
                    "download_dir": str(tmp_path / "downloads"),
                }
            }
        ),
        encoding="utf-8",
    )

    settings = config.get_seakarr_settings(env={}, config_path=cfg)

    assert settings["username"] == "from-config"
    assert settings["password"] == "config-secret"
    assert settings["server_host"] == "config.slsknet.org"
    assert settings["server_port"] == "1234"
    assert settings["download_dir"] == str(tmp_path / "downloads")


def test_seakarr_env_overrides_muzik_config(tmp_path: Path) -> None:
    cfg = tmp_path / "config.yaml"
    cfg.write_text(
        yaml.dump(
            {"soulseek": {"username": "config-user", "password": "config-secret"}}
        ),
        encoding="utf-8",
    )

    settings = config.get_seakarr_settings(
        env={
            "MUZIK_SOULSEEK_USERNAME": "env-user",
            "MUZIK_SOULSEEK_PASSWORD": "env-secret",
            "MUZIK_SOULSEEK_DOWNLOAD_DIR": str(tmp_path / "env-downloads"),
        },
        config_path=cfg,
    )

    assert settings["username"] == "env-user"
    assert settings["password"] == "env-secret"
    assert settings["download_dir"] == str(tmp_path / "env-downloads")


def test_native_settings_default_to_beets(tmp_path: Path) -> None:
    assert config.get_native_settings(
        env={}, config_path=tmp_path / "missing.yaml"
    ) == {
        "match": "beets",
        "metadata": "beets",
        "tags": "beets",
        "library": "beets",
        "import": "beets",
    }


def test_native_settings_read_config_and_environment(tmp_path: Path) -> None:
    cfg = tmp_path / "config.yaml"
    cfg.write_text("native:\n  match: shadow\n  tags: native\n", encoding="utf-8")
    settings = config.get_native_settings(
        env={"MUZIK_NATIVE_MATCH": "native"}, config_path=cfg
    )
    assert settings["match"] == "native"
    assert settings["tags"] == "native"
    assert settings["library"] == "beets"


def test_config_set_soulseek_writes_muzik_config(
    tmp_path: Path,
    monkeypatch,
) -> None:
    cfg = tmp_path / "muzik" / "config.yaml"
    monkeypatch.setattr(config_cmd, "MUZIK_CONFIG_FILE", cfg)
    download_dir = tmp_path / "soulseek-downloads"

    config_cmd.config_set_soulseek(
        username="listener",
        password="secret",
        server_host="server.slsknet.org",
        server_port=2416,
        download_dir=download_dir,
    )

    data = yaml.safe_load(cfg.read_text(encoding="utf-8"))
    assert data["soulseek"] == {
        "username": "listener",
        "password": "secret",
        "server_host": "server.slsknet.org",
        "server_port": 2416,
        "download_dir": str(download_dir),
    }
    assert download_dir.exists()
