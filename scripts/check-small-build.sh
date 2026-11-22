#!/bin/sh
# Builds the small build for a target and holds it to its budget: scripts/check-small-build.sh <target>
#
# The small build is rvrs without the Console (docs/modules.md), the build for servers and for
# routers with little flash. It fails when the binary is bigger than the target's budget in
# scripts/small-build.budget, or when a crate comes into the build or leaves it that
# scripts/small-build.deps doesn't list, which is how a new dependency in rvrs-core shows up.
# It prints the size beside the one recorded, and writes both to the job summary in CI.
set -eu
[ $# -eq 1 ] || { echo "Usage: scripts/check-small-build.sh <target>" >&2; exit 2; }
target="$1"
root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

line="$(awk -v target="$target" '$1 == target' scripts/small-build.budget)"
[ -n "$line" ] || { echo "scripts/small-build.budget has no line for $target." >&2; exit 1; }
budget="$(echo "$line" | awk '{ print $2 }')"
recorded="$(echo "$line" | awk '{ print $3 }')"

sh scripts/build.sh "$target" --without console
binary="${CARGO_TARGET_DIR:-target}/$target/small/rvrs"
bytes="$(wc -c < "$binary" | tr -d ' ')"
packed="$(gzip -9 -c "$binary" | wc -c | tr -d ' ')"
change=$((bytes - recorded))
sign=""
[ "$change" -ge 0 ] && sign="+"

failed=0
report="The small build for $target is $bytes bytes ($sign$change against the $recorded recorded), $packed gzipped. Its budget is $budget."
echo "$report"

if [ "$bytes" -gt "$budget" ]; then
  echo "It is over its budget by $((bytes - budget)) bytes." >&2
  failed=1
fi

# The crates, by name, as cargo tree lists them for this build.
features="$(sed -n 's/^default = \[\(.*\)\]$/\1/p' crates/rvrs/Cargo.toml | tr -d '" ' | tr ',' '\n' | grep -vx console | paste -sd, -)"
cargo tree --locked -p rvrs --no-default-features --features "$features" --target "$target" -e normal --prefix none -f '{p}' |
  awk '{ print $1 }' | sort -u > "${TMPDIR:-/tmp}/small-build.deps.$$"
grep -v '^#' scripts/small-build.deps | sort -u > "${TMPDIR:-/tmp}/small-build.listed.$$"
came="$(comm -13 "${TMPDIR:-/tmp}/small-build.listed.$$" "${TMPDIR:-/tmp}/small-build.deps.$$" | paste -sd' ' -)"
went="$(comm -23 "${TMPDIR:-/tmp}/small-build.listed.$$" "${TMPDIR:-/tmp}/small-build.deps.$$" | paste -sd' ' -)"
rm -f "${TMPDIR:-/tmp}/small-build.deps.$$" "${TMPDIR:-/tmp}/small-build.listed.$$"
if [ -n "$came" ]; then
  echo "Crates came into the small build: $came. If they belong there, add them to scripts/small-build.deps." >&2
  case " $came " in *" ratatui "* | *" crossterm "*) echo "ratatui and crossterm are the Console's, and the small build leaves the Console out." >&2 ;; esac
  failed=1
fi
if [ -n "$went" ]; then
  echo "Crates left the small build: $went. Take them out of scripts/small-build.deps." >&2
  failed=1
fi

if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
  {
    echo "### Small build, $target"
    echo
    echo "| Binary | Gzipped | Recorded | Change | Budget |"
    echo "| ---: | ---: | ---: | ---: | ---: |"
    echo "| $bytes | $packed | $recorded | $sign$change | $budget |"
    [ -z "$came" ] || echo "
New crates: $came"
    [ -z "$went" ] || echo "
Crates gone: $went"
  } >> "$GITHUB_STEP_SUMMARY"
fi
exit "$failed"
