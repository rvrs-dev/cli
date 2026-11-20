#!/bin/sh
# Builds what the plugin directories take, into dist/ (or the folder given):
#   rvrs-plugin-<version>.zip                the plugin, for the ChatGPT and Codex submission
#   rvrs-network-diagnostics.tar.gz          the skill alone, for /.well-known/agent-skills
# Claude's directory and skills.sh read the repo itself and need neither.
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
out=${1:-"$root/dist"}
sh "$root/scripts/check-skill.sh"
version=$(sed -n 's/.*"version": "\(.*\)".*/\1/p' "$root/plugins/rvrs/.claude-plugin/plugin.json")
mkdir -p "$out"
out=$(cd "$out" && pwd)
rm -f "$out/rvrs-plugin-$version.zip" "$out/rvrs-network-diagnostics.tar.gz"
(cd "$root/plugins" && zip -qr "$out/rvrs-plugin-$version.zip" rvrs -x '*.DS_Store' -x 'rvrs/evals/*')
(cd "$root/plugins/rvrs/skills" && COPYFILE_DISABLE=1 tar -czf "$out/rvrs-network-diagnostics.tar.gz" --exclude .DS_Store rvrs-network-diagnostics)
echo "package-skill: $out/rvrs-plugin-$version.zip"
echo "package-skill: $out/rvrs-network-diagnostics.tar.gz"
