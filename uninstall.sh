#!/usr/bin/env bash
# Remove Nyx Clipboard.
#
#   ./uninstall.sh            remove the programs, their services and the shortcut
#   ./uninstall.sh --purge    also delete the clipboard history, snippets and settings
set -euo pipefail
say() { printf '\033[1m==>\033[0m %s\n' "$*"; }

bin="$HOME/.local/bin"
config="${XDG_CONFIG_HOME:-$HOME/.config}"
share="${XDG_DATA_HOME:-$HOME/.local/share}"
cache="${XDG_CACHE_HOME:-$HOME/.cache}"

# Give Super + Ctrl + V back to Omarchy's clipboard panel.
if [[ -x $bin/nyx-clipboard ]]; then
  "$bin/nyx-clipboard" --unintegrate || true
fi
systemctl --user disable --now nyx-clipboard.service nyx-clipboardd.service 2>/dev/null || true
rm -f "$share/nyx/actions/io.github.design_nexus.nyx.Clipboard.json"
rm -f "$bin/nyx-clipboard" "$bin/nyx-clipboardd" "$bin/nyx-clipboard-settings" \
  "$config/systemd/user/nyx-clipboard.service" "$config/systemd/user/nyx-clipboardd.service" \
  "$share/dbus-1/services/io.github.design_nexus.nyx.Clipboard1.service" \
  "$share/applications/io.github.design_nexus.nyx.Clipboard.desktop" \
  "$share/icons/hicolor/scalable/apps/io.github.design_nexus.nyx.Clipboard.svg"
rm -rf "$share/nyx-settings/extensions/clipboard" "$share/settings/extensions/clipboard"
systemctl --user daemon-reload 2>/dev/null || true
if [[ ${1:-} == --purge ]]; then
  rm -f "$share/nyx/clipboard.db" "$share/nyx/clipboard.db-wal" "$share/nyx/clipboard.db-shm"
  rm -rf "$share/nyx-clipboard" "$config/nyx-clipboard" "$cache/nyx-clipboard"
fi
say "Nyx Clipboard removed. Omarchy's own clipboard history is unchanged."
