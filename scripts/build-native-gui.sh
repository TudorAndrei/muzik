#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "$0")/.." && pwd)
manifest="$repo_root/rust/gpui_app/Cargo.toml"
target_dir="$repo_root/target"

if [[ -n "${MUZIK_NATIVE_TARGET:-}" ]]; then
  target_dir="$target_dir/$MUZIK_NATIVE_TARGET"
  cargo build --manifest-path "$manifest" --release --locked --target "$MUZIK_NATIVE_TARGET"
else
  cargo build --manifest-path "$manifest" --release --locked
fi

install -d "$repo_root/muzik/bin"
install -m 755 "$target_dir/release/muzik-gpui" "$repo_root/muzik/bin/muzik-gpui"
