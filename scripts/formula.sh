#!/bin/sh
# Writes the Homebrew formula for a release: scripts/formula.sh <version> <SHA256SUMS>
set -eu
version="$1"
sums="$2"
sum() { awk -v name="rvrs-$1.tar.gz" '$2 == name { print $1 }' "$sums"; }
url="https://github.com/rvrs-dev/cli/releases/download/v$version"
cat <<FORMULA
class Rvrs < Formula
  desc "traceroute, ping, MTR, dig and BGP in one binary"
  homepage "https://rvrs.dev"
  version "$version"
  license any_of: ["MIT", "Apache-2.0"]

  on_macos do
    on_arm do
      url "$url/rvrs-aarch64-apple-darwin.tar.gz"
      sha256 "$(sum aarch64-apple-darwin)"
    end
    on_intel do
      url "$url/rvrs-x86_64-apple-darwin.tar.gz"
      sha256 "$(sum x86_64-apple-darwin)"
    end
  end

  on_linux do
    on_arm do
      url "$url/rvrs-aarch64-unknown-linux-musl.tar.gz"
      sha256 "$(sum aarch64-unknown-linux-musl)"
    end
    on_intel do
      url "$url/rvrs-x86_64-unknown-linux-musl.tar.gz"
      sha256 "$(sum x86_64-unknown-linux-musl)"
    end
  end

  def install
    bin.install "rvrs"
    generate_completions_from_executable(bin/"rvrs", "completions")
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/rvrs --version")
  end
end
FORMULA
