# Distribution

The project is moving to a Rust CLI and a Rust GPUI app. The Cargo workspace
is at the repository root. It contains the library crates, the CLI app in
`rust/cli_app`, and the GPUI app in `rust/gpui_app`.

No release workflow runs now. The GPUI app starts the Python service in
`muzik/native_gui` for workflow, watchlist, and Spotify playlist reads. Spotify status, client ID changes, login, and logout run in Rust. The Rust CLI does not yet have all commands from the Python
CLI. A Rust-only release needs both ports to be complete and checked.

Cocogitto reads conventional commits and controls version tags. Its version
hook runs `muzik-release`, which sets the version of each Rust crate and updates
the root `Cargo.lock`. Run the Rust checks with `cargo test --workspace --locked`
and `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings`.

Existing beets config files and SQLite library files must remain usable. The
Rust crates use these files directly. A Rust release must check this data with
the CLI and the app before it is published.
