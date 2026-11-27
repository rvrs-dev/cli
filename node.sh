#!/bin/sh
# Sets this machine up as an rvrs node: installs the small build, keeps the node's token and runs
# `rvrs node` as a service that starts again after a crash and at boot.
#
#   wget -qO- https://rvrs.dev/node.sh | RVRS_TOKEN=<token> sh
#   curl -fsSL https://rvrs.dev/node.sh | sudo RVRS_TOKEN=<token> sh
#
# The token is made at app.rvrs.dev under Installs, New Token. It goes in the environment of sh,
# so it is never in an address, an argument or the service's files. The node keeps it in
# /var/lib/rvrs/node/token, readable by the user rvrs alone.
#
# It needs root, Linux, and OpenRC (Alpine) or systemd (Debian, Ubuntu, Fedora, Arch and most
# others). Running it again updates rvrs and restarts the node, and keeps the token unless
# RVRS_TOKEN gives another one.
#
#   --uninstall  stop the node and remove it, with its token, its outbox and its schedules
#
# The node's settings are written to /etc/rvrs/node.env, which the service reads, when any of
# them is set: RVRS_NODE_TAGS, RVRS_NODE_LOCATION, RVRS_NODE_PLACE,
# RVRS_NODE_LOCATION_PRECISION, RVRS_NODE_ALLOW, RVRS_NODE_LIMITS and RVRS_API (docs/node.md).
# When none is set, the file stays as it is.
#
# RVRS_INSTALLER runs a local install.sh in place of the release's, and RVRS_INSTALL_ARCHIVE and
# RVRS_INSTALL_SUMS reach it, which is how this is tested.
set -eu

repo="rvrs-dev/cli"
service=rvrs-node
user=rvrs
settings_names="RVRS_NODE_TAGS RVRS_NODE_LOCATION RVRS_NODE_PLACE RVRS_NODE_LOCATION_PRECISION RVRS_NODE_ALLOW RVRS_NODE_LIMITS RVRS_API"

usage() {
  echo "Usage: RVRS_TOKEN=<token> sh node.sh [--uninstall]"
  echo "  --uninstall  stop the node and remove it from this machine"
}

uninstall=0
while [ $# -gt 0 ]; do
  case "$1" in
    --uninstall) uninstall=1 ;;
    -h | --help)
      usage
      exit 0
      ;;
    *)
      echo "rvrs node.sh: node.sh has no option $1. See node.sh --help." >&2
      exit 2
      ;;
  esac
  shift
done

fail() {
  echo "rvrs node.sh: $1" >&2
  exit 1
}

[ "$(uname -s)" = Linux ] || fail "A node runs on Linux, and this is $(uname -s)."
[ -f /etc/openwrt_release ] && fail "OpenWrt needs a package of its own, which isn't released yet."
if [ "$(id -u)" != 0 ]; then
  if [ "$uninstall" = 1 ]; then
    fail "Removing a node takes root. Run it with sudo: curl -fsSL https://rvrs.dev/node.sh | sudo sh -s -- --uninstall"
  fi
  fail "Setting up a node takes root. Run it with sudo: curl -fsSL https://rvrs.dev/node.sh | sudo RVRS_TOKEN=<token> sh"
fi

# The service manager. OpenRC counts only when it booted this machine, which /run/openrc says.
if [ -d /run/systemd/system ] && command -v systemctl >/dev/null 2>&1; then
  init=systemd
elif command -v openrc-run >/dev/null 2>&1 && [ -d /run/openrc ]; then
  init=openrc
else
  fail "This machine has neither OpenRC nor systemd. In a container, use the image ghcr.io/rvrs-dev/rvrs. Elsewhere, install rvrs with install.sh and start rvrs node under your service manager."
fi

if [ "$uninstall" = 1 ]; then
  if [ "$init" = systemd ]; then
    systemctl disable --now "$service" >/dev/null 2>&1 || true
    rm -f /etc/systemd/system/rvrs-node.service
    systemctl daemon-reload
  else
    rc-service "$service" stop >/dev/null 2>&1 || true
    rc-update del "$service" default >/dev/null 2>&1 || true
    rm -f /etc/init.d/rvrs-node
  fi
  rm -rf /var/lib/rvrs /etc/rvrs
  # node.sh writes the file only where no group could open ICMP sockets, so that is undone too.
  if [ -f /etc/sysctl.d/60-rvrs-node.conf ]; then
    printf '1 0\n' >/proc/sys/net/ipv4/ping_group_range 2>/dev/null || true
    rm -f /etc/sysctl.d/60-rvrs-node.conf
  fi
  rm -f /usr/local/bin/rvrs
  if id "$user" >/dev/null 2>&1; then
    if command -v userdel >/dev/null 2>&1; then
      userdel "$user" 2>/dev/null || true
    else
      deluser "$user" 2>/dev/null || true
    fi
  fi
  if grep -q "^$user:" /etc/group 2>/dev/null; then
    groupdel "$user" 2>/dev/null || delgroup "$user" 2>/dev/null || true
  fi
  echo "Removed the node from this machine. Its token works until you revoke it at app.rvrs.dev under Installs."
  exit 0
