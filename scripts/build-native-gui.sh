#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "$0")/.." && pwd)
manifest="$repo_root/rust/gpui_app/Cargo.toml"
target_args=()
target_dir="$repo_root/rust/gpui_app/target"

if [[ -n "${MUZIK_NATIVE_TARGET:-}" ]]; then
  target_args=(--target "$MUZIK_NATIVE_TARGET")
  target_dir="$target_dir/$MUZIK_NATIVE_TARGET"
fi

cargo build --manifest-path "$manifest" --release --locked "${target_args[@]}"
install -d "$repo_root/muzik/bin"
install -m 755 "$target_dir/release/muzik-gpui" "$repo_root/muzik/bin/muzik-gpui"
