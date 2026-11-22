#!/bin/sh
# Checks that every module can be left out (docs/modules.md): rvrs builds without each one on
# its own, and without any, with no warnings. CI runs it.
set -eu
root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"
modules="$(sed -n 's/^default = \[\(.*\)\]$/\1/p' crates/rvrs/Cargo.toml | tr -d '" ' | tr ',' ' ')"
export RUSTFLAGS="${RUSTFLAGS:-} -D warnings"

check() {
  echo "Without $1"
  cargo check --locked --all-targets -p rvrs --no-default-features --features "$2"
}

for out in $modules; do
  # Leaving out traceroute leaves out what works inside it.
  case "$out" in traceroute) gone="traceroute tcp mtu" ;; *) gone="$out" ;; esac
  features=""
  for name in $modules; do
    case " $gone " in *" $name "*) ;; *) features="$features${features:+,}$name" ;; esac
  done
  check "$out" "$features"
done
check "any module" ""
