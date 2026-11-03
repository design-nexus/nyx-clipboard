# Nyx Clipboard

A fast floating clipboard for Omarchy and the Nyx suite: everything you copied,
searchable as you type, with pins, snippets, actions that fit what you copied, and the
verification codes from your notifications, kept safely for a few minutes.

**Super + Ctrl + V** opens it (Super + V stays Omarchy's universal paste). Type to
search; ↑ ↓ to move; Enter pastes into the window you were in.

## What it does

- **History** of text, formatted text, links, code, Markdown, HTML, images and
  screenshots, and copied files, each with its original formats (paste the formatting,
  or Shift + Enter for plain text). The same thing copied again moves to the top.
- **Search** as you type: substrings and fuzzy matches; `img:`, `url:`, `file:`,
  `code:`, `text:`, `fav:` filter by kind.
- **Recent, Pinned and Snippets.** Ctrl + P pins (pinned items are never removed),
  Ctrl + S stars. Snippets fill in `{date}`, `{time}`, `{clipboard}` and more.
- **Several at once:** Ctrl + Space marks items; Enter pastes them together.
- **Alt + 1 … 9** pastes one of the first nine.
- **Actions** (Menu key, Shift + F10, right-click): change case, trim, join lines,
  remove formatting or link tracking, convert Markdown, HTML and JSON, URL-encode;
  extract text from an image (Capture); save as a file or image; show in Files; new
  note or add to a note (Notes); save to Drive or share (Share); summarize, rewrite,
  translate, extract details or explain code with the Assistant, on this computer.
  Other apps can add their own (docs/API.md).
- **Hover** an item for its whole text or a larger picture.
- **Verification codes** from notifications show at the top with a countdown, and
  Passwords' browser extension offers them in code fields for the site they name.
- **Private mode** (Ctrl + Shift + P), app exclusions, and passwords and keys kept out
  of the history.
- **`cb` in Glimpse** searches the history and pastes from there.
- Follows the Nyx theme and Omarchy's, live.

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/design-nexus/nyx-clipboard/main/install.sh | bash
```

From a checkout: `./install.sh`. It builds, installs `nyx-clipboard`, `nyx-clipboardd`
and `nyx-clipboard-settings` to `~/.local/bin`, enables two user services
(`nyx-clipboardd`, `nyx-clipboard`), adds Clipboard's page to Settings → Desktop, and
binds the shortcut in a block it owns at the end of `~/.config/hypr/hyprland.lua`
(`NYX_SKIP_INTEGRATION=1` leaves Hyprland alone). Your existing Omarchy clipboard
history comes in on first start.

`./uninstall.sh` removes it and gives the key back to Omarchy; `--purge` also deletes the
history, snippets and settings.

## Settings

Settings → Desktop → Clipboard: the shortcut, where the popup opens (middle or
pointer), how much history to keep and for how long, images, private mode, excluded
apps, verification codes, the Assistant's translation language, and actions from other
apps. They live in `~/.config/nyx-clipboard/settings.toml`.

## Works with

| App | What | Needs |
| --- | --- | --- |
| Glimpse | `cb …` searches and pastes | Glimpse with the `cb` provider |
| Notification Center | finds codes in notifications, masks them in its history, copies them safely | nyx-notifyd |
| Passwords | fills codes from notifications in browsers | the paired extension, vault unlocked |
| Capture | Extract text from an image | `nyx-capture --ocr-file` |
| Notes | New note, Add to a note | Share for new notes; Notes open for adding |
| Share, Cloud | Share…, Save to Drive | Share; a Cloud account for Drive |
| Files | Show in Files | a FileManager1 service (nyx-files) |
| Assistant | Summarize, Rewrite, Translate, Extract, Explain code | the Assistant running |
| Settings | the Clipboard page | Nyx Settings |

## Command line

```sh
nyx-clipboard --toggle [recent|pinned|snippets]   # open or close the popup
nyx-clipboard --list [QUERY]                      # the history as JSON
nyx-clipboard --actions ID | --run ID ACTION      # an entry's actions
nyx-clipboard --integrate [KEY] | --unintegrate   # the Hyprland shortcut
```

## More

- [docs/API.md](docs/API.md): the D-Bus interfaces, who may use them, action manifests.
- [docs/SECURITY.md](docs/SECURITY.md): what's stored, caller checks, verification codes.
- [docs/LIMITATIONS.md](docs/LIMITATIONS.md): what depends on the compositor and other apps.

## Development

```sh
cargo test --locked --workspace
cargo build -p nyx-clipboardd -p nyx-clipboard && cargo build -p nyx-clipboard-dbus --example call
tests/run.sh          # the daemon on a private bus: access, import, codes
```

MIT licensed.
