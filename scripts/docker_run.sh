#!/usr/bin/env bash
# B-10: brings up the full 3-node Argonaut devnet (archive + pruning + fresh
# backfilling node) defined in docker-compose.yml. Pass extra args through to
# `docker compose up`, e.g. `./scripts/docker_run.sh node1 node2` to skip the
# fresh-backfill node, or `./scripts/docker_run.sh --build` to force a rebuild.
set -e

echo "*** Starting Argonaut 3-node devnet ***"

cd "$(dirname "${BASH_SOURCE[0]}")/.."

docker compose down --remove-orphans
docker compose up "$@"
