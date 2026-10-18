#!/usr/bin/env bash
set -euo pipefail

root="${MUZIK_HOME:?set MUZIK_HOME to the profile folder}"

real_data() {
  if [[ "$(uname)" == "Darwin" ]]; then
    printf '%s\n' "$HOME/Library/Application Support/com.tudorandrei.muzik"
  else
    printf '%s\n' "${XDG_DATA_HOME:-$HOME/.local/share}/muzik"
  fi
}

real_config() {
  if [[ "$(uname)" == "Darwin" ]]; then
    real_data
  else
    printf '%s\n' "${XDG_CONFIG_HOME:-$HOME/.config}/muzik"
  fi
}

case "$root" in
  / | "$HOME" | "$HOME/" | "$(real_data)" | "$(real_config)")
    echo "MUZIK_HOME must not be $root" >&2
    exit 1
    ;;
esac

init() {
  mkdir -p "$root/data" "$root/config" "$root/cache" "$root/beets" "$root/music"
  if [[ ! -f "$root/beets/config.yaml" ]]; then
    cat >"$root/beets/config.yaml" <<EOF
directory: $root/music
library: $root/beets/library.db
EOF
  fi
}

copy() {
  init
  local source
  source="$(real_data)/muzik.db"
  if [[ ! -f "$source" ]]; then
    echo "No database at $source" >&2
    exit 1
  fi
  rm -f "$root/data/muzik.db" "$root/data/muzik.db-wal" "$root/data/muzik.db-shm"
  sqlite3 "$source" ".backup '$root/data/muzik.db'"
  local name
  for name in bandcamp_cookies.txt bandcamp_user; do
    if [[ -f "$(real_config)/$name" ]]; then
      cp "$(real_config)/$name" "$root/config/$name"
    fi
  done
  echo "Copied the real database and sign-ins into $root"
}

case "${1:-init}" in
  init) init ;;
  copy) copy ;;
  reset)
    rm -rf "$root"
    init
    ;;
  *)
    echo "usage: $0 [init|copy|reset]" >&2
    exit 2
    ;;
esac
