# Clipboard

Applications: apps/daemon (`nyx-clipboardd`, no GTK: watches the Wayland clipboard,
keeps history, serves D-Bus), apps/popup (`nyx-clipboard`, the resident layer-shell
popup) and apps/settings (`nyx-clipboard-settings`, the Settings extension helper).
Libraries without GTK: crates/nyx-clipboard-core (classification, secrets, transforms,
verification codes, fuzzy search, peers), crates/nyx-clipboard-store (SQLite) and
crates/nyx-clipboard-dbus (names and a blocking client).

Preserve the application ID `io.github.design_nexus.nyx.Clipboard`, the bus name
`io.github.design_nexus.nyx.Clipboard1`, the binaries and the display name `Clipboard`.
`Clipboard1`, `Actions1` and `Otp1` are public (docs/API.md): change them only
compatibly; anything else needs a new version.

Rules: verification codes never reach SQLite, the clipboard or any search result; they
live in locked memory and expire (docs/SECURITY.md). Copies marked
`x-kde-passwordManagerHint` are never read beyond the hint. Every caller is identified
from its connection (`peer.rs`), never from what it sends. Don't claim an integration
works until it has been tried against the real app.

Commands: `cargo test --locked --workspace`; `cargo clippy --workspace --all-targets`;
`./install.sh`. `tests/dbus.sh` runs the daemon on a private bus. Try the popup on a
headless output without focus: `NYX_CLIPBOARD_NO_KEYBOARD=1 nyx-clipboard --only
HEADLESS-<n> --show` (its app ID is `io.github.design_nexus.nyx.Clipboard.Test`).
Never inject input or steal focus while testing.

Use the public, pinned shared crates; never import another application's internals.
Use canonical STYLE.md from the pinned nyx-design dependency.
Common guidance: https://github.com/design-nexus/nyx/blob/main/docs/AGENTS_COMMON.md
