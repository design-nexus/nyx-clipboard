#!/usr/bin/env bash
# The daemon on a private session bus with scratch XDG folders and no compositor:
# callers outside the suite are refused, the suite's own programs (built next to the
# daemon) are answered, and verification codes stay out of everything but Otp1.
#
#   cargo build -p nyx-clipboardd -p nyx-clipboard && tests/dbus.sh
set -euo pipefail
here=$(cd "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
bin="${CARGO_TARGET_DIR:-$here/target}/debug"
[[ -x $bin/nyx-clipboardd && -x $bin/nyx-clipboard ]] || { echo "build first: cargo build -p nyx-clipboardd -p nyx-clipboard" >&2; exit 2; }

scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT
export XDG_CONFIG_HOME=$scratch/config XDG_DATA_HOME=$scratch/data XDG_STATE_HOME=$scratch/state XDG_CACHE_HOME=$scratch/cache
export WAYLAND_DISPLAY=nyx-clipboard-test-none HYPRLAND_INSTANCE_SIGNATURE=none
mkdir -p "$XDG_STATE_HOME/omarchy"
printf '%s' '["imported from omarchy", {"type":"text","text":"ghp_abcdefghijklmnopqrstuvwxyz0123456789"}]' >"$XDG_STATE_HOME/omarchy/clipboard-history.json"

exec dbus-run-session -- bash -s "$bin" <<'INNER'
set -euo pipefail
bin=$1
fail() { echo "FAIL: $*" >&2; exit 1; }
"$bin/nyx-clipboardd" 2>"$XDG_STATE_HOME/daemon.log" &
pid=$!
trap 'kill $pid 2>/dev/null || true' EXIT
for _ in $(seq 50); do busctl --user status io.github.design_nexus.nyx.Clipboard1 >/dev/null 2>&1 && break; sleep 0.1; done
busctl --user status io.github.design_nexus.nyx.Clipboard1 >/dev/null 2>&1 || fail "the daemon didn't take its name"
B=(io.github.design_nexus.nyx.Clipboard1 /io/github/design_nexus/nyx/Clipboard1)

# Strangers are refused on every interface.
for call in "io.github.design_nexus.nyx.Clipboard1 Status" "io.github.design_nexus.nyx.Clipboard1 Search s ''" \
  "io.github.design_nexus.nyx.Clipboard.Otp1 Current" \
  "io.github.design_nexus.nyx.Clipboard.Otp1 Offer sss Code 'Your code is 483920' Mail" \
  "io.github.design_nexus.nyx.Clipboard.Otp1 Match s https://github.com" \
  "io.github.design_nexus.nyx.Clipboard.Actions1 List x 1"; do
  out=$(eval busctl --user call "${B[@]}" $call 2>&1 || true)
  grep -qiE "access denied|only answers Nyx apps" <<<"$out" || fail "busctl got an answer to $call: $out"
done
echo "ok: callers outside the suite are refused"

# The suite's popup binary is answered; Omarchy's history came in, its token didn't.
sleep 0.5
list=$("$bin/nyx-clipboard" --list "")
grep -q "imported from omarchy" <<<"$list" || fail "Omarchy's history wasn't imported: $list"
grep -q "ghp_" <<<"$list" && fail "a token from Omarchy's history was stored"
echo "ok: Omarchy's history imported once, without secrets"

# Nothing about codes is in the database.
db="$XDG_DATA_HOME/nyx/clipboard.db"
[[ -f $db ]] || fail "no database"
[[ $(stat -c %a "$db") == 600 ]] || fail "the database isn't private"
grep -qa "483920" "$db" && fail "a code reached the database"
echo "ok: the database is private and holds no codes"
grep -q "can't watch the clipboard" "$XDG_STATE_HOME/daemon.log" || fail "no compositor, but no complaint"
echo "ok: without a compositor the service still answers"
INNER
