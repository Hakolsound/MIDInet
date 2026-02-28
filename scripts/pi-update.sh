#!/usr/bin/env bash
# ──────────────────────────────────────────────────────────────
# MIDInet — Pi Update Script
# Pulls latest code from GitHub, rebuilds, and restarts services.
#
# Usage:  sudo midinet-update
#    or:  sudo bash scripts/pi-update.sh  (from repo root)
#
# This is installed as /usr/local/bin/midinet-update by pi-provision.sh
# ──────────────────────────────────────────────────────────────
set -euo pipefail

BRANCH="${MIDINET_BRANCH:-v3.1}"
FORCE=false
if [ "${1:-}" = "--force" ]; then
    FORCE=true
fi

RED='\033[0;31m'
GREEN='\033[0;32m'
CYAN='\033[0;36m'
NC='\033[0m'

if [ "$(id -u)" -ne 0 ]; then
    echo -e "${RED}Run as root: sudo midinet-update${NC}"
    exit 1
fi

# Auto-detect source directory
if [ -n "${MIDINET_DIR:-}" ] && [ -d "$MIDINET_DIR/.git" ]; then
    : # explicit override, use as-is
elif [ -d "/opt/midinet/src/.git" ]; then
    MIDINET_DIR="/opt/midinet/src"
elif [ -d "/home/pi/MIDInet/.git" ]; then
    MIDINET_DIR="/home/pi/MIDInet"
else
    echo -e "${RED}Source not found. Checked /opt/midinet/src and /home/pi/MIDInet.${NC}"
    echo -e "${RED}Set MIDINET_DIR or run pi-provision.sh first.${NC}"
    exit 1
fi

echo -e "${CYAN}MIDInet Update${NC}"
echo ""

# Allow root to operate on the repo (owned by pi).
# Git 2.35.2+ blocks this by default with "dubious ownership".
# Use an env var so every git command in this script inherits the setting,
# even under systemd where HOME may not be /root.
export GIT_CONFIG_COUNT=1
export GIT_CONFIG_KEY_0=safe.directory
export GIT_CONFIG_VALUE_0="$MIDINET_DIR"

# Pull latest
echo -e "${CYAN}[1/4]${NC} Pulling latest from origin/$BRANCH..."
cd "$MIDINET_DIR"
BEFORE=$(git rev-parse --short HEAD)
git fetch origin
# Use -B to force branch checkout (avoids checking out tag when both tag + branch
# exist with the same name, which causes detached HEAD)
git checkout -B "$BRANCH" "origin/$BRANCH"
AFTER=$(git rev-parse --short HEAD)

# Re-exec with the freshly-pulled script if it changed.
# This avoids the chicken-and-egg problem where the running copy is stale
# and misses new install steps (e.g. new systemd units).
NEW_SCRIPT="$MIDINET_DIR/scripts/pi-update.sh"
if [ "${MIDINET_REEXEC:-}" != "1" ] && [ -f "$NEW_SCRIPT" ] && ! cmp -s "$0" "$NEW_SCRIPT"; then
    echo -e "    ${CYAN}ℹ${NC}  Update script changed — re-executing with new version"
    export MIDINET_REEXEC=1 MIDINET_DIR MIDINET_BRANCH="${BRANCH}"
    exec bash "$NEW_SCRIPT" "$@"
fi

NEED_BUILD=true

# Check if installed binaries match current HEAD
INSTALLED_HASH=$(cat /usr/local/bin/.midinet-version 2>/dev/null || echo "none")
BINARIES_MATCH=false
if [ "$INSTALLED_HASH" = "$AFTER" ]; then
    BINARIES_MATCH=true
fi

if [ "$FORCE" = true ]; then
    if [ "$BEFORE" != "$AFTER" ]; then
        echo -e "    ${GREEN}✓${NC} Updated $BEFORE → $AFTER"
    else
        echo -e "    ${GREEN}✓${NC} Already up-to-date ($AFTER) — forced rebuild"
    fi
elif [ "$BINARIES_MATCH" = true ]; then
    echo -e "    ${GREEN}✓${NC} Already up-to-date ($AFTER) — binaries match"
    NEED_BUILD=false
elif [ "$BEFORE" != "$AFTER" ]; then
    echo -e "    ${GREEN}✓${NC} Updated $BEFORE → $AFTER"
    echo ""
    echo "  Changes:"
    git log --oneline "$BEFORE..$AFTER" | head -10 | sed 's/^/    /'
    echo ""
    # Skip build if only non-Rust files changed (scripts, deploy, docs, etc.)
    RUST_CHANGES=$(git diff --name-only "$BEFORE..$AFTER" -- 'crates/' 'Cargo.toml' 'Cargo.lock' | head -1)
    if [ -z "$RUST_CHANGES" ]; then
        echo -e "    ${CYAN}ℹ${NC}  No Rust source changes — skipping build"
        NEED_BUILD=false
    fi
else
    # Source unchanged but binaries are stale (e.g. user ran git pull separately,
    # or script re-exec'd after a non-Rust update).
    # Check if there are actually Rust changes since the installed version.
    RUST_CHANGES=$(git diff --name-only "$INSTALLED_HASH..$AFTER" -- 'crates/' 'Cargo.toml' 'Cargo.lock' 2>/dev/null | head -1)
    if [ -n "$RUST_CHANGES" ] || [ "$INSTALLED_HASH" = "none" ]; then
        echo -e "    ${GREEN}✓${NC} Source up-to-date ($AFTER) — binaries stale, rebuilding"
    else
        echo -e "    ${GREEN}✓${NC} Source up-to-date ($AFTER) — no Rust changes since $INSTALLED_HASH, skipping build"
        NEED_BUILD=false
    fi
fi

