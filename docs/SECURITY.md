# Security and privacy

Everything stays on this computer. There is no network code in Nyx Clipboard; the only
programs it talks to are Nyx apps on the session bus, Capture (run for OCR) and the
Assistant's private socket (only when you pick an Assistant action).

## What is stored

- **Where:** `~/.local/share/nyx/clipboard.db` (SQLite, WAL, mode 0600, `secure_delete`
  on) and `~/.local/share/nyx-clipboard/blobs/` (0700; images and large copies, named
  after their content hash). Previews are cached in `~/.cache/nyx-clipboard/thumbs`.
- **Never stored:**
  - copies that carry `x-kde-passwordManagerHint` (Passwords, KeePassXC, 1Password…):
    the daemon doesn't even read them;
  - copies that look like secrets (private keys, access tokens, API keys, `password: …`,
    generated passwords) while *Don't save passwords and keys* is on: kept in memory
    for two minutes so the popup can still paste them, marked sensitive;
  - anything copied in private mode;
  - copies made while an excluded app is focused;
  - verification codes, ever.
- **Retention:** by count, age and total size (Settings); pinned entries stay.
  *Clear history* deletes rows and files.
- **Omarchy's history** (`~/.local/state/omarchy/clipboard-history.json`) is read once,
  on first start, skipping secrets. Omarchy's own watchers keep running and keep their
  own copy, which Nyx Clipboard doesn't control.

## Who can ask for what

Callers are identified from their bus connection, never from what they send: the
credentials the bus daemon gives for the connection (uid, pid and, with dbus-broker, a
pidfd), then the executable that pid runs, read with the process pinned (the pidfd's
pid, or the process start time, before and after). The executable must be one of the
suite's, in the daemon's own folder; its name decides the scope (docs/API.md).
Unknown callers get `AccessDenied`.

This keeps out other users and programs that aren't part of the suite. As with any
same-user check, a program running as you could start one of the suite's binaries
itself; the boundary against that is the user account, as everywhere on the desktop.

## Verification codes

1. A notification arrives. `nyx-notifyd` (Notification Center's service) finds a probable
   code with its pattern and context rules and hands the notification to
   `Otp1.Offer`, unless the app is left out of history. Clipboard runs its own detector
   (`crates/nyx-clipboard-core/src/otp.rs`: a code word must be near; prices, order
   numbers, years, times and phone numbers don't count) and keeps only the code, the
   sending app, the sites the text names and its words for brand matching.
2. The code goes into a 64-byte buffer locked out of swap and zeroed when dropped; the
   process is marked not dumpable. It expires after 3 minutes (1–5 in Settings), on
   session lock (logind `Lock`), on sleep (`PrepareForSleep`) and on exit (SIGTERM).
3. It shows at the top of the popup with a countdown. It is **not** put on the
   clipboard unless you copy it; then it's marked sensitive and cleared after 30 s if
   it's still there.
4. Passwords' browser extension, on a one-time-code field, asks its daemon
   (`otp.match`), which asks Clipboard (`Otp1.Match`, Vault scope only) for codes that
   may be for the page: `high` when the notification names the site, `low` when it
   names none; a code naming another site isn't offered. Filling (`otp.take`) gives a
   code once; a `low` one only after you pick it in the menu, which says it's unsure.
   The extension fills the field; it never submits the form.
5. Notification Center masks codes in its stored history once they're ten minutes old
   (Settings → Keep verification codes turns that off), and its Copy button goes
   through Clipboard (sensitive, cleared).

Checked by `tests/codes.sh` (private bus) and the unit tests in `codes.rs`, `otp.rs`,
`secret.rs` and `peer.rs`.

## Pasting

The popup closes, then the daemon asks Hyprland to send Ctrl+V (Ctrl+Shift+V in a
terminal) to the focused window, as Omarchy's universal paste does. Nothing reads
other windows' contents.

## AI

Assistant actions run only when you pick one, on the Assistant's local model through
its private socket (0600). The text is marked as data, not instructions. Nothing is
sent anywhere else, and no model runs otherwise.
