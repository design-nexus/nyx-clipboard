# Limitations

What Nyx Clipboard can't do, or does only as far as the compositor and other apps
allow. None of these is worked around with anything unsafe.

- **Wayland data-control.** Watching and serving the clipboard needs
  `ext-data-control-v1` or `wlr-data-control` (Hyprland has both). Without it the
  service still answers, and the popup says the clipboard isn't being watched.
- **X11.** There is no X11 backend. Hyprland keeps XWayland's clipboard in step with
  Wayland's, so X11 apps' copies are seen through it.
- **The primary selection** (middle-click) isn't recorded.
- **Excluded apps** are recognized by the window that has focus when the copy arrives
  (data-control doesn't say which app copied). A copy made by an app in the
  background is attributed to the focused one.
- **Pasting** sends Ctrl+V (Ctrl+Shift+V to windows Omarchy tags as terminals) through
  Hyprland. An app that pastes with another key, or a field that refuses synthetic
  keys, gets the content on the clipboard but no paste. Outside Hyprland it falls back
  to `wtype`.
- **Rich content:** one text, one HTML, one image and the file list are kept per copy.
  Other formats an app offers (RTF, app-private types) aren't.
- **Large copies:** above *Largest item* (10 MB by default) aren't kept. A copying app
  gets two seconds to hand over each representation.
- **Secret detection** is pattern-based. A short, ordinary-looking password won't be
  recognized; password managers' copies are recognized by their hint, which most send.
- **Omarchy's clipboard history** keeps running beside this one (by choice) with its
  own file, which Nyx Clipboard doesn't control.
- **Verification codes:**
  - They come from Nyx Notification Center's service (`nyx-notifyd`). Without it, or
    with Clipboard's code setting off, nothing is detected.
  - In Omarchy's notification mode (the default), Omarchy's shell keeps the
    notification text in `~/.local/state/omarchy/notifications/history.json` for a
    day; Nyx can't mask that copy. In Nyx mode, Notification Center's `Popup` signal
    still carries the text of the notification on the session bus while it's shown.
  - Origin matching uses the domains, sender addresses and brand names a notification
    mentions. A notification that names nothing is an unsure match and needs your pick.
  - Browser autofill needs Passwords' extension, paired, with the vault unlocked.
- **Assistant actions** need the Assistant running (they're hidden otherwise); they can
  be slow on a busy model and say so when it's in a voice turn.
- **Extract text** uses Capture's OCR (Tesseract); Capture reads PNG images.
- **Notes:** a new note goes through Share (a proposal you approve in Notes); *Add to a
  note* needs Notes open with a vault and its write permission on, and names the note
  by title or path.
- **Glimpse** shows text previews and kinds in its `cb` results, not image thumbnails.
- **The Assistant's voice command** "open clipboard" still opens Omarchy's menu: its
  command phrases can only run Omarchy commands, and changing that is up to the
  Assistant.
