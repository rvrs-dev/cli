# The rvrs node as a container: the small build's static binary and nothing else.
#
#   docker run -d --name rvrs-node --restart unless-stopped --network host \
#     -e RVRS_TOKEN=<token> -v rvrs-node:/var/lib/rvrs ghcr.io/rvrs-dev/rvrs
#
# --network host has the runs start from the machine's own address, with IPv6 where the machine
# has it. The container runs as root, which Docker's default capabilities let send ICMP. The
# volume keeps the node's outbox, so results that wait for the uplink outlast a restart.
#
# The release workflow puts the small build of each architecture next to this file, as
# rvrs-amd64 and rvrs-arm64 (.github/workflows/release.yml). The binary carries its own root
# certificates, and Docker gives the container the machine's resolver.
FROM scratch
ARG TARGETARCH
COPY rvrs-${TARGETARCH} /usr/bin/rvrs
ENV RVRS_HOME=/var/lib/rvrs RVRS_NODE_STATE=/var/lib/rvrs/node
VOLUME /var/lib/rvrs
ENTRYPOINT ["/usr/bin/rvrs", "node"]
