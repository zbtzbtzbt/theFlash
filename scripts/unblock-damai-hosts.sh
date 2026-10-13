#!/usr/bin/env bash
# Undoes scripts/block-damai-hosts.sh: removes the lines it wrote to
# /etc/hosts, so this machine reaches the real damai.cn again (#22). Every
# other line stays as it is. Run it where browsers or `theflash watch` cannot
# reach damai.cn because the block ran there; it asks for sudo only when
# there is something to remove.
#
# HOSTS_FILE names a file to edit instead of /etc/hosts, without sudo;
# backend/tests/hosts_block.rs uses that.
set -euo pipefail
cd "$(dirname "$0")/.."

hosts_file="${HOSTS_FILE:-/etc/hosts}"
list=scripts/damai-hosts.txt
# Ends every line scripts/block-damai-hosts.sh writes; the two must stay the same.
tag='# theflash: scripts/block-damai-hosts.sh wrote this; scripts/unblock-damai-hosts.sh removes it'

hosts=""
while IFS= read -r line; do
  line="${line%%#*}"
  line="$(echo "$line" | tr -d '[:space:]')"
  [ -n "$line" ] && hosts="$hosts $line"
done <"$list"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

# The block's lines are `0.0.0.0 <host> <tag>`, whatever the host, since the
# list may have changed since. Before the tag (e860486), the block wrote
# `0.0.0.0 <host>` for the hosts of the list.
awk -v tag="$tag" -v hosts="$hosts" -v removed="$work/removed" '
  BEGIN { n = split(hosts, h, " "); for (i = 1; i <= n; i++) listed[h[i]] = 1 }
  ($0 == ("0.0.0.0 " $2 " " tag)) || ($0 == ("0.0.0.0 " $2) && ($2 in listed)) {
    print > removed
    next
  }
  { print }
' "$hosts_file" >"$work/kept"

if [ ! -s "$work/removed" ]; then
  echo "nothing to undo: no line of $hosts_file was written by scripts/block-damai-hosts.sh"
  exit 0
fi

# The real /etc/hosts belongs to root, so it is only ever written through sudo.
# Both keep the file itself, with its owner and mode, and replace what is in it.
if [ -n "${HOSTS_FILE:-}" ]; then
  cat "$work/kept" >"$hosts_file"
else
  sudo tee "$hosts_file" <"$work/kept" >/dev/null
fi

if [ -z "${HOSTS_FILE:-}" ] && [ "$(uname -s)" = Darwin ]; then
  sudo dscacheutil -flushcache || true
  sudo killall -HUP mDNSResponder || true
fi

echo "removed from $hosts_file:"
sed 's/^/  /' "$work/removed"
