"""Check that release wheels contain an executable GPUI desktop program."""

from __future__ import annotations

from pathlib import Path
import stat
from zipfile import ZipFile


def main() -> None:
    wheels = list(Path("dist").glob("muzik-*.whl"))
    if len(wheels) != 1:
        raise SystemExit(f"Expected one wheel in dist, found {len(wheels)}")
    with ZipFile(wheels[0]) as wheel:
        matches = [
            info for info in wheel.infolist() if info.filename == "muzik/bin/muzik-gpui"
        ]
        if len(matches) != 1:
            raise SystemExit("Wheel has no muzik/bin/muzik-gpui")
        mode = matches[0].external_attr >> 16
        if not mode & stat.S_IXUSR:
            raise SystemExit("GPUI binary in wheel is not executable")
    print(f"GPUI binary is in {wheels[0]}")


if __name__ == "__main__":
    main()
