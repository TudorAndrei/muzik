"""Check that a source archive can build both Rust parts of muzik."""

from __future__ import annotations

from pathlib import Path
import tarfile


REQUIRED = (
    "rust/Cargo.toml",
    "rust/Cargo.lock",
    "rust/crates/muzik-soulseek/Cargo.toml",
    "rust/crates/muzik-soulseek/src/lib.rs",
    "rust/crates/muzik-py/Cargo.toml",
    "rust/crates/muzik-py/src/lib.rs",
    "rust/gpui_app/Cargo.toml",
    "rust/gpui_app/Cargo.lock",
    "rust/gpui_app/src/main.rs",
    "rust/gpui_app/src/bridge.rs",
    "scripts/build-native-gui.sh",
)


def main() -> None:
    archives = list(Path("dist").glob("muzik-*.tar.gz"))
    if len(archives) != 1:
        raise SystemExit(f"Expected one source archive in dist, found {len(archives)}")
    with tarfile.open(archives[0], "r:gz") as archive:
        names = {member.name.split("/", 1)[1] for member in archive.getmembers()}
    missing = [path for path in REQUIRED if path not in names]
    if missing:
        raise SystemExit(f"Source archive lacks: {', '.join(missing)}")
    print(f"Native GUI source is in {archives[0]}")


if __name__ == "__main__":
    main()
