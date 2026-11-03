#!/usr/bin/env bash
# Install Nyx Clipboard.
#
#   curl -fsSL https://raw.githubusercontent.com/design-nexus/nyx-clipboard/main/install.sh | bash
#
# Two user services: nyx-clipboardd keeps the clipboard history (and verification codes,
# in memory only), and the popup opens with Super + Ctrl + V (`nyx-clipboard --toggle`).
# NYX_SKIP_INTEGRATION=1 leaves the Hyprland shortcut alone.
set -euo pipefail

REPO="design-nexus/nyx-clipboard"
say() { printf '\033[1m==>\033[0m %s\n' "$*"; }
warn() { printf '\033[33m==>\033[0m %s\n' "$*" >&2; }
die() { printf '\033[31m==>\033[0m %s\n' "$*" >&2; exit 1; }

command -v hyprctl >/dev/null || warn "Hyprland wasn't found. Nyx Clipboard is made for Omarchy (Hyprland)."

if command -v pacman >/dev/null; then
  need=()
  for p in gtk4 gtk4-layer-shell pkgconf gcc git; do pacman -Qq "$p" >/dev/null 2>&1 || need+=("$p"); done
  if ((${#need[@]})); then
    say "Installing ${need[*]}"
    sudo pacman -S --needed --noconfirm "${need[@]}"
  fi
fi

# This checkout, or a fresh one when piped from curl.
here=""
[[ -n ${BASH_SOURCE[0]:-} && -f ${BASH_SOURCE[0]} ]] && here=$(cd "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
if [[ -n $here && -f $here/Cargo.toml && -d $here/crates ]]; then
  src=$here
else
  say "Fetching the source"
  git clone -q --depth 1 "https://github.com/$REPO.git" "$work/src"
  src="$work/src"
fi

bins=(nyx-clipboard nyx-clipboardd nyx-clipboard-settings)
if [[ ${NYX_SKIP_BUILD:-0} != 1 && ! -f $src/.prebuilt ]]; then
  if ! command -v cargo >/dev/null; then
    command -v pacman >/dev/null || die "Rust (cargo) is needed to build Nyx Clipboard. Install it and run this again."
    say "Installing rust"
    sudo pacman -S --needed --noconfirm rust
  fi
  say "Building Nyx Clipboard (a few minutes the first time)"
  (cd "$src" && CARGO_NET_GIT_FETCH_WITH_CLI=true cargo build --release --locked -p nyx-clipboard -p nyx-clipboardd -p nyx-clipboard-settings)
fi

bin="$HOME/.local/bin"
config="${XDG_CONFIG_HOME:-$HOME/.config}"
share="${XDG_DATA_HOME:-$HOME/.local/share}"
units="$config/systemd/user"
dbus="$share/dbus-1/services"
apps="$share/applications"
icons="$share/icons/hicolor/scalable/apps"
mkdir -p "$bin" "$units" "$dbus" "$apps" "$icons"
target="${CARGO_TARGET_DIR:-$src/target}/release"
# Replace the files rather than writing into them, so running copies keep their old
# binary until they restart.
for b in "${bins[@]}"; do
  install -m 755 "$target/$b" "$bin/$b.new" && mv -f "$bin/$b.new" "$bin/$b"
done
install -m 644 "$src/data/systemd/nyx-clipboardd.service" "$src/data/systemd/nyx-clipboard.service" "$units/"
install -m 644 "$src/data/dbus/io.github.design_nexus.nyx.Clipboard1.service" "$dbus/"
install -m 644 "$src/data/desktop/io.github.design_nexus.nyx.Clipboard.desktop" "$apps/"
install -m 644 "$src/data/icons/io.github.design_nexus.nyx.Clipboard.svg" "$icons/"
update-desktop-database "$apps" >/dev/null 2>&1 || true
gtk-update-icon-cache -q -t "$share/icons/hicolor" >/dev/null 2>&1 || true

# Clipboard's page in Nyx Settings (Settings → Desktop → Clipboard).
for host in nyx-settings settings; do
  ext="$share/$host/extensions/clipboard"
  mkdir -p "$ext"
  install -m 644 "$src/data/settings/extension.toml" "$ext/extension.toml"
done

systemctl --user daemon-reload 2>/dev/null || true
# The history service has no window, so it simply restarts on the new build.
systemctl --user enable nyx-clipboardd.service 2>/dev/null || true
systemctl --user restart nyx-clipboardd.service 2>/dev/null || true
# The popup is hidden between uses, so restarting it loses nothing.
systemctl --user enable nyx-clipboard.service 2>/dev/null || true
systemctl --user restart nyx-clipboard.service 2>/dev/null || true

# Super + Ctrl + V, in a block Clipboard owns in hyprland.lua (Settings changes the key).
if [[ ${NYX_SKIP_INTEGRATION:-0} != 1 && -f $config/hypr/hyprland.lua ]]; then
  "$bin/nyx-clipboard" --integrate || warn "The shortcut wasn't set. Settings → Desktop → Clipboard can set it."
fi

say "Installed. Open it with Super + Ctrl + V (or: nyx-clipboard --toggle)"
case ":$PATH:" in
  *":$bin:"*) ;;
  *) warn "$bin isn't on your PATH." ;;
esac
