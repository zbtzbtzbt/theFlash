#!/usr/bin/env bash
# Resolves every host in scripts/damai-hosts.txt to 0.0.0.0, so nothing run
# after this reaches the real damai.cn. CI runs it before `scripts/check.sh`
# on Linux and macOS (automated tests and CI never touch the real damai.cn;
# see docs/decisions/0003-tests-cannot-reach-damai.md).
#
# It edits /etc/hosts only on a GitHub-hosted runner, which is thrown away
# after the job. Anywhere else it refuses: the block cuts the whole machine
# off from damai.cn, browsers and `theflash watch` included, and looks like a
# broken network (#22). Every line it writes is tagged, and
# scripts/unblock-damai-hosts.sh removes the tagged lines.
#
# HOSTS_FILE names a file to edit instead of /etc/hosts, without sudo;
# backend/tests/hosts_block.rs uses that.
set -euo pipefail
cd "$(dirname "$0")/.."

if [ -z "${HOSTS_FILE:-}" ] && [ "${RUNNER_ENVIRONMENT:-}" != github-hosted ]; then
  cat >&2 <<'EOF'
block-damai-hosts.sh: refusing to edit /etc/hosts, because this is not a
GitHub-hosted runner (RUNNER_ENVIRONMENT is not github-hosted).

The block cuts every program on the machine off from the real damai.cn,
browsers and `theflash watch` included, and the failures look like a broken
network. That is fine on a runner that is thrown away after the job, not on a
machine someone uses. `make check` does not need it: its tests cannot reach
damai.cn without it (docs/decisions/0003-tests-cannot-reach-damai.md).

If an earlier run blocked damai.cn on this machine,
scripts/unblock-damai-hosts.sh removes the lines that run wrote.
EOF
  exit 1
fi

hosts_file="${HOSTS_FILE:-/etc/hosts}"
list=scripts/damai-hosts.txt
# Ends every line this script writes. scripts/unblock-damai-hosts.sh removes
# the lines that end with it, so the two must stay the same.
tag='# theflash: scripts/block-damai-hosts.sh wrote this; scripts/unblock-damai-hosts.sh removes it'

hosts=()
while IFS= read -r line; do
  line="${line%%#*}"
  line="$(echo "$line" | tr -d '[:space:]')"
  [ -n "$line" ] && hosts+=("$line")
done <"$list"
[ "${#hosts[@]}" -gt 0 ] || { echo "$list names no host" >&2; exit 1; }

# The real /etc/hosts belongs to root, so it is only ever written through sudo.
append() {
  if [ -n "${HOSTS_FILE:-}" ]; then
    cat >>"$hosts_file"
  else
    sudo tee -a "$hosts_file" >/dev/null
  fi
}

missing=()
for h in "${hosts[@]}"; do
  grep -Eq "^0\.0\.0\.0[[:space:]]+${h//./\\.}([[:space:]]|\$)" "$hosts_file" 2>/dev/null || missing+=("$h")
done
if [ "${#missing[@]}" -gt 0 ]; then
  {
    # Without this, the first line would run into a last line that has no newline.
    if [ -s "$hosts_file" ] && [ -n "$(tail -c 1 "$hosts_file")" ]; then
      echo
    fi
    for h in "${missing[@]}"; do
      echo "0.0.0.0 $h $tag"
    done
  } | append
fi

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
