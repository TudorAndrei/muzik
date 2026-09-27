"""Record selected beets config values for the Rust config loader."""

import json
from pathlib import Path
from tempfile import TemporaryDirectory

import beets
from beets import IncludeLazyConfig


ROOT = Path(__file__).resolve().parents[2]
OUTPUT = ROOT / "rust/crates/muzik-core/tests/fixtures/config.json"
USER_YAML = """\
library: ~/Music/.library.db
plugins: [musicbrainz, mbsync, fetchart, embedart]
paths:
  default: $albumartist/$album%aunique{}/$track $title
  comp: Compilations/$album%aunique{}/$track $title
  singleton: Non-Album/$artist/$title
match:
  strong_rec_thresh: 0.08
  preferred:
    countries: [GB, US]
fetchart:
  sources: [filesystem]
embedart:
  auto: yes
"""
OVERRIDES = {"match": {"medium_rec_thresh": 0.3}}
PATHS = [
    "library",
    "plugins",
    "paths.default",
    "paths.comp",
    "paths.singleton",
    "match.strong_rec_thresh",
    "match.medium_rec_thresh",
    "match.rec_gap_thresh",
    "match.preferred.countries",
    "match.preferred.media",
    "fetchart.sources",
    "embedart.auto",
    "import.move",
    "import.write",
    "create_backup_before_migrations",
]


def main() -> None:
    with TemporaryDirectory() as directory:
        user_file = Path(directory) / "config.yaml"
        user_file.write_text(USER_YAML, encoding="utf-8")
        config = IncludeLazyConfig("beets", "beets")
        config.read(user=False)
        config.set_file(str(user_file))
        config.set(OVERRIDES)
        values = {}
        for dotted_path in PATHS:
            view = config
            for key in dotted_path.split("."):
                view = view[key]
            values[dotted_path] = view.get()

    OUTPUT.parent.mkdir(parents=True, exist_ok=True)
    OUTPUT.write_text(
        json.dumps(
            {
                "beets_version": beets.__version__,
                "user_yaml": USER_YAML,
                "overrides": OVERRIDES,
                "values": values,
            },
            indent=2,
            sort_keys=True,
        )
        + "\n",
        encoding="utf-8",
    )


if __name__ == "__main__":
    main()
