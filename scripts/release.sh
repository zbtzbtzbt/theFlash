#!/usr/bin/env bash
# The steps of a release that decide what gets published. The release
# workflow (.github/workflows/release.yml) runs them; backend/tests/release.rs
# tests them. The rules are in docs/code-standards.md (发版), the reasons in
# docs/decisions/0006-release-cli-binaries.md.
#
#   release.sh gate <tag> [<commit>]
#       May <tag> be released? Exits 1 at the first rule it breaks, so
#       nothing is published. Without <commit> the tag must exist.
#   release.sh version <commit>
#       The version in backend/Cargo.toml at <commit>.
#   release.sh notes <tag> <commit>
#       Release notes: the commit titles since the last final release.
#   release.sh build <target>
#       Builds frontend assets (when npm is present) and the release binary.
#   release.sh package <target> <version> <binary>
#       Checks one built binary (version and embedded UI `serve`) and packs it
#       into dist/ with its checksum.
#   release.sh stage <dir>
#       Adds install.sh to <dir> and checks <dir> holds every asset.
#   release.sh publish <tag> <commit> <prerelease> <dir> <notes-file>
#       Uploads <dir> as a draft release, checks it, then makes it public.
#
# Run from the repository root of a full clone. gate and publish reach GitHub
# through gh (authenticated by GH_TOKEN) for the repository in
# GITHUB_REPOSITORY; they need jq.
set -euo pipefail

# The platforms a release has a build for, as Rust target triples. The build
# matrix in release.yml lists the same ones; a release whose files differ
# from assets() fails before it is made public.
TARGETS="aarch64-apple-darwin x86_64-apple-darwin x86_64-unknown-linux-musl"

assets() {
  local t
  for t in $TARGETS; do
    echo "theflash-$t.tar.gz"
    echo "theflash-$t.tar.gz.sha256"
  done
  echo install.sh
  echo install.sh.sha256
}

refuse() {
  echo "拒绝发布：$*" >&2
  exit 1
}

sha256() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | cut -d' ' -f1
  else
    shasum -a 256 "$1" | cut -d' ' -f1
  fi
}

# `<hash>  <name>`, the line `sha256sum -c` and `shasum -a 256 -c` read.
checksum_file() {
  (cd "$(dirname "$1")" && echo "$(sha256 "$(basename "$1")")  $(basename "$1")" >"$(basename "$1").sha256")
}

