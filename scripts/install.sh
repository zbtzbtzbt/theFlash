#!/bin/sh
# Installs theflash from a release of zbtzbtzbt/theFlash, or upgrades it:
# running it again installs the newest release over the old one.
#
#   sh install.sh               the newest release (pre-releases are skipped)
#   sh install.sh v0.1.0-rc.1   that release, pre-releases included
#
# The repository is private, so the download goes through GitHub CLI (gh),
# logged in as an account that can read it. Installs into
# $THEFLASH_INSTALL_DIR, ~/.local/bin by default, after checking the archive
# against its .sha256. Each release carries this script as an asset; see
# README.md (下载和运行).
set -eu

repo=zbtzbtzbt/theFlash
tag=${1:-}
dir=${THEFLASH_INSTALL_DIR:-$HOME/.local/bin}

fail() {
  echo "安装失败：$*" >&2
  exit 1
}

case $tag in
  "" | v[0-9]*) ;;
  *) fail "版本要写成 tag 的样子，例如 v0.1.0，不是 $tag" ;;
esac

case "$(uname -s) $(uname -m)" in
  "Darwin arm64") target=aarch64-apple-darwin ;;
  "Darwin x86_64") target=x86_64-apple-darwin ;;
  "Linux x86_64") target=x86_64-unknown-linux-musl ;;
  *) fail "没有给 $(uname -s) $(uname -m) 构建的版本，只有 macOS arm64、macOS x86_64 和 Linux x86_64" ;;
esac
archive=theflash-$target.tar.gz

command -v gh >/dev/null 2>&1 ||
  fail "要先装 GitHub CLI（https://cli.github.com）并用 gh auth login 登录：仓库是私有的，下载要用能读它的账号"

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

gh release download ${tag:+"$tag"} --repo "$repo" --dir "$tmp" \
  --pattern "$archive" --pattern "$archive.sha256" ||
  fail "从 $repo 下载 ${tag:-最新版本} 的 $archive 失败；gh auth status 可以看是否已登录"

cd "$tmp"
want=$(cut -d' ' -f1 <"$archive.sha256")
if command -v sha256sum >/dev/null 2>&1; then
  got=$(sha256sum "$archive" | cut -d' ' -f1)
else
  got=$(shasum -a 256 "$archive" | cut -d' ' -f1)
fi
[ -n "$want" ] && [ "$want" = "$got" ] ||
  fail "$archive 和 $archive.sha256 对不上，文件可能下载坏了；什么都没有安装"

tar -xzf "$archive"
[ -f theflash ] || fail "$archive 里没有 theflash"
chmod 755 theflash
mkdir -p "$dir"
# A new file moved over the old one: a theflash that is running (a watch,
# say) keeps running the old program instead of failing.
cp theflash "$dir/.theflash.new.$$"
mv -f "$dir/.theflash.new.$$" "$dir/theflash"

echo "已安装 $("$dir/theflash" --version)：$dir/theflash"
case ":$PATH:" in
  *":$dir:"*) ;;
  *) echo "$dir 不在 PATH 里：把 export PATH=\"$dir:\$PATH\" 加进 shell 的配置文件，或者用完整路径运行。" ;;
esac
