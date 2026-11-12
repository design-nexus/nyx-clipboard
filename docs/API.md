# Nyx Clipboard D-Bus API

Bus name `io.github.design_nexus.nyx.Clipboard1`, object `/io/github/design_nexus/nyx/Clipboard1`,
on the session bus, served by `nyx-clipboardd` (a user service, also D-Bus activated).
Structured values are JSON strings; ignore fields you don't know. Compatible additions
(methods, JSON fields) keep the version; anything else gets a new one.

## Who may call

Every call is checked against the caller's connection: the bus's credentials for it
(uid, pid, pidfd) and the executable that pid runs, read with the process pinned (see
`crates/nyx-clipboard-core/src/peer.rs`). The executable must sit in the daemon's own
folder (`~/.local/bin` when installed) and decides the caller's scope:

| Program | Scope | May use |
| --- | --- | --- |
| `nyx-clipboard` (the popup) | Popup | everything below except `Otp1.Offer`, `Match`, `Take` |
| `nyx-glimpse`, `nyx-glimpse-backend` (the Quickshell panel's search) | Glimpse | `Clipboard1` history methods, `Actions1.List`/`Run` and `Control` like Command; no codes |
| `nyx-clipboard-settings` | Settings | `Status`, `Private`, `SetPrivate`, `Clear`, `Reload`, `Actions1.Manifests` |
| `nyx-notifyd` | Notifications | `Otp1.Offer`, `Otp1.Dismiss` |
| `nyx-notification-center` | NotificationCenter | `Otp1.CopyCode` |
| `nyx-passwordsd` | Vault | `Otp1.Match`, `Otp1.Take` |
| `nyx-actions` (the `nyx` command) | Command | `Control`: status, private mode on, clear (asking first), open |
| any other `nyx-*` | App | `Actions1.Register`, `Unregister`; `Control`: status, private mode on, open |

Anything else (busctl, scripts, other users, other folders) gets
`org.freedesktop.DBus.Error.AccessDenied`.

## `io.github.design_nexus.nyx.Clipboard1`

| Method | Signature | Returns |
| --- | --- | --- |
| `Status` | `() → s` | `{entries, pinned, snippets, bytes, private, watching, problem, version}` |
| `Search` | `(s query) → s` | Entries (below), best first. `query` is `{text, view, kind, offset, limit}` (all optional; `""` for the newest 50). `view`: `recent`, `pinned`, `favorites`. `text` takes prefixes: `img:`, `url:`, `file:`, `code:`, `text:`, `rich:`, `html:`, `md:`, `fav:`, `pin:`. Substrings through an FTS5 trigram index, then fuzzy matches among the 2,000 newest |
| `Get` | `(x id) → s` | `{entry, text, truncated, html, files}` (text cut at 256 KiB), or `""` |
| `Copy` | `(x id, b plain) → b` | Puts the entry back on the clipboard: every kept representation, or only its text |
| `Paste` | `(ax ids, b plain) → b` | As `Copy`, then pastes into the focused window 140 ms later (close your window first). Several ids are joined with the separator from Settings |
| `CopyText` | `(s text, b paste) → b` | Copies text (an action's result) |
| `SetPinned` | `(x id, b on) → b` | Pinned entries are never removed by retention |
| `SetFavorite` | `(x id, b on) → b` | |
| `Delete` | `(ax ids) → u` | Deleted for good, files too |
| `Clear` | `(b pinned) → u` | The history (pinned entries too when `pinned`) |
| `Snippets` | `(s query) → s` | `[{id, name, body, created, updated}]` |
| `SaveSnippet` | `(s snippet) → x` | `{id, name, body}`; `id` 0 adds one. Popup only |
| `DeleteSnippet` | `(x id) → b` | Popup only |
| `UseSnippet` | `(x id, b paste) → b` | Copies it with `{date}`, `{time}`, `{datetime}`, `{weekday}`, `{clipboard}` filled in |
| `Transient` | `() → s` | Copied secrets kept in memory (never stored): `[{id, label, created, expires}]`. Popup only |
| `UseTransient` | `(s id, b paste) → b` | Copies one, marked sensitive. Popup only |
| `ForgetTransient` | `(s id) → b` | Popup only |
| `Private` / `SetPrivate` | `() → b` / `(b) → b` | Private mode: nothing new is recorded |
| `Reload` | `() → ()` | Read `settings.toml` again and apply retention |

Signal `Changed()` (no payload): read again.

An entry: `{id, kind, preview, size, chars, mimes, app, created, last_used, uses, pinned,
favorite, image, dims}`. `kind` is `text`, `rich`, `url`, `code`, `markdown`, `html`,
`image` or `files`; `image` is the stored file for previews.

## `io.github.design_nexus.nyx.Clipboard.Actions1`

| Method | Signature | Returns |
| --- | --- | --- |
| `List` | `(x id) → s` | `[{id, label, icon, group, source, destructive, previews}]` for that entry, in menu order |
| `Run` | `(x id, s action, s args) → s` | `{ok, message, text, copied, close}`; `args` is `{paste, target}` |
| `Register` | `(s manifest) → b` | A running Nyx app adds an action (below, with a `dbus` target). It lasts until the app leaves the bus |
| `Unregister` | `(s id) → b` | |
| `Manifests` | `() → s` | Third-party manifests and whether each is approved (Settings) |

Built-in actions: paste, paste as plain text, copy, pin, favorite, save as snippet,
delete; change case (upper, lower, title, sentence, snake, kebab, camel), trim, join
lines, remove tracking from links, remove formatting; Markdown ↔ HTML, HTML → text,
JSON format and compact, URL encode and decode; copy as path or file link, Show in
Files; Extract text (Capture), Save image, Save as file, New note, Add to a note, Save
to Drive, Share; Assistant summarize, rewrite, translate, extract details, explain code.
Actions for apps that aren't installed (or, for the Assistant, not running) aren't
listed.

### Action manifests

A Nyx app registers one at run time, or a third party drops one into
`~/.local/share/nyx-clipboard/actions/<id>.toml`, where it does nothing until the user
turns it on in Settings → Desktop → Clipboard:

```toml
id = "com.example.paste-bin"      # reverse-DNS
label = "Upload to paste.example"
icon = "send-to-symbolic"
kinds = ["text", "code"]          # empty: every kind with text
exec = ["/usr/bin/paste-upload"]  # gets the text on stdin; its stdout is the result (1 MiB, 30 s)
output = "copy"                   # copy | preview | none
```

or, for a Nyx app's own Control tool (called with `{"text": …}`):

```toml
[dbus]
bus = "io.github.design_nexus.nyx.Notes"
path = "/io/github/design_nexus/nyx/Notes"
tool = "notes.clip"
```

Programs run with a cleared environment (PATH, HOME, LANG, XDG_RUNTIME_DIR,
WAYLAND_DISPLAY only). Content never goes on a command line.

## `io.github.design_nexus.nyx.Clipboard.Otp1`

Verification codes found in notifications. They live in locked memory (`mlock`, zeroed
on drop, the process not dumpable), expire after Settings' time (3 minutes by default),
go when the session locks, the computer sleeps or the service stops, and are never
written anywhere or returned by `Clipboard1`.

| Method | Signature | Who | Returns |
| --- | --- | --- | --- |
| `Offer` | `(s summary, s body, s app) → b` | nyx-notifyd | Whether a code was found and held. Nothing else about the notification is kept |
| `Current` | `() → s` | popup | `[{id, code, app, sites, received, expires}]` |
| `Copy` | `(s id, b paste) → b` | popup | Copies a code marked sensitive; cleared after Settings' time if it's still there |
| `CopyCode` | `(s code) → b` | popup, Notification Center | The same, for a code the caller shows |
| `Dismiss` | `(s id) → b` | popup, nyx-notifyd | |
| `Match` | `(s url) → s` | nyx-passwordsd | `[{id, app, confidence, received, expires}]`, no codes. `high`: the notification names this site (a domain, a sender address, the brand); `low`: it names none. Codes naming another site aren't listed |
| `Take` | `(s id, s url, b confirmed) → s` | nyx-passwordsd | `{code, confidence}`, once. A `low` code needs `confirmed` (the user picked it); never for another site |

Signal `Changed()` (no payload).

## Examples

```sh
nyx-clipboard --list "url: github"          # the history as JSON (the popup's scope)
nyx-clipboard --actions 42                  # what entry 42 offers
nyx-clipboard --run 42 ocr                  # run one; prints the outcome
tests/dbus.sh && tests/codes.sh             # the checks, on a private bus
```

## Control (`io.github.design_nexus.nyx.Control`)

The suite's control interface (API 2, nyx-core's nyx-actions contract) on the same
path, for `nyx clipboard …`, Glimpse's commands and Nyx Assistant:
`clipboard.status` (counts and private mode, nothing from the history),
`clipboard.private` (turns private mode on; off stays with the popup and Settings),
`clipboard.clear` (the popup, Settings, Glimpse and Command scopes only, and only confirmed) and `clipboard.open`.
`nyx-clipboardd --describe-actions` prints the description the installer writes to
`~/.local/share/nyx/actions/`.