version() {
  local commit=${1:?usage: release.sh version <commit>}
  git show "$commit:backend/Cargo.toml" | awk '
    /^\[/ { pkg = ($0 == "[package]"); next }
    pkg && /^version[ \t]*=/ {
      sub(/^version[ \t]*=[ \t]*"/, ""); sub(/".*/, ""); print; exit
    }'
}

gate() {
  local tag=${1:?usage: release.sh gate <tag> [<commit>]} commit=${2:-}
  local repo=${GITHUB_REPOSITORY:?GITHUB_REPOSITORY is not set}
  local version prerelease=false tagged cargo runs verdicts status conclusion url

  [[ $tag =~ ^v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(-rc\.[1-9][0-9]*)?$ ]] ||
    refuse "tag $tag 的格式不对：正式版是 vX.Y.Z，预发布是 vX.Y.Z-rc.N（N 从 1 开始）"
  version=${tag#v}
  [[ $tag != *-rc.* ]] || prerelease=true

  if [ -n "$commit" ]; then
    [[ $commit =~ ^[0-9a-f]{40}$ ]] || refuse "commit 要写完整的 40 位 SHA，不是 $commit"
    git cat-file -e "$commit^{commit}" 2>/dev/null || refuse "仓库里没有 commit $commit"
  fi
  if tagged=$(git rev-parse -q --verify "refs/tags/$tag^{commit}"); then
    [ -z "$commit" ] || [ "$commit" = "$tagged" ] ||
      refuse "tag $tag 已经指向 ${tagged}，不是 $commit"
    commit=$tagged
  elif [ -z "$commit" ]; then
    refuse "tag $tag 不存在；要从一个 commit 发布，就写上它的完整 SHA"
  fi
  echo "tag $tag → commit $commit"

  git rev-parse -q --verify refs/remotes/origin/main >/dev/null ||
    refuse "没有 origin/main，先 git fetch origin main"
  git merge-base --is-ancestor "$commit" refs/remotes/origin/main ||
    refuse "commit $commit 不在 main 上（不是 origin/main 的祖先）"
  echo "在 main 上：是 origin/main 的祖先"

  cargo=$(version "$commit")
  [ "$cargo" = "$version" ] ||
    refuse "commit $commit 上 backend/Cargo.toml 的版本是 ${cargo:-（没写）}，和 tag $tag 不一致：先在 Cargo.toml 里改版本号并合进 main"
  echo "版本：backend/Cargo.toml 是 $cargo"

  runs=$(gh api "repos/$repo/actions/workflows/test.yml/runs?head_sha=$commit&per_page=100") ||
    refuse "读不到 commit $commit 上 Test 工作流的 run"
  # Only a run that tested exactly this commit counts: a push to main, or a
  # manual run. A pull request's run tests the PR merged into its base.
  verdicts=$(jq -r --arg sha "$commit" '
    .workflow_runs[]
    | select(.head_sha == $sha and (.event == "push" or .event == "workflow_dispatch"))
    | "\(.status) \(.conclusion // "none") \(.html_url)"' <<<"$runs")
  [ -n "$verdicts" ] ||
    refuse "commit $commit 上没有跑过 Test 工作流（只算 push 到 main 和手动触发的 run，PR 的 run 测的是合并后的结果）"
  # Every run must have finished green: one still running, cancelled or red
  # stops the release, even if another run passed.
  while read -r status conclusion url; do
    [ "$status" = completed ] || refuse "commit $commit 上的 Test 还没跑完（${status}）：$url"
    [ "$conclusion" = success ] || refuse "commit $commit 上的 Test 没有通过（${conclusion}）：$url"
    echo "Test 跑完并且是绿的：$url"
  done <<<"$verdicts"

  if [ -n "${GITHUB_OUTPUT:-}" ]; then
    {
      echo "tag=$tag"
      echo "commit=$commit"
      echo "version=$version"
      echo "prerelease=$prerelease"
    } >>"$GITHUB_OUTPUT"
  fi
  echo "可以发布 $tag"
}

notes() {
  local tag=${1:?usage: release.sh notes <tag> <commit>} commit=${2:?usage: release.sh notes <tag> <commit>}
  local repo=${GITHUB_REPOSITORY:?GITHUB_REPOSITORY is not set} prev kind=""
  # The newest final release (no -rc) before this one. A pre-release lists
  # everything since then too, so its notes are what the final release will say.
  prev=$(git tag --merged "$commit" --list 'v*' --sort=-v:refname |
    grep -E '^v[0-9]+\.[0-9]+\.[0-9]+$' | grep -vxF "$tag" | head -n 1 || true)
  [[ $tag != *-rc.* ]] || kind="，预发布"
  echo "theflash ${tag#v}${kind}。"
  echo
  echo "下载、校验和运行见 [README](https://github.com/$repo/blob/$tag/README.md#下载和运行)。每个文件的 SHA256 在同名的 \`.sha256\` 文件里。"
  echo
  if [ -n "$prev" ]; then
    echo "## 自 $prev 以来的改动"
    echo
    git log --no-merges --format='- %s (%h)' "$prev..$commit"
  else
    echo "## 改动"
    echo
    git log --no-merges --format='- %s (%h)' "$commit"
  fi
}

build() {
  local target=${1:?usage: release.sh build <target>}
  if command -v npm >/dev/null 2>&1 && [ -f frontend/package.json ]; then
    (cd frontend && { [ -d node_modules ] || npm ci --no-audit --no-fund; } && npm run build)
  fi
  (cd backend && cargo build --release --locked --target "$target")
}

package() {
  local usage="usage: release.sh package <target> <version> <binary>"
  local target=${1:?$usage} version=${2:?$usage} bin=${3:?$usage} want desc got help_out name
  case $target in
    aarch64-apple-darwin) want="Mach-O 64-bit executable arm64" ;;
    x86_64-apple-darwin) want="Mach-O 64-bit executable x86_64" ;;
    x86_64-unknown-linux-musl) want="ELF 64-bit LSB" ;;
    *) refuse "产物清单里没有 ${target}（见 release.sh 的 TARGETS）" ;;
  esac
  [ "$(basename "$bin")" = theflash ] || refuse "$bin 不叫 theflash"
  desc=$(file -b "$bin")
  [[ $desc == *"$want"* ]] || refuse "$bin 不是 $target 的程序：$desc"
  # The Linux build must not need any library of the machine it runs on.
  [[ $target != *linux* ]] || [[ $desc == *x86-64* && $desc == *static* ]] ||
    refuse "$bin 不是静态链接的 x86-64 程序：$desc"
  # An Intel build runs under Rosetta on Apple silicon.
  if [ "$target" = x86_64-apple-darwin ] && [ "$(uname -m)" = arm64 ]; then
    got=$(arch -x86_64 "$bin" --version)
    help_out=$(arch -x86_64 "$bin" --help 2>&1 || true)
  else
    got=$("$bin" --version)
    help_out=$("$bin" --help 2>&1 || true)
  fi
  [ "$got" = "theflash $version" ] || refuse "$bin --version 输出的是 ${got}，应该是 theflash ${version}"
  [[ $help_out == *"theflash serve"* ]] || refuse "$bin 没有包含本地界面子命令 theflash serve"
  name=theflash-$target.tar.gz
  mkdir -p dist
  COPYFILE_DISABLE=1 tar --no-xattrs -czf "dist/$name" -C "$(dirname "$bin")" theflash
  checksum_file "dist/$name"
  echo "dist/${name}：${desc}；$got"
}