# Ensure Rust toolchain is available under sudo.
# rustup uses RUSTUP_HOME and CARGO_HOME to locate its config and binaries.
# Under sudo, $HOME is /root so it can't find the pi user's installation.
RUST_USER_HOME=""
for candidate in /home/pi /root; do
    if [ -d "$candidate/.cargo/bin" ] && [ -d "$candidate/.rustup" ]; then
        RUST_USER_HOME="$candidate"
        break
    fi
done

if [ -n "$RUST_USER_HOME" ]; then
    export CARGO_HOME="$RUST_USER_HOME/.cargo"
    export RUSTUP_HOME="$RUST_USER_HOME/.rustup"
    export PATH="$CARGO_HOME/bin:$PATH"
fi

if ! command -v cargo &>/dev/null; then
    echo -e "${CYAN}Rust not found. Installing...${NC}"
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable
    # Set up environment for the rest of this script
    if [ -n "$RUST_USER_HOME" ]; then
        export CARGO_HOME="$RUST_USER_HOME/.cargo"
        export RUSTUP_HOME="$RUST_USER_HOME/.rustup"
    else
        export CARGO_HOME="$HOME/.cargo"
        export RUSTUP_HOME="$HOME/.rustup"
    fi
    export PATH="$CARGO_HOME/bin:$PATH"
    if ! command -v cargo &>/dev/null; then
        echo -e "${RED}Rust installation failed. Install manually:${NC}"
        echo -e "${RED}  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh${NC}"
        exit 1
    fi
    echo -e "    ${GREEN}✓${NC} Rust installed ($(rustc --version))"
fi

# Build (skip if no Rust changes)
STEP=2
if [ "$NEED_BUILD" = true ]; then
    echo -e "${CYAN}[2/4]${NC} Building release (first build may take 15+ min on ARM)..."
    echo -e "    ${CYAN}ℹ${NC}  The final linking step uses low CPU and may look stuck — this is normal on ARM"
    BUILD_START=$SECONDS
    cargo build --release -p midi-host -p midi-admin -p midi-cli
    BUILD_ELAPSED=$((SECONDS - BUILD_START))
    BUILD_MIN=$((BUILD_ELAPSED / 60))
    BUILD_SEC=$((BUILD_ELAPSED % 60))
    echo -e "    ${GREEN}✓${NC} Build complete (${BUILD_MIN}m ${BUILD_SEC}s)"
    STEP=3
fi

# Stop services
echo -e "${CYAN}[$STEP/4]${NC} Stopping services..."
systemctl stop midinet-admin.service 2>/dev/null || true
systemctl stop midinet-host.service 2>/dev/null || true
STEP=$((STEP + 1))

# Install & restart
echo -e "${CYAN}[$STEP/4]${NC} Installing and restarting..."
if [ "$NEED_BUILD" = true ]; then
    install -m 755 "$MIDINET_DIR/target/release/midi-host"  /usr/local/bin/midi-host
    install -m 755 "$MIDINET_DIR/target/release/midi-admin" /usr/local/bin/midi-admin
    install -m 755 "$MIDINET_DIR/target/release/midi-cli"   /usr/local/bin/midi-cli
fi
# Always stamp the version so future runs don't re-trigger for non-Rust changes
echo "$AFTER" > /usr/local/bin/.midinet-version

# Write markers so the admin service can check for updates without repo access.
# src-dir: path to git repo (for run_update to find the update script)
# git-remote: origin URL so ls-remote works without traversing the repo
echo "$MIDINET_DIR" > /var/lib/midinet/src-dir
git -C "$MIDINET_DIR" remote get-url origin > /var/lib/midinet/git-remote 2>/dev/null || true

# Restore repo ownership — git operations above run as root and create
# root-owned files in .git/objects, preventing the normal user from pulling.
REPO_OWNER=$(stat -c '%U:%G' "$MIDINET_DIR")
chown -R "$REPO_OWNER" "$MIDINET_DIR"

# Ensure the midi user (admin service) can read the repo for update checks.
# Allow traversal into the parent dir (e.g. /home/pi) without listing permission,
# and make the repo itself world-readable.
chmod o+x "$(dirname "$MIDINET_DIR")"
find "$MIDINET_DIR" -type d -exec chmod o+rx {} + 2>/dev/null || true
find "$MIDINET_DIR" -type f -exec chmod o+r {} + 2>/dev/null || true

# Ensure midinet-update command exists (may be missing on manually-set-up Pis)
install -m 755 "$MIDINET_DIR/scripts/pi-update.sh" /usr/local/bin/midinet-update

# Update systemd units in case they changed
install -m 644 "$MIDINET_DIR/deploy/midinet-host.service"   /etc/systemd/system/
install -m 644 "$MIDINET_DIR/deploy/midinet-admin.service"  /etc/systemd/system/
install -m 644 "$MIDINET_DIR/deploy/midinet-update.service" /etc/systemd/system/
install -m 644 "$MIDINET_DIR/deploy/midinet-update.path"    /etc/systemd/system/
systemctl daemon-reload

# Enable the path unit so the admin panel can trigger updates without sudo.
# (The path unit watches /var/lib/midinet/update-trigger and starts
# midinet-update.service as root when the file is modified.)
systemctl enable --now midinet-update.path 2>/dev/null || true

systemctl start midinet-host.service
systemctl start midinet-admin.service

echo ""
echo -e "${GREEN}✓ MIDInet updated and running${NC}"
echo ""
systemctl status midinet-host.service midinet-admin.service --no-pager -l 2>/dev/null || true
echo ""
echo "  Dashboard: http://$(hostname -I | awk '{print $1}'):8080"
echo "  Logs:      journalctl -u midinet-host -u midinet-admin -f"
