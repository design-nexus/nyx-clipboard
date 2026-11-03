#!/usr/bin/env bash
# Build what the checks need, then run them: unit tests, and the daemon on a private bus.
set -euo pipefail
cd "$(dirname -- "${BASH_SOURCE[0]}")/.."
cargo test --locked --workspace --quiet
cargo build --locked --quiet -p nyx-clipboardd -p nyx-clipboard
cargo build --locked --quiet -p nyx-clipboard-dbus --example call
tests/dbus.sh
tests/codes.sh
