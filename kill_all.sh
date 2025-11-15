#!/bin/bash
# Kill all String Driver processes
# Based on launcher.rs: kills stepper_gui, operations_gui, audio_monitor, persist, qjackctl, launcher
# Also kills xterm "Persist Monitor" window and audmon.sh processes

set -euo pipefail

echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "Killing all String Driver processes..."
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

# Function to kill and verify process is dead
kill_and_verify() {
    local pattern="$1"
    local name="$2"
    local max_attempts=5
    
    echo "Killing $name..."
    
    # Try SIGTERM first
    pkill -f "$pattern" 2>/dev/null || true
    sleep 0.2
    
    # Check if still running, if so use SIGKILL
    if pgrep -f "$pattern" >/dev/null 2>&1; then
        pkill -9 -f "$pattern" 2>/dev/null || true
        sleep 0.2
    fi
    
    # Verify it's dead
    local attempts=0
    while pgrep -f "$pattern" >/dev/null 2>&1 && [ $attempts -lt $max_attempts ]; do
        pkill -9 -f "$pattern" 2>/dev/null || true
        sleep 0.2
        attempts=$((attempts + 1))
    done
    
    if pgrep -f "$pattern" >/dev/null 2>&1; then
        echo "  ✗ $name still running after kill attempts"
        return 1
    else
        echo "  ✓ $name killed"
        return 0
    fi
}

# Kill stepper_gui (launched by launcher)
kill_and_verify "stepper_gui" "stepper_gui" || true

# Kill operations_gui (launched by launcher)
kill_and_verify "operations_gui" "operations_gui" || true

# Kill audio_monitor (launched by persist script, path: target/release/audio_monitor)
kill_and_verify "target/release/audio_monitor" "audio_monitor" || true
kill_and_verify "audio_monitor" "audio_monitor (any)" || true

# Kill persist script (runs in xterm "Persist Monitor")
kill_and_verify "Persist Monitor" "persist xterm" || true
kill_and_verify "audmon.sh" "audmon.sh" || true

# Kill qjackctl (checked by persist, may be launched separately)
kill_and_verify "qjackctl" "qjackctl" || true

# Kill launcher if still running
kill_and_verify "launcher" "launcher" || true

# Clean up Unix sockets
echo "Cleaning up Unix sockets..."
rm -f /tmp/stepper_gui_*.sock && echo "  ✓ Unix sockets cleaned" || echo "  - No sockets to clean"

# Final verification - check for any remaining processes
echo ""
echo "Verifying all processes are dead..."
REMAINING=$(pgrep -f -E "(stepper_gui|operations_gui|target/release/audio_monitor|Persist Monitor|audmon.sh|qjackctl)" 2>/dev/null | wc -l)
if [ "$REMAINING" -gt 0 ]; then
    echo "  ⚠ Warning: $REMAINING process(es) still running:"
    pgrep -f -E "(stepper_gui|operations_gui|target/release/audio_monitor|Persist Monitor|audmon.sh|qjackctl)" 2>/dev/null | while read pid; do
        ps -p "$pid" -o pid,cmd --no-headers 2>/dev/null || true
    done
else
    echo "  ✓ All processes verified dead"
fi

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "Kill complete!"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

