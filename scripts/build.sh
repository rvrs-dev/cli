#!/bin/sh
# Builds rvrs for a target and packs its release archive into dist/.
#
#   sh scripts/build.sh x86_64-unknown-linux-musl                     the full build
#   sh scripts/build.sh x86_64-unknown-linux-musl --without console   the small build
#
# --without names modules to leave out (docs/modules.md), separated by commas. The archive is
# named the way the installer and rvrs update ask for it: rvrs-<target>.tar.gz for the full
# build, rvrs-<target>-without-<modules>.tar.gz otherwise, with the names sorted. A build without
# the Console uses the small profile, which optimizes for size. The last line says where the
# binary is and how big it is.
set -eu

[ $# -ge 1 ] || { echo "Usage: scripts/build.sh <target> [--without <modules>]" >&2; exit 2; }
target="$1"
shift
without=""
while [ $# -gt 0 ]; do
  case "$1" in
    --without) [ $# -ge 2 ] || { echo "build.sh: --without takes the modules to leave out." >&2; exit 2; }; without="$2"; shift 2 ;;
    --without=*) without="${1#--without=}"; shift ;;
    *) echo "build.sh: build.sh has no option $1." >&2; exit 2 ;;
  esac
done

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"
# The modules are the default features, on their one line in crates/rvrs/Cargo.toml.
modules="$(sed -n 's/^default = \[\(.*\)\]$/\1/p' crates/rvrs/Cargo.toml | tr -d '" ' | tr ',' ' ')"
[ -n "$modules" ] || { echo "build.sh: crates/rvrs/Cargo.toml names no default features." >&2; exit 1; }

left=""
for name in $(echo "$without" | tr ',' ' '); do
  case " $modules " in
    *" $name "*) left="$left $name" ;;
    *) echo "build.sh: rvrs has no module $name. The modules are $modules." >&2; exit 2 ;;
  esac
done
# The TCP probe and the path MTU check work inside traceroute (modules.rs, needs).
case " $left " in *" traceroute "*) left="$left tcp mtu" ;; esac
left="$(printf '%s\n' $left | sort -u | tr '\n' ' ' | sed 's/ *$//')"

if [ -z "$left" ]; then
  profile=release
  archive="rvrs-$target.tar.gz"
  cargo build --profile "$profile" --locked --target "$target"
else
  features=""
  for name in $modules; do
    case " $left " in *" $name "*) ;; *) features="$features${features:+,}$name" ;; esac
  done
  case " $left " in *" console "*) profile=small ;; *) profile=release ;; esac
  archive="rvrs-$target-without-$(echo "$left" | tr ' ' '-').tar.gz"
  cargo build --profile "$profile" --locked --target "$target" -p rvrs --no-default-features --features "$features"
fi

folder="${CARGO_TARGET_DIR:-target}/$target/$profile"
mkdir -p dist
tar -czf "dist/$archive" -C "$folder" rvrs
echo "$folder/rvrs $(wc -c < "$folder/rvrs" | tr -d ' ') bytes, dist/$archive $(wc -c < "dist/$archive" | tr -d ' ') bytes"