fi

token="$(printf '%s' "${RVRS_TOKEN:-}" | tr -d ' \t\r\n')"
if [ -z "$token" ] && [ ! -s /var/lib/rvrs/node/token ]; then
  fail "A node needs its token. Make one at app.rvrs.dev under Installs, New Token, then run node.sh again with RVRS_TOKEN=<token> in front of sh."
fi

# The settings given now, checked before anything changes. A value is written in single quotes,
# which both the shell and systemd read, so it can't hold one.
given=""
for name in $settings_names; do
  eval "value=\${$name:-}"
  [ -n "$value" ] || continue
  case "$value" in
    *"'"* | *"
"*) fail "$name can't hold a single quote or a line break." ;;
  esac
  given="$given $name"
done

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

# The small build, through the release's own installer, which checks it against SHA256SUMS. It
# lands in a folder of its own first, so nothing on the machine changes when it fails.
if [ -n "${RVRS_INSTALLER:-}" ]; then
  cp "$RVRS_INSTALLER" "$work/install.sh"
else
  fetch "https://github.com/$repo/releases/latest/download/install.sh" "$work/install.sh" ||
    fail "Couldn't download install.sh from the newest release."
fi
mkdir "$work/bin"
if ! RVRS_INSTALL_DIR="$work/bin" sh "$work/install.sh" --without console --no-link \
  </dev/null >"$work/install.log" 2>&1; then
  cat "$work/install.log" >&2
  exit 1
fi
mkdir -p /usr/local/bin
cp "$work/bin/rvrs" /usr/local/bin/.rvrs.new
chmod 755 /usr/local/bin/.rvrs.new
# A rename, so a node that is running keeps its binary until it restarts.
mv -f /usr/local/bin/.rvrs.new /usr/local/bin/rvrs
echo "Installed $(/usr/local/bin/rvrs --version) in /usr/local/bin"

if ! id "$user" >/dev/null 2>&1; then
  nologin="$(command -v nologin 2>/dev/null || echo /bin/false)"
  if command -v useradd >/dev/null 2>&1; then
    useradd --system --user-group --home-dir /var/lib/rvrs --no-create-home --shell "$nologin" "$user"
  elif command -v adduser >/dev/null 2>&1; then
    grep -q "^$user:" /etc/group || addgroup -S "$user"
    adduser -S -D -H -h /var/lib/rvrs -s "$nologin" -G "$user" "$user"
  else
    fail "Couldn't make the user rvrs. This system has neither useradd nor adduser."
  fi
  echo "Made the user rvrs, which the node runs as"
fi

mkdir -p /var/lib/rvrs/node
chown "$user:$user" /var/lib/rvrs /var/lib/rvrs/node
chmod 750 /var/lib/rvrs
chmod 700 /var/lib/rvrs/node
if [ -n "$token" ]; then
  (umask 077 && printf '%s\n' "$token" >/var/lib/rvrs/node/token.new)
  chown "$user:$user" /var/lib/rvrs/node/token.new
  mv -f /var/lib/rvrs/node/token.new /var/lib/rvrs/node/token
fi

if [ -n "$given" ]; then
  mkdir -p /etc/rvrs
  {
    echo "# The rvrs node's settings, read by the service rvrs-node when it starts."
    for name in $given; do
      eval "value=\$$name"
      printf "%s='%s'\n" "$name" "$value"
    done
  } >/etc/rvrs/node.env.new
  mv -f /etc/rvrs/node.env.new /etc/rvrs/node.env
fi

# ICMP. The node opens an ICMP datagram socket, which net.ipv4.ping_group_range allows by
# group, and falls back to a raw one, which CAP_NET_RAW allows. Alpine's kernel allows the
# datagram socket to no group, and the path MTU check has no fallback, so where no group may, the
# node's group gets the range for itself. The service grants CAP_NET_RAW either way.
# awk reads the file whole. busybox's read takes it a byte at a time and gets the first number only.
if awk '{ exit !($1 > $2) }' /proc/sys/net/ipv4/ping_group_range 2>/dev/null; then
  gid="$(id -g "$user")"
  echo "net.ipv4.ping_group_range = $gid $gid" >/etc/sysctl.d/60-rvrs-node.conf
  if printf '%s %s\n' "$gid" "$gid" >/proc/sys/net/ipv4/ping_group_range 2>/dev/null; then
    echo "Let the group rvrs open ICMP sockets: net.ipv4.ping_group_range is $gid $gid"
  fi
