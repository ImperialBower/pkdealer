#!/usr/bin/env bash
# Run the pkdealer service and demo client side-by-side in tmux.
#
# Usage:
#   ./demo.sh          — start a fresh tmux session called "pkdealer-demo"
#   ./demo.sh attach   — reattach to an existing session
#
# Exiting:
#   Ctrl+B  then  D       — detach from tmux (leaves session running in background)
#   Ctrl+B  then  :kill-session  Enter  — kill everything and exit completely
#   ./demo.sh             — re-running the script also kills any existing session first

set -euo pipefail

SESSION="pkdealer-demo"
ROOT="$(cd "$(dirname "$0")" && pwd)"

# ── Tear down any stale session ───────────────────────────────────────────────
tmux kill-session -t "$SESSION" 2>/dev/null || true

# ── Build both binaries first so there's no compile delay during the demo ─────
echo "Building binaries…"
cargo build --bin pkdealer_service -p pkdealer_service \
    --manifest-path "$ROOT/Cargo.toml" -q
cargo build --example demo -p pkdealer_client \
    --manifest-path "$ROOT/Cargo.toml" -q
echo "Done."

# ── Create session with two side-by-side panes ────────────────────────────────
#   Pane 0 (left)  — service
#   Pane 1 (right) — demo client

tmux new-session -d -s "$SESSION" -x "$(tput cols)" -y "$(tput lines)"

# Keep panes open after the process inside them exits
tmux set-option -t "$SESSION" remain-on-exit on

# Left pane: start the service
tmux send-keys -t "$SESSION:0.0" \
    "cd '$ROOT' && cargo run --bin pkdealer_service" Enter

# Split vertically (left | right)
tmux split-window -h -t "$SESSION:0.0"

# Right pane: wait for the service to be ready, then run the demo
tmux send-keys -t "$SESSION:0.1" \
    "cd '$ROOT' && sleep 2 && cargo run --example demo -p pkdealer_client" Enter

# Even out the pane widths
tmux select-layout -t "$SESSION" even-horizontal

# Focus the right (demo) pane so the output is front and centre
tmux select-pane -t "$SESSION:0.1"

# ── Attach ────────────────────────────────────────────────────────────────────
tmux attach-session -t "$SESSION"
