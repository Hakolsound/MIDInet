#!/usr/bin/env bash
# ──────────────────────────────────────────────────────────────
# MIDInet — Avahi hostname alias publisher
# Publishes additional .local hostnames for this Pi.
#
# A Pi can only have one system hostname (e.g., midinet.local).
# This script publishes extra aliases so the same Pi is also
# reachable at companion.local (for Bitfocus Companion).
#
# Runs as a systemd service: midinet-avahi-alias.service
# Config: /etc/midinet/avahi-aliases.conf (one alias per line)
# ──────────────────────────────────────────────────────────────
set -euo pipefail

ALIAS_CONF="/etc/midinet/avahi-aliases.conf"

if [ ! -f "$ALIAS_CONF" ]; then
    echo "No alias config at $ALIAS_CONF — nothing to publish."
    # Sleep forever so systemd doesn't restart-loop
    exec sleep infinity
fi

# Read aliases (skip comments and blank lines)
mapfile -t ALIASES < <(grep -v '^\s*#' "$ALIAS_CONF" | grep -v '^\s*$')

if [ ${#ALIASES[@]} -eq 0 ]; then
    echo "No aliases configured — nothing to publish."
    exec sleep infinity
fi

# Wait for a valid IP on the primary interface
get_ip() {
    hostname -I 2>/dev/null | awk '{print $1}'
}

IP=""
for i in $(seq 1 30); do
    IP=$(get_ip)
    if [ -n "$IP" ]; then
        break
    fi
    echo "Waiting for IP address... (attempt $i/30)"
    sleep 2
done

if [ -n "$IP" ]; then
    echo "Publishing ${#ALIASES[@]} alias(es) for IP $IP"
else
    echo "Warning: No IP detected, using fallback 0.0.0.0"
    IP="0.0.0.0"
fi

# Build avahi-publish-address commands for each alias
PIDS=()
for alias in "${ALIASES[@]}"; do
    # Ensure alias ends with .local
    if [[ "$alias" != *.local ]]; then
        alias="${alias}.local"
    fi
    echo "  Publishing: $alias -> $IP"
    avahi-publish-address -R "$alias" "$IP" &
    PIDS+=($!)
done

# Wait for any child to exit (means avahi-daemon stopped or error)
cleanup() {
    for pid in "${PIDS[@]}"; do
        kill "$pid" 2>/dev/null || true
    done
    wait
}
trap cleanup EXIT INT TERM

wait -n