# <dir> holds exactly the assets, and each file matches its .sha256.
check_dir() {
  local dir=$1 want got name
  want=$(assets | sort)
  got=$(cd "$dir" && printf "%s\n" * | sort)
  [ "$got" = "$want" ] || refuse "$dir 里的文件和产物清单不一致：
$(diff <(echo "$want") <(echo "$got") || true)"
  for name in $(assets | grep -v '\.sha256$'); do
    [ "$(cat "$dir/$name.sha256")" = "$(sha256 "$dir/$name")  $name" ] ||
      refuse "$name 和 $name.sha256 对不上"
  done
  echo "$dir 里有全部 $(assets | wc -l | tr -d ' ') 个产物，校验和都对"
}

stage() {
  local dir=${1:?usage: release.sh stage <dir>}
  cp scripts/install.sh "$dir/install.sh"
  checksum_file "$dir/install.sh"
  check_dir "$dir"
}

# The release on GitHub holds every asset, fully uploaded, the same size and
# (when GitHub reports it) the same SHA256 as the file built here.
check_release() {
  local tag=$1 dir=$2 draft=$3 json name state size digest
  json=$(gh release view "$tag" --json isDraft,assets)
  [ "$(jq -r .isDraft <<<"$json")" = "$draft" ] || refuse "$tag 的 isDraft 应该是 $draft"
  [ "$(jq -r '.assets[].name' <<<"$json" | sort)" = "$(assets | sort)" ] ||
    refuse "$tag 上的文件和产物清单不一致：$(jq -r '[.assets[].name] | join(" ")' <<<"$json")"
  for name in $(assets); do
    read -r state size digest < <(jq -r --arg n "$name" \
      '.assets[] | select(.name == $n) | "\(.state) \(.size) \(.digest // "-")"' <<<"$json")
    [ "$state" = uploaded ] || refuse "$tag 的 $name 没有上传完（${state}）"
    [ "$size" = "$(wc -c <"$dir/$name" | tr -d ' ')" ] || refuse "$tag 的 $name 大小是 ${size}，和构建出来的不一样"
    [ "$digest" = - ] || [ "$digest" = "sha256:$(sha256 "$dir/$name")" ] ||
      refuse "$tag 的 $name 校验和是 ${digest}，和构建出来的不一样"
  done
  echo "$tag 上有全部 $(assets | wc -l | tr -d ' ') 个产物，大小和校验和都和构建出来的一致"
}

publish() {
  local usage="usage: release.sh publish <tag> <commit> <prerelease> <dir> <notes-file>"
  local tag=${1:?$usage} commit=${2:?$usage} prerelease=${3:?$usage} dir=${4:?$usage} notes=${5:?$usage}
  local state at
  check_dir "$dir"
  state=$(gh release view "$tag" --json isDraft --jq 'if .isDraft then "draft" else "published" end' 2>/dev/null || true)
  case $state in
    published) refuse "$tag 已经发布过；发布过的 release 不覆盖，换一个版本号" ;;
    draft)
      # Left by an earlier attempt that failed before making it public.
      echo "删掉之前没发完的草稿 $tag"
      gh release delete "$tag" --yes
      ;;
  esac
  local args=(--draft --title "theflash ${tag#v}" --notes-file "$notes" --target "$commit")
  [ "$prerelease" != true ] || args+=(--prerelease)
  # shellcheck disable=SC2046 # one argument per asset; names have no spaces
  gh release create "$tag" "${args[@]}" $(assets | sed "s|^|$dir/|")
  check_release "$tag" "$dir" true
  gh release edit "$tag" --draft=false
  check_release "$tag" "$dir" false
  at=$(gh api "repos/${GITHUB_REPOSITORY:?GITHUB_REPOSITORY is not set}/commits/$tag" --jq .sha)
  [ "$at" = "$commit" ] || refuse "发布后 tag $tag 指向 ${at}，不是 $commit"
  echo "已发布 ${tag}：$(gh release view "$tag" --json url --jq .url)"
}

cmd=${1:-}
[ $# -eq 0 ] || shift
case $cmd in
  gate | version | notes | build | package | stage | publish) "$cmd" "$@" ;;
  *)
    sed -n '2,/^set -euo/p' "$0" | sed '$d' >&2
    exit 2
    ;;
esac
