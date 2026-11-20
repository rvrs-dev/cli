#!/bin/sh
# Installs rvrs from the latest release.
#
#   curl -fsSL https://rvrs.dev/install.sh | sh
#   curl -fsSL https://rvrs.dev/install.sh | sh -s -- --link
#
# --link also links traceroute, ping, mtr and dig to rvrs, next to it.
# RVRS_INSTALL_DIR sets the folder (default ~/.local/bin).
# RVRS_INSTALL_ARCHIVE installs from a local .tar.gz instead of downloading one.
set -eu

repo="rvrs-dev/cli"
dir="${RVRS_INSTALL_DIR:-$HOME/.local/bin}"
link=0

for arg in "$@"; do
  case "$arg" in
    --link) link=1 ;;
    -h | --help)
      echo "Usage: install.sh [--link]"
      echo "  --link   also link traceroute, ping, mtr and dig to rvrs"
      exit 0
      ;;
    *)
      echo "rvrs install: unknown option $arg" >&2
      exit 2
      ;;
  esac
done

fail() {
  echo "rvrs install: $1" >&2
  exit 1
}

case "$(uname -s)" in
  Darwin) os="apple-darwin" ;;
  Linux) os="unknown-linux-musl" ;;
  *) fail "rvrs runs on macOS and Linux, and this is $(uname -s)." ;;
esac

case "$(uname -m)" in
  x86_64 | amd64) arch="x86_64" ;;
  arm64 | aarch64) arch="aarch64" ;;
  *) fail "There is no rvrs build for $(uname -m)." ;;
esac

target="$arch-$os"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

if [ -n "${RVRS_INSTALL_ARCHIVE:-}" ]; then
  cp "$RVRS_INSTALL_ARCHIVE" "$work/rvrs.tar.gz"
else
  url="https://github.com/$repo/releases/latest/download/rvrs-$target.tar.gz"
  if command -v curl >/dev/null 2>&1; then
    curl -fsSL "$url" -o "$work/rvrs.tar.gz" || fail "Couldn't download $url."
  elif command -v wget >/dev/null 2>&1; then
    wget -qO "$work/rvrs.tar.gz" "$url" || fail "Couldn't download $url."
  else
    fail "Installing needs curl or wget."
  fi
fi

tar -xzf "$work/rvrs.tar.gz" -C "$work" || fail "Couldn't unpack the download."
[ -f "$work/rvrs" ] || fail "The download has no rvrs binary in it."

mkdir -p "$dir"
install -m 755 "$work/rvrs" "$dir/rvrs"
echo "Installed $("$dir/rvrs" --version) to $dir/rvrs"

if [ "$link" = 1 ]; then
  for tool in traceroute ping mtr dig; do
    if [ -e "$dir/$tool" ] && [ ! -L "$dir/$tool" ]; then
      echo "Skipped $tool: $dir/$tool is a file that isn't a link. Remove it and run the installer again." >&2
      continue
    fi
    ln -sf rvrs "$dir/$tool"
    echo "Linked $dir/$tool to rvrs"
  done
fi

case ":$PATH:" in
  *":$dir:"*) ;;
  *) echo "Add $dir to your PATH to run rvrs: export PATH=\"$dir:\$PATH\"" ;;
esac

if [ "$link" = 1 ]; then
  for tool in traceroute ping mtr dig; do
    found="$(command -v "$tool" 2>/dev/null || true)"
    if [ -n "$found" ] && [ "$found" != "$dir/$tool" ]; then
      echo "$tool still runs $found, because it comes first on your PATH. Put $dir before it to use rvrs."
    fi
  done
fi

if [ "$os" = "unknown-linux-musl" ]; then
  echo "rvrs status checks that this device allows the ICMP sockets rvrs needs."
fi
