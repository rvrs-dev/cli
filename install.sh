#!/bin/sh
# Installs rvrs from the latest release.
#
#   curl -fsSL https://rvrs.dev/install.sh | sh
#
# On a terminal it asks whether rvrs should also run as traceroute, ping, mtr and dig, and
# whether to set up tab completion. Without a terminal it asks nothing and does neither.
#
#   --link      run rvrs as traceroute, ping, mtr and dig, without asking
#   --no-link   leave the system tools as they are, without asking
#   --yes       answer yes to every question
#
# RVRS_INSTALL_DIR sets the folder (default ~/.local/bin).
# RVRS_INSTALL_ARCHIVE installs from a local .tar.gz, and RVRS_INSTALL_SUMS from a local
# SHA256SUMS, in place of the downloads.
set -eu

repo="rvrs-dev/cli"
dir="${RVRS_INSTALL_DIR:-$HOME/.local/bin}"
link=ask
completions=ask

for arg in "$@"; do
  case "$arg" in
    --link) link=yes ;;
    --no-link) link=no ;;
    --yes | -y)
      [ "$link" = ask ] && link=yes
      completions=yes
      ;;
    -h | --help)
      echo "Usage: install.sh [--link | --no-link] [--yes]"
      echo "  --link      run rvrs as traceroute, ping, mtr and dig"
      echo "  --no-link   leave the system tools as they are"
      echo "  --yes       answer yes to every question"
      exit 0
      ;;
    *)
      echo "rvrs install: install.sh has no option $arg. See install.sh --help." >&2
      exit 2
      ;;
  esac
done

fail() {
  echo "rvrs install: $1" >&2
  exit 1
}

# Questions go to the terminal, which under `curl … | sh` isn't stdin.
interactive=0
if [ -t 1 ] && { : </dev/tty; } 2>/dev/null; then
  interactive=1
fi

# ask "<question>" <default y|n>: true for yes.
ask() {
  if [ "$2" = y ]; then hint="[Y/n]"; else hint="[y/N]"; fi
  printf '%s %s ' "$1" "$hint"
  read -r answer </dev/tty || answer=""
  case "$answer" in
    [Yy]*) return 0 ;;
    [Nn]*) return 1 ;;
    *) [ "$2" = y ] ;;
  esac
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
archive="rvrs-$target.tar.gz"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

fetch() {
  if command -v curl >/dev/null 2>&1; then
    curl -fsSL "$1" -o "$2"
  elif command -v wget >/dev/null 2>&1; then
    wget -qO "$2" "$1"
  else
    fail "Installing needs curl or wget."
  fi
}

base="https://github.com/$repo/releases/latest/download"
if [ -n "${RVRS_INSTALL_ARCHIVE:-}" ]; then
  cp "$RVRS_INSTALL_ARCHIVE" "$work/$archive"
else
  fetch "$base/$archive" "$work/$archive" || fail "Couldn't download $base/$archive."
fi
if [ -n "${RVRS_INSTALL_SUMS:-}" ]; then
  cp "$RVRS_INSTALL_SUMS" "$work/SHA256SUMS"
elif [ -z "${RVRS_INSTALL_ARCHIVE:-}" ]; then
  fetch "$base/SHA256SUMS" "$work/SHA256SUMS" || fail "Couldn't download $base/SHA256SUMS."
fi

# The archive has to match the checksum the release lists for it.
if [ -f "$work/SHA256SUMS" ]; then
  expected="$(awk -v name="$archive" '{ file = $2; sub(/^\*/, "", file) } file == name { print tolower($1) }' "$work/SHA256SUMS")"
  [ -n "$expected" ] || fail "The release lists no checksum for $archive."
  if command -v sha256sum >/dev/null 2>&1; then
    actual="$(sha256sum "$work/$archive" | awk '{ print $1 }')"
  elif command -v shasum >/dev/null 2>&1; then
    actual="$(shasum -a 256 "$work/$archive" | awk '{ print $1 }')"
  else
    fail "Checking the download needs sha256sum or shasum."
  fi
  [ "$actual" = "$expected" ] || fail "$archive doesn't match its checksum. Nothing was installed."
fi

tar -xzf "$work/$archive" -C "$work" || fail "Couldn't unpack $archive."
[ -f "$work/rvrs" ] || fail "$archive has no rvrs binary in it."

mkdir -p "$dir"
install -m 755 "$work/rvrs" "$dir/rvrs"
echo "Installed $("$dir/rvrs" --version) in $dir"

# Running rvrs as the original tools: a link per tool, never over a real file.
linked=""
for tool in traceroute ping mtr dig; do
  case "$link" in
    yes) want=1 ;;
    no) want=0 ;;
    *)
      want=0
      if [ "$interactive" = 1 ]; then
        system="$(command -v "$tool" 2>/dev/null || true)"
        case "$system" in
          "" | "$dir/$tool") question="Use rvrs as $tool?" ;;
          *) question="Use rvrs as $tool, in place of $system?" ;;
        esac
        if ask "$question" n; then want=1; fi
      fi
      ;;
  esac
  [ "$want" = 1 ] || continue
  if [ -e "$dir/$tool" ] && [ ! -L "$dir/$tool" ]; then
    echo "Skipped $tool: $dir/$tool is a file of its own. Remove it and run the installer again." >&2
    continue
  fi
  ln -sf rvrs "$dir/$tool"
  linked="$linked $tool"
done
[ -z "$linked" ] || echo "Linked$linked to rvrs"

case ":$PATH:" in
  *":$dir:"*) ;;
  *) echo "Add $dir to your PATH: export PATH=\"$dir:\$PATH\"" ;;
esac

shadowed=""
for tool in $linked; do
  found="$(command -v "$tool" 2>/dev/null || true)"
  if [ -n "$found" ] && [ "$found" != "$dir/$tool" ]; then
    shadowed="$shadowed $tool"
  fi
done
if [ -n "$shadowed" ]; then
  names="$(echo $shadowed | awk '{ for (i = 1; i <= NF; i++) printf "%s%s", $i, (i == NF ? "" : (i == NF - 1 ? " and " : ", ")) }')"
  echo "Your shell still runs the system's $names. Put $dir before /usr/bin on your PATH."
fi

# Tab completion for the shell this person uses.
shell="$(basename "${SHELL:-}")"
case "$shell" in
  zsh | bash | fish)
    case "$completions" in
      yes) want=1 ;;
      *)
        want=0
        if [ "$interactive" = 1 ] && ask "Set up tab completion for $shell?" y; then want=1; fi
        ;;
    esac
    if [ "$want" = 1 ]; then
      "$dir/rvrs" completions install "$shell" || echo "Tab completion wasn't set up. Run rvrs completions install later." >&2
    fi
    ;;
esac

if [ "$os" = "unknown-linux-musl" ]; then
  echo "Check that this device allows the ICMP sockets rvrs needs: rvrs status"
fi
