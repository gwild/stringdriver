#!/bin/bash
# Kill all String Driver processes
# Kills: stepper_gui, operations_gui, audio_monitor (audmon), and launcher

echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "Killing all String Driver processes..."
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

# Kill stepper_gui
echo "Killing stepper_gui..."
pkill -f "stepper_gui" && echo "  ✓ stepper_gui killed" || echo "  - stepper_gui not running"

# Kill operations_gui
echo "Killing operations_gui..."
pkill -f "operations_gui" && echo "  ✓ operations_gui killed" || echo "  - operations_gui not running"

# Kill audio_monitor (audmon)
echo "Killing audio_monitor (audmon)..."
pkill -f "audio_monitor" && echo "  ✓ audio_monitor killed" || echo "  - audio_monitor not running"
pkill -f "audmon" && echo "  ✓ audmon processes killed" || echo "  - audmon not running"

# Kill launcher if still running
echo "Killing launcher..."
pkill -f "launcher" && echo "  ✓ launcher killed" || echo "  - launcher not running"

# Clean up Unix sockets
echo "Cleaning up Unix sockets..."
rm -f /tmp/stepper_gui_*.sock && echo "  ✓ Unix sockets cleaned" || echo "  - No sockets to clean"

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "Kill complete!"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