fi

if [ "$init" = systemd ]; then
  cat >/etc/systemd/system/rvrs-node.service <<'EOF'
# Written by node.sh (rvrs.dev/node.sh). Running node.sh again writes it anew.
[Unit]
Description=rvrs node
Documentation=https://rvrs.dev/docs
Wants=network-online.target
After=network-online.target

[Service]
User=rvrs
Group=rvrs
Environment=RVRS_HOME=/var/lib/rvrs
EnvironmentFile=-/etc/rvrs/node.env
ExecStart=/usr/local/bin/rvrs node --state /var/lib/rvrs/node
Restart=always
RestartSec=5
AmbientCapabilities=CAP_NET_RAW
CapabilityBoundingSet=CAP_NET_RAW
NoNewPrivileges=yes
ProtectSystem=strict
ProtectHome=yes
PrivateTmp=yes
ReadWritePaths=/var/lib/rvrs

[Install]
WantedBy=multi-user.target
EOF
  systemctl daemon-reload
  systemctl enable "$service" >/dev/null 2>&1
  systemctl restart "$service"
  log_hint="journalctl -u rvrs-node"
else
  cat >/etc/init.d/rvrs-node <<'EOF'
#!/sbin/openrc-run
# Written by node.sh (rvrs.dev/node.sh). Running node.sh again writes it anew.

name="rvrs node"
description="Makes the runs an rvrs workspace asks of this machine"
supervisor=supervise-daemon
command=/usr/local/bin/rvrs
command_args="node --state /var/lib/rvrs/node"
command_user=rvrs:rvrs
capabilities="^cap_net_raw"
respawn_delay=5
respawn_max=0
if command -v logger >/dev/null 2>&1; then
	output_logger="logger -t rvrs-node -p daemon.info"
	error_logger="logger -t rvrs-node -p daemon.err"
fi

export RVRS_HOME=/var/lib/rvrs
if [ -f /etc/rvrs/node.env ]; then
	set -a
	. /etc/rvrs/node.env
	set +a
fi

depend() {
	need net
	use dns logger
	after firewall
}
EOF
  chmod 755 /etc/init.d/rvrs-node
  rc-update add "$service" default >/dev/null 2>&1
  offset=0
  [ -f /var/log/messages ] && offset="$(wc -c </var/log/messages | tr -d ' ')"
  rc-service "$service" stop >/dev/null 2>&1 || true
  rc-service "$service" start >/dev/null
  log_hint="grep rvrs-node /var/log/messages"
fi
echo "Started the service rvrs-node"

# Without a system log to read, there is nothing to wait for.
if [ "$init" = openrc ] && [ ! -f /var/log/messages ]; then
  exit 0
fi

# What the node wrote since it started.
said() {
  if [ "$init" = systemd ]; then
    journalctl _SYSTEMD_INVOCATION_ID="$invocation" -o cat --no-pager 2>/dev/null
  else
    tail -c +"$((offset + 1))" /var/log/messages 2>/dev/null | grep rvrs-node
  fi
}
invocation=""
if [ "$init" = systemd ]; then
  invocation="$(systemctl show -p InvocationID --value "$service")"
fi

# Up to 20 seconds for the API to answer the node's first heartbeat and open its event stream.
name=""
waited=0
while [ "$waited" -lt 20 ]; do
  out="$(said || true)"
  case "$out" in
    *"doesn't know this node's token"*)
      fail "The API refused the token. Make a new one at app.rvrs.dev under Installs and run node.sh again with it."
      ;;
  esac
  name="$(printf '%s\n' "$out" | sed -n 's/.*The API knows this node as \(.*\)\.$/\1/p' | tail -n 1)"
  case "$out" in *"The event stream is open."*) break ;; esac
  sleep 1
  waited=$((waited + 1))
done

case "$out" in
  *"The event stream is open."*)
    if [ -n "$name" ]; then
      echo "The node $name is online."
    else
      echo "The node is online."
    fi
    ;;
  *)
    if [ -n "$name" ]; then
      echo "The API knows this node as $name. Its event stream isn't open yet. Its log: $log_hint"
    else
      echo "The node hasn't reached the API yet. Its log: $log_hint"
    fi
    ;;
esac
