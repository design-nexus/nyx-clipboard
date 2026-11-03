#!/usr/bin/env bash
# Verification-code security, end to end on a private session bus: the daemon and a test
# client copied beside it under the names of the programs it trusts (nyx-notifyd,
# nyx-passwordsd, nyx-clipboard, nyx-glimpse), plus busctl as a stranger.
#
#   cargo build -p nyx-clipboardd -p nyx-clipboard && cargo build -p nyx-clipboard-dbus --example call && tests/codes.sh
set -euo pipefail
here=$(cd "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
target="${CARGO_TARGET_DIR:-$here/target}/debug"
for f in nyx-clipboardd nyx-clipboard examples/call; do
  [[ -x $target/$f ]] || { echo "build first (see the top of this file)" >&2; exit 2; }
done

scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT
mkdir -p "$scratch/bin"
cp "$target/nyx-clipboardd" "$target/nyx-clipboard" "$scratch/bin/"
for name in nyx-notifyd nyx-passwordsd nyx-glimpse nyx-notification-center nyx-mail; do
  cp "$target/examples/call" "$scratch/bin/$name"
done
export XDG_CONFIG_HOME=$scratch/config XDG_DATA_HOME=$scratch/data XDG_STATE_HOME=$scratch/state XDG_CACHE_HOME=$scratch/cache
export WAYLAND_DISPLAY=nyx-clipboard-test-none HYPRLAND_INSTANCE_SIGNATURE=none

exec dbus-run-session -- bash -s "$scratch/bin" <<'INNER'
set -euo pipefail
bin=$1
fail() { echo "FAIL: $*" >&2; exit 1; }
"$bin/nyx-clipboardd" 2>"$XDG_STATE_HOME.log" &
pid=$!
trap 'kill $pid 2>/dev/null || true' EXIT
for _ in $(seq 50); do busctl --user status io.github.design_nexus.nyx.Clipboard1 >/dev/null 2>&1 && break; sleep 0.1; done
denied() { grep -qiE "AccessDenied|access denied" <<<"$1" || fail "$2: $1"; }
B=(io.github.design_nexus.nyx.Clipboard1 /io/github/design_nexus/nyx/Clipboard1 io.github.design_nexus.nyx.Clipboard.Otp1)

# Only the notification service may offer codes.
denied "$(busctl --user call "${B[@]}" Offer sss Code 'Your code is 111222' Mail 2>&1 || true)" "a stranger offered a code"
denied "$("$bin/nyx-mail" Otp1 Offer s:Code 's:Your code is 111222' s:Mail || true)" "another Nyx app offered a code"
out=$("$bin/nyx-notifyd" Otp1 Offer s:GitHub "s:Your GitHub launch code is 483920. https://github.com/sessions" s:Thunderbird)
grep -q "true" <<<"$out" || fail "the notification service's code wasn't taken: $out"
"$bin/nyx-notifyd" Otp1 Offer s:Weather "s:Sunny, 21 degrees" s:Weather | grep -q false || fail "a weather report became a code"
echo "ok: only nyx-notifyd offers codes, and only real codes are held"

# Nobody but the popup sees them; the history and its search never do.
denied "$(busctl --user call "${B[@]}" Current 2>&1 || true)" "a stranger read the codes"
denied "$("$bin/nyx-glimpse" Otp1 Current || true)" "Glimpse read the codes"
denied "$("$bin/nyx-passwordsd" Otp1 Current || true)" "the Vault read every code"
"$bin/nyx-glimpse" Clipboard1 Search 's:{"text":"483920"}' | grep -q 483920 && fail "a code is in search results"
"$bin/nyx-glimpse" Clipboard1 Search 's:' | grep -q 483920 && fail "a code is in the history"
codes=$("$bin/nyx-clipboard" --list "" 2>&1 || true)
grep -q 483920 <<<"$codes" && fail "a code reached the popup's history list"
echo "ok: codes aren't in the history, its search, or anyone's reach but the popup's"

# The Vault gets candidates (no codes) for the site the notification names, and a code once.
denied "$(busctl --user call "${B[@]}" Match s https://github.com/login 2>&1 || true)" "a stranger matched codes"
denied "$("$bin/nyx-glimpse" Otp1 Match s:https://github.com/login || true)" "Glimpse matched codes"
m=$("$bin/nyx-passwordsd" Otp1 Match s:https://github.com/login)
grep -q '"confidence":"high"' <<<"$m" || fail "no sure match for github.com: $m"
grep -q 483920 <<<"$m" && fail "Match gave away the code"
id=$(grep -o '"id":"c[0-9]*"' <<<"$m" | head -1 | cut -d'"' -f4)
[[ -n $(  "$bin/nyx-passwordsd" Otp1 Match s:https://evil.example/login | grep -o '"id"' || true) ]] && fail "a code was offered to another site"
denied "$("$bin/nyx-passwordsd" Otp1 Take "s:$id" s:https://evil.example/login b:true || true)" "a code went to another site"
t=$("$bin/nyx-passwordsd" Otp1 Take "s:$id" s:https://github.com/login b:false)
grep -q 483920 <<<"$t" || fail "the Vault didn't get the code for github.com: $t"
again=$("$bin/nyx-passwordsd" Otp1 Take "s:$id" s:https://github.com/login b:false || true)
grep -q 483920 <<<"$again" && fail "a code was given twice"
echo "ok: codes go to the Vault only for the site they name, once, and never in a list"

# A code that names no site needs the user's confirmation.
"$bin/nyx-notifyd" Otp1 Offer s:Messages "s:Your verification code is 552210" s:Messages >/dev/null
m=$("$bin/nyx-passwordsd" Otp1 Match s:https://bank.example/login)
grep -q '"confidence":"low"' <<<"$m" || fail "an unsure match wasn't marked low: $m"
id=$(grep -o '"id":"c[0-9]*"' <<<"$m" | head -1 | cut -d'"' -f4)
denied "$("$bin/nyx-passwordsd" Otp1 Take "s:$id" s:https://bank.example/login b:false || true)" "an unsure code went without confirmation"
"$bin/nyx-passwordsd" Otp1 Take "s:$id" s:https://bank.example/login b:true | grep -q 552210 || fail "a confirmed code wasn't given"
echo "ok: an unsure match needs confirmation"

# The Notification Center may copy a code it shows (no clipboard here, so it fails to copy,
# but it isn't refused); others may not.
denied "$(busctl --user call "${B[@]}" CopyCode s 483920 2>&1 || true)" "a stranger copied a code"
nc=$("$bin/nyx-notification-center" Otp1 CopyCode s:483920 || true)
grep -qiE "AccessDenied" <<<"$nc" && fail "the Notification Center was refused: $nc"
echo "ok: the Notification Center copies codes through the service"

# Nothing about codes on disk.
grep -rqa -e 483920 -e 552210 "$XDG_DATA_HOME" "$XDG_CONFIG_HOME" 2>/dev/null && fail "a code was written to disk"
echo "ok: no code was written to disk"
INNER
