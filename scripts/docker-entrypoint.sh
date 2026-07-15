#!/usr/bin/env bash
# B-10: translates docker-compose environment variables into argonaut-node
# CLI flags, so docker-compose.yml stays declarative (env vars per service)
# instead of duplicating a long, easy-to-typo command line in each service
# block. Every variable has a safe default matching a plain full node with
# no mining, no pruning, no bootnodes -- override per-service in compose.
set -euo pipefail

NODE_NAME="${NODE_NAME:-argonaut-node}"
CHAIN="${CHAIN:-local}"
VALIDATOR="${VALIDATOR:-false}"
PRISM_PRUNE="${PRISM_PRUNE:-false}"
PRISM_K_PRUNE="${PRISM_K_PRUNE:-60}"
PRISM_STAGE2="${PRISM_STAGE2:-false}"
NODE_KEY_FILE="${NODE_KEY_FILE:-}"
BOOTNODES="${BOOTNODES:-}"
PORT="${PORT:-30333}"
RPC_PORT="${RPC_PORT:-9944}"
PROMETHEUS_PORT="${PROMETHEUS_PORT:-9615}"
BASE_PATH="${BASE_PATH:-/data}"
LOG="${LOG:-info,prism=debug}"

args=(
  --chain "$CHAIN"
  --base-path "$BASE_PATH"
  --name "$NODE_NAME"
  --port "$PORT"
  --rpc-port "$RPC_PORT"
  --prometheus-port "$PROMETHEUS_PORT"
  --rpc-external
  --rpc-cors all
  --rpc-methods unsafe
  --network-backend libp2p
  -l "$LOG"
)

[ "$VALIDATOR" = "true" ] && args+=(--validator)
[ "$PRISM_PRUNE" = "true" ] && args+=(--prism-prune --prism-k-prune "$PRISM_K_PRUNE")
[ "$PRISM_STAGE2" = "true" ] && args+=(--prism-stage2)
[ -n "$NODE_KEY_FILE" ] && args+=(--node-key-file "$NODE_KEY_FILE")

if [ -n "$BOOTNODES" ]; then
  # comma-separated in the env var (compose-friendly), space-separated
  # --bootnodes args on the actual command line.
  IFS=',' read -ra bn <<< "$BOOTNODES"
  for addr in "${bn[@]}"; do
    args+=(--bootnodes "$addr")
  done
fi

echo "argonaut-node ${args[*]}" >&2
exec argonaut-node "${args[@]}"
