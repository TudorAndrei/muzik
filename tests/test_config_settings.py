from pathlib import Path

import yaml

from muzik import config
from muzik.commands import config as config_cmd


def test_slskd_settings_read_from_muzik_config(tmp_path: Path) -> None:
    cfg = tmp_path / "config.yaml"
    cfg.write_text(
        yaml.dump(
            {
                "slskd": {
                    "url": "http://slskd.local:5030/",
                    "api_key": "from-config",
                    "download_dir": str(tmp_path / "downloads"),
                }
            }
        ),
        encoding="utf-8",
    )

    settings = config.get_slskd_settings(env={}, config_path=cfg)

    assert settings == {
        "url": "http://slskd.local:5030",
        "api_key": "from-config",
        "download_dir": str(tmp_path / "downloads"),
    }


def test_slskd_env_overrides_muzik_config(tmp_path: Path) -> None:
    cfg = tmp_path / "config.yaml"
    cfg.write_text(
        yaml.dump({"slskd": {"url": "http://config", "api_key": "config-key"}}),
        encoding="utf-8",
    )

    settings = config.get_slskd_settings(
        env={
            "SLSKD_URL": "http://env/",
            "SLSKD_API_KEY": "env-key",
            "SLSKD_DOWNLOAD_DIR": str(tmp_path / "env-downloads"),
        },
        config_path=cfg,
    )

    assert settings["url"] == "http://env"
    assert settings["api_key"] == "env-key"
    assert settings["download_dir"] == str(tmp_path / "env-downloads")


def test_config_set_soulseek_writes_muzik_config(
    tmp_path: Path,
    monkeypatch,
) -> None:
    cfg = tmp_path / "muzik" / "config.yaml"
    monkeypatch.setattr(config_cmd, "MUZIK_CONFIG_FILE", cfg)
    download_dir = tmp_path / "slskd-downloads"

    config_cmd.config_set_soulseek(
        url="http://localhost:5030/",
        api_key="secret",
        download_dir=download_dir,
    )

    data = yaml.safe_load(cfg.read_text(encoding="utf-8"))
    assert data["slskd"] == {
        "url": "http://localhost:5030",
        "api_key": "secret",
        "download_dir": str(download_dir),
    }
    assert download_dir.exists()
