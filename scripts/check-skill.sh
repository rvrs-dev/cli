#!/bin/sh
# Checks the agent skill and its plugin manifests: the frontmatter the Agent Skills spec asks
# for, links that resolve, one version everywhere, the MCP server named the same in both MCP
# configs, and no command the CLI doesn't have.
# With a built binary (RVRS=target/release/rvrs) it also asks rvrs about each command it finds.
set -eu
# Running rvrs here must not count as using it.
export RVRS_PULSE=0 RVRS_UPDATE_CHECK=0

root=$(cd "$(dirname "$0")/.." && pwd)
plugin="$root/plugins/rvrs"
name=rvrs-network-diagnostics
skill="$plugin/skills/$name"
file="$skill/SKILL.md"
failed=0
fail() {
  echo "check-skill: $1" >&2
  failed=1
}

[ -f "$file" ] || { echo "check-skill: $file is missing" >&2; exit 1; }
[ "$(sed -n 1p "$file")" = "---" ] || fail "SKILL.md has no frontmatter"
front=$(awk 'NR>1 && /^---$/ {exit} NR>1 {print}' "$file")

[ "$(printf '%s\n' "$front" | sed -n 's/^name: //p')" = "$name" ] || fail "name must be $name, the folder's name"
description=$(printf '%s\n' "$front" | sed -n 's/^description: //p')
[ -n "$description" ] || fail "description is missing"
[ "${#description}" -le 1024 ] || fail "description is ${#description} characters, over 1024"
compatibility=$(printf '%s\n' "$front" | sed -n 's/^compatibility: //p')
[ "${#compatibility}" -le 500 ] || fail "compatibility is over 500 characters"
# The frontmatter is YAML a real parser reads, with the description as one string.
if command -v ruby >/dev/null 2>&1; then
  ruby -ryaml -e '
    front = File.read(ARGV[0], encoding: "UTF-8").split(/^---$/)[1]
    data = YAML.safe_load(front)
    abort "name is not a string" unless data["name"].is_a?(String)
    abort "description is not one string" unless data["description"].is_a?(String) && data["description"].length.between?(1, 1024)
    abort "metadata values must be strings" unless data.fetch("metadata", {}).values.all? { |value| value.is_a?(String) }
  ' "$file" || fail "the frontmatter does not parse as YAML"
fi
lines=$(wc -l <"$file")
[ "$lines" -le 500 ] || fail "SKILL.md is $lines lines, over 500"

# Every relative link in the skill resolves.
for doc in "$file" "$skill"/references/*.md; do
  dir=$(dirname "$doc")
  grep -o '](\([^)#]*\)' "$doc" | sed 's/^](//' | while read -r link; do
    case "$link" in
      http*|"") ;;
      *) [ -e "$dir/$link" ] || echo "check-skill: $(basename "$doc") links to $link, which is missing" >&2 ;;
    esac
  done
done | grep . && failed=1

# One version: the skill, every manifest, and the CLI's VERSION as major.minor.
version=$(printf '%s\n' "$front" | sed -n 's/^  version: "\(.*\)"/\1/p')
for manifest in "$plugin/.claude-plugin/plugin.json" "$plugin/.codex-plugin/plugin.json" "$plugin/plugin.json"; do
  grep -q "\"version\": \"$version\"" "$manifest" || fail "${manifest#"$plugin/"} is not at $version"
done

# Claude's .mcp.json and OpenAI's mcp.json name the same MCP server.
for config in "$plugin/.mcp.json" "$plugin/mcp.json"; do
  grep -q '"url": "https://mcp.rvrs.dev"' "$config" || fail "$(basename "$config") does not name https://mcp.rvrs.dev"
done
case "$version" in
  "$(cat "$root/VERSION")".*) ;;
  *) fail "skill version $version does not start with VERSION $(cat "$root/VERSION")" ;;
esac
grep -q '"source": "./plugins/rvrs"' "$root/.claude-plugin/marketplace.json" || fail "marketplace.json does not point at plugins/rvrs"

# No real public address in what gets published: documentation ranges and well-known resolvers only.
if grep -rhoE '\b[0-9]{1,3}(\.[0-9]{1,3}){3}\b' "$plugin" |
  grep -vE '^(192\.0\.2\.|198\.51\.100\.|203\.0\.113\.|1\.1\.1\.1$|8\.8\.8\.8$|9\.9\.9\.9$)' | grep .; then
  fail "an address outside the documentation ranges is in the plugin"
fi

# Every subcommand the skill names is one the CLI has.
if [ -n "${RVRS:-}" ]; then
  help=$("$RVRS" --help)
  for command in $(grep -rhoE '`rvrs [a-z0-9]+' "$skill" | sed 's/`rvrs //' | sort -u); do
    case "$command" in
      dig|ping|mtr|traceroute|bgp|help) ;;
      *) printf '%s\n' "$help" | grep -qE "^  $command( |$)" || fail "the skill names rvrs $command, which rvrs --help doesn't list" ;;
    esac
  done
fi

[ "$failed" = 0 ] || exit 1
echo "check-skill: $name $version is in order"
