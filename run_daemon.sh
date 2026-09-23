#!/usr/bin/env bash
set -e

PORT="${1:-auto}"
LOG_FILE="/tmp/gibberish/daemon.log"

mkdir -p /tmp/gibberish

# Clean up any existing instances
pkill -f gibberish-daemon 2>/dev/null || true

echo "Starting Gibberish Daemon (Target: ${PORT})..."
echo "Logging live to terminal and ${LOG_FILE}"
echo "--------------------------------------------------"

if [ "$PORT" = "auto" ]; then
    cargo run --release -p gibberish-daemon -- --log-file "${LOG_FILE}"
else
    cargo run --release -p gibberish-daemon -- --port "${PORT}" --log-file "${LOG_FILE}"
fi
