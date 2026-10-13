#!/usr/bin/env bash
# Resolves every host in scripts/damai-hosts.txt to 0.0.0.0, so nothing run
# after this reaches the real damai.cn. CI runs it before `scripts/check.sh`
# on Linux and macOS (automated tests and CI never touch the real damai.cn;
# see docs/decisions/0003-tests-cannot-reach-damai.md).
#
# HOSTS_FILE overrides /etc/hosts; the script's own test uses that.
set -euo pipefail
cd "$(dirname "$0")/.."

hosts_file="${HOSTS_FILE:-/etc/hosts}"
list=scripts/damai-hosts.txt

hosts=()
while IFS= read -r line; do
  line="${line%%#*}"
  line="$(echo "$line" | tr -d '[:space:]')"
  [ -n "$line" ] && hosts+=("$line")
done <"$list"
[ "${#hosts[@]}" -gt 0 ] || { echo "$list names no host" >&2; exit 1; }

write() {
  if [ -w "$hosts_file" ]; then
    cat >>"$hosts_file"
  else
    sudo tee -a "$hosts_file" >/dev/null
  fi
}

missing=()
for h in "${hosts[@]}"; do
  grep -Eq "^0\.0\.0\.0[[:space:]]+${h//./\\.}([[:space:]]|\$)" "$hosts_file" 2>/dev/null || missing+=("$h")
done
for h in "${missing[@]+"${missing[@]}"}"; do
  echo "0.0.0.0 $h" | write
done

# Only the real /etc/hosts is worth asking the resolver about.
if [ -z "${HOSTS_FILE:-}" ]; then
  if [ "$(uname -s)" = Darwin ]; then
    sudo dscacheutil -flushcache || true
    sudo killall -HUP mDNSResponder || true
  fi
  for h in "${hosts[@]}"; do
    if [ "$(uname -s)" = Darwin ]; then
      answer="$(dscacheutil -q host -a name "$h")"
    else
      answer="$(getent ahostsv4 "$h" || true)"
    fi
    if ! grep -q '0\.0\.0\.0' <<<"$answer"; then
      echo "$h still resolves to something else:" >&2
      echo "$answer" >&2
      exit 1
    fi
  done
fi

echo "blocked ${#hosts[@]} damai hosts in $hosts_file"
