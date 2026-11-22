#!/bin/sh
# Installs rvrs from the latest release.
#
#   curl -fsSL https://rvrs.dev/install.sh | sh
#   curl -fsSL https://rvrs.dev/install.sh | sh -s -- --without console
#
# It installs the full build, with every build feature, unless --without names some to leave out
# (docs/build-features.md). rvrs update keeps that choice, and a build feature added in a later
# version comes with the next update unless it is left out too.
#
# On a terminal it asks whether rvrs should also run as traceroute, ping, mtr and dig, and
# whether to set up tab completion. Without a terminal it asks nothing and does neither.
#
#   --without <features>  leave out build features, separated by commas: console, bgp, dns and more
#   --link                run rvrs as traceroute, ping, mtr and dig, without asking
#   --no-link             leave the system tools as they are, without asking
#   --yes                 answer yes to every question
#
# RVRS_INSTALL_DIR sets the folder (default ~/.local/bin).
# RVRS_INSTALL_ARCHIVE installs from a local .tar.gz, and RVRS_INSTALL_SUMS from a local
# SHA256SUMS, in place of the downloads.
set -eu

repo="rvrs-dev/cli"
dir="${RVRS_INSTALL_DIR:-$HOME/.local/bin}"
link=ask
completions=ask
without=""

# Every build feature, as crates/rvrs/src/features.rs lists them. A test there checks these two
# lines.
features="traceroute ping mtr dns bgp tcp mtu console"
inside_traceroute="tcp mtu"

usage() {
  echo "Usage: install.sh [--without <features>] [--link | --no-link] [--yes]"
  echo "  --without <features>  leave out build features, separated by commas: $(echo $features | tr ' ' ',')"
  echo "  --link                run rvrs as traceroute, ping, mtr and dig"
  echo "  --no-link             leave the system tools as they are"
  echo "  --yes                 answer yes to every question"
}

while [ $# -gt 0 ]; do
  case "$1" in
    --link) link=yes ;;
    --no-link) link=no ;;
    --yes | -y)
      [ "$link" = ask ] && link=yes
      completions=yes
      ;;
    --without)
      if [ $# -lt 2 ]; then
        echo "rvrs install: --without takes the build features to leave out, as in --without console." >&2
        exit 2
      fi
      without="$without,$2"
      shift
      ;;
    --without=*) without="$without,${1#--without=}" ;;
    -h | --help)
      usage
      exit 0
      ;;
    *)
      echo "rvrs install: install.sh has no option $1. See install.sh --help." >&2
      exit 2
      ;;
  esac
  shift
done

fail() {
  echo "rvrs install: $1" >&2
  exit 1
}

# "a", "a and b", "a, b and c", from words.
and_list() {
  echo "$@" | awk '{ for (i = 1; i <= NF; i++) printf "%s%s", $i, (i == NF ? "" : (i == NF - 1 ? " and " : ", ")) }'
}

# What to leave out: known names only, with what works inside traceroute when it goes, sorted.
left=""
for name in $(echo "$without" | tr ',' ' '); do
  name="$(echo "$name" | tr '[:upper:]' '[:lower:]')"
  case " $features " in
    *" $name "*) left="$left $name" ;;
    *)
      echo "rvrs install: rvrs has no build feature $name. The build features are $(and_list $features)." >&2
      exit 2
      ;;
  esac
done
case " $left " in *" traceroute "*) left="$left $inside_traceroute" ;; esac
left="$(printf '%s\n' $left | sort -u | tr '\n' ' ' | sed 's/ *$//')"
has() {
  case " $left " in *" $1 "*) return 1 ;; *) return 0 ;; esac
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
  x86_64 | amd64) target="x86_64-$os" ;;
  arm64 | aarch64) target="aarch64-$os" ;;
  armv7*)
    [ "$os" = "unknown-linux-musl" ] || fail "There is no rvrs build for $(uname -m) on $(uname -s)."
    target="armv7-unknown-linux-musleabihf"
    ;;
  *) fail "There is no rvrs build for $(uname -m)." ;;
esac

if [ -z "$left" ]; then
  archive="rvrs-$target.tar.gz"
else
  archive="rvrs-$target-without-$(echo "$left" | tr ' ' '-').tar.gz"
fi
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

# The checksums first: they also say which builds the release has.
base="https://github.com/$repo/releases/latest/download"
if [ -n "${RVRS_INSTALL_SUMS:-}" ]; then
  cp "$RVRS_INSTALL_SUMS" "$work/SHA256SUMS"
elif [ -z "${RVRS_INSTALL_ARCHIVE:-}" ]; then
  fetch "$base/SHA256SUMS" "$work/SHA256SUMS" || fail "Couldn't download $base/SHA256SUMS."
fi

# The checksum the release lists for a file, or nothing.
listed() {
  awk -v name="$1" '{ file = $2; sub(/^\*/, "", file) } file == name { print tolower($1) }' "$work/SHA256SUMS"
}

if [ -f "$work/SHA256SUMS" ] && [ -z "$(listed "$archive")" ]; then
  # Say which builds there are, as rvrs update does.
  builds="$(awk -v full="rvrs-$target.tar.gz" -v prefix="rvrs-$target-without-" '
    { file = $2; sub(/^\*/, "", file) }
    file == full { print "0 the full build"; next }
    index(file, prefix) == 1 && file ~ /\.tar\.gz$/ {
      names = substr(file, length(prefix) + 1); sub(/\.tar\.gz$/, "", names)
      n = split(names, part, "-"); text = ""
      for (i = 1; i <= n; i++) text = text part[i] (i == n ? "" : (i == n - 1 ? " and " : ", "))
      print "1 the one without " text
    }' "$work/SHA256SUMS" | sort | cut -c3- | awk '{ line[NR] = $0 } END { for (i = 1; i <= NR; i++) printf "%s%s", line[i], (i == NR ? "" : (i == NR - 1 ? " and " : ", ")) }')"
  [ -n "$builds" ] || fail "The release has no build for $target."
  fail "The release has no build without $(and_list $left) for $target. It has $builds."
fi

if [ -n "${RVRS_INSTALL_ARCHIVE:-}" ]; then
  cp "$RVRS_INSTALL_ARCHIVE" "$work/$archive"
else
  fetch "$base/$archive" "$work/$archive" || fail "Couldn't download $base/$archive."
fi

# The archive has to match the checksum the release lists for it.
if [ -f "$work/SHA256SUMS" ]; then
  expected="$(listed "$archive")"
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

# Running rvrs as the original tools: a link per tool the build has, never over a real file.
linked=""
for tool in traceroute ping mtr dig; do
  feature="$tool"
  [ "$tool" = dig ] && feature=dns
  has "$feature" || continue
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
  names="$(and_list $shadowed)"
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
