#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

exec uvx --isolated --python 3.14 \
  --from beets==2.13.1 \
  --with confuse==2.3.0 \
  --with jellyfish==1.3.0 \
  --with lap==0.5.13 \
  --with mediafile==0.17.0 \
  --with mutagen==1.48.1 \
  --with numpy==2.5.4 \
  --with PyYAML==6.0.3 \
  --with Unidecode==1.4.0 \
  python -I "$root/scripts/beets_fixtures.py" "$@"
