#!/usr/bin/env bash

set -uo pipefail

SCRIPT_PATH=$(readlink -f "$0")
XTERM_BIN=$(command -v xterm || true)

wait_for_pattern_clear() {
    local pattern="$1"
    local desc="$2"
    local attempts=40
    while pgrep -f "$pattern" >/dev/null 2>&1; do
        if [ $attempts -le 0 ]; then
            printf '[persist] Warning: %s still running after timeout\n' "$desc"
            return 1
        fi
        sleep 0.5
        attempts=$((attempts - 1))
    done
    return 0
}

wait_for_name_clear() {
    local name="$1"
    local desc="$2"
    local attempts=40
    while pgrep -x "$name" >/dev/null 2>&1; do
        if [ $attempts -le 0 ]; then
            printf '[persist] Warning: %s still running after timeout\n' "$desc"
            return 1
        fi
        sleep 0.5
        attempts=$((attempts - 1))
    done
    return 0
}

wait_for_pattern_present() {
    local pattern="$1"
    local desc="$2"
    local attempts=40
    while ! pgrep -f "$pattern" >/dev/null 2>&1; do
        if [ $attempts -le 0 ]; then
            printf '[persist] Warning: %s not detected after launch\n' "$desc"
            return 1
        fi
        sleep 0.5
        attempts=$((attempts - 1))
    done
    return 0
}

if [ "${PERSIST_CHILD:-0}" != "1" ]; then
    if [ -n "${DISPLAY:-}" ] && [ -n "$XTERM_BIN" ]; then
        printf '[persist] Closing previous qjackctl session(s)\n'
        pkill -TERM qjackctl && sleep 2 && pkill -KILL qjackctl || true
        wait_for_pattern_clear "qjackctl" "qjackctl session(s)"

        printf '[persist] Closing previous master_gui persist xterm session(s)\n'
        pkill -f "Master GUI Persist Monitor" >/dev/null 2>&1 || true
        wait_for_pattern_clear "Master GUI Persist Monitor" "master_gui persist xterm session(s)"

        printf '[persist] Closing any existing master_gui xterm session(s)\n'
        pkill -f "target/release/master_gui" >/dev/null 2>&1 || true
        wait_for_pattern_clear "target/release/master_gui" "master_gui xterm session(s)"

        printf '[persist] Terminating any existing master_gui process(es)\n'
        pkill -f "target/release/master_gui" >/dev/null 2>&1 || true
        wait_for_pattern_clear "target/release/master_gui" "master_gui process(es)"
        printf '[persist] Spawning dedicated terminal using %s\n' "$XTERM_BIN"
        if "$XTERM_BIN" -T "Master GUI Persist Monitor" -e env PERSIST_CHILD=1 "$SCRIPT_PATH" "$@" & then
            wait_for_pattern_present "Master GUI Persist Monitor" "master_gui persist xterm session"
            exit 0
        else
            printf '[persist] Failed to launch dedicated terminal; continuing in current session\n' >&2
        fi
    else
        printf '[persist] Dedicated terminal unavailable (DISPLAY or xterm missing); continuing in current session\n'
    fi
fi

if ! command -v pgrep >/dev/null 2>&1 || ! command -v pkill >/dev/null 2>&1; then
    echo "Required utilities pgrep/pkill are not available." >&2
    exit 69
fi

# Check for JACK tools
if command -v jack_lsp >/dev/null 2>&1; then
    JACK_AVAILABLE=1
else
    JACK_AVAILABLE=0
fi

# Load audio_monitor.yaml to check if JACK backend is configured
USE_JACK=0
QJACKCTL_CMD=""
if [ -f "$SCRIPT_DIR/audmon/audio_monitor.yaml" ]; then
    HOSTNAME=$(hostname)
    # Try to extract AUDIO_BACKEND and QJACKCTL_CMD from YAML
    # This is a simple grep-based approach - could be improved with yq or python
    if grep -A 20 "$HOSTNAME" "$SCRIPT_DIR/audmon/audio_monitor.yaml" | grep -q "AUDIO_BACKEND.*JACK"; then
        USE_JACK=1
        # Extract QJACKCTL_CMD
        QJACKCTL_CMD=$(grep -A 20 "$HOSTNAME" "$SCRIPT_DIR/audmon/audio_monitor.yaml" | grep "QJACKCTL_CMD" | sed 's/.*QJACKCTL_CMD:[[:space:]]*//' | tr -d '"' | tr -d "'")
        if [ -z "$QJACKCTL_CMD" ]; then
            # Try default location
            if command -v qjackctl >/dev/null 2>&1; then
                QJACKCTL_CMD=$(command -v qjackctl)
            else
                printf '[persist] Warning: JACK backend configured but QJACKCTL_CMD not found\n'
                USE_JACK=0
            fi
        fi
    fi
fi

# If JACK is not available but configured, disable JACK mode
if [ "$USE_JACK" -eq 1 ] && [ "$JACK_AVAILABLE" -eq 0 ]; then
    printf '[persist] Warning: JACK backend configured but jack_lsp not available\n'
    USE_JACK=0
fi

SCRIPT_DIR=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
RUST_EXEC_PATH="$SCRIPT_DIR/target/release/master_gui"

# Check if release binary exists and if rebuild is needed
needs_build=false

if [ ! -f "$RUST_EXEC_PATH" ] || [ ! -x "$RUST_EXEC_PATH" ]; then
    printf '[persist] Release binary not found or not executable, building...\n'
    needs_build=true
else
    # Check if any source files are newer than the binary
    binary_mtime=$(stat -c %Y "$RUST_EXEC_PATH" 2>/dev/null || stat -f %m "$RUST_EXEC_PATH" 2>/dev/null || echo "0")
    
    # Find newest modification time among source files and config
    newest_source_mtime=0
    
    # Check Cargo files and build.rs
    for file in "$SCRIPT_DIR/Cargo.toml" "$SCRIPT_DIR/Cargo.lock" "$SCRIPT_DIR/build.rs"; do
        if [ -f "$file" ]; then
            file_mtime=$(stat -c %Y "$file" 2>/dev/null || stat -f %m "$file" 2>/dev/null || echo "0")
            if [ "$file_mtime" -gt "$newest_source_mtime" ]; then
                newest_source_mtime=$file_mtime
            fi
        fi
    done
    
    # Check all Rust source files
    if command -v find >/dev/null 2>&1; then
        for file in $(find "$SCRIPT_DIR/src" -type f \( -name "*.rs" -o -name "*.toml" \) 2>/dev/null); do
            if [ -f "$file" ]; then
                file_mtime=$(stat -c %Y "$file" 2>/dev/null || stat -f %m "$file" 2>/dev/null || echo "0")
                if [ "$file_mtime" -gt "$newest_source_mtime" ]; then
                    newest_source_mtime=$file_mtime
                fi
            fi
        done
    fi
    
    # Check audmon source files (since master_gui depends on audmon)
    AUDMON_DIR="$SCRIPT_DIR/audmon"
    if [ -d "$AUDMON_DIR" ]; then
        if command -v find >/dev/null 2>&1; then
            for file in $(find "$AUDMON_DIR/src" -type f \( -name "*.rs" -o -name "*.toml" \) 2>/dev/null); do
                if [ -f "$file" ]; then
                    file_mtime=$(stat -c %Y "$file" 2>/dev/null || stat -f %m "$file" 2>/dev/null || echo "0")
                    if [ "$file_mtime" -gt "$newest_source_mtime" ]; then
                        newest_source_mtime=$file_mtime
                    fi
                fi
            done
        fi
    fi
    
    # If any source file is newer than binary, rebuild is needed
    if [ "$newest_source_mtime" -gt "$binary_mtime" ]; then
        printf '[persist] Source files newer than binary, rebuild needed...\n'
        needs_build=true
    fi
fi

if [ "$needs_build" = true ]; then
    if ! command -v cargo >/dev/null 2>&1; then
        echo "ERROR: cargo not found. Cannot build release binary." >&2
        exit 66
    fi
    
    printf '[persist] Running: cargo build --release --bin master_gui\n'
    if ! (cd "$SCRIPT_DIR" && cargo build --release --bin master_gui); then
        echo "ERROR: Failed to build release binary" >&2
        exit 66
    fi
    
    if [ ! -f "$RUST_EXEC_PATH" ] || [ ! -x "$RUST_EXEC_PATH" ]; then
        echo "ERROR: Release binary still not found after build: $RUST_EXEC_PATH" >&2
        exit 66
    fi
    printf '[persist] Release binary built successfully\n'
fi

# Resolve to absolute path for process matching
RUST_EXEC=$(cd "$SCRIPT_DIR" && readlink -f "target/release/master_gui" 2>/dev/null || echo "$RUST_EXEC_PATH")
RUST_CMD=("$RUST_EXEC" "$@")
RUST_PID=""
MONITOR_DELAY=0
INITIAL_LAUNCH_DONE=0
READY_STATUS="$SCRIPT_DIR/.master_gui_status"

printf 'waiting\n' > "$READY_STATUS"
printf '[persist] Status file initialized at %s\n' "$READY_STATUS"

set_status() {
    local status="$1"
    printf '%s\n' "$status" > "$READY_STATUS"
    printf '[persist] Status -> %s\n' "$status"
}

mark_waiting() {
    set_status "waiting"
}

mark_ready() {
    set_status "ready"
}

launch_qjackctl() {
    if [ "$USE_JACK" -eq 0 ]; then
        return 0  # Not using JACK, skip
    fi
    
    if is_qjackctl_running; then
        printf '[persist] qjackctl already running\n'
        return 0
    fi
    
    if [ -z "$QJACKCTL_CMD" ]; then
        printf '[persist] Error: QJACKCTL_CMD not set but JACK backend configured\n'
        return 1
    fi
    
    # Set up DISPLAY if not set
    if [ -z "${DISPLAY:-}" ]; then
        export DISPLAY=":0"
        printf '[persist] DISPLAY not set, using :0\n'
    fi
    
    # Set up XAUTHORITY if not set
    if [ -z "${XAUTHORITY:-}" ] && [ -n "${HOME:-}" ]; then
        export XAUTHORITY="$HOME/.Xauthority"
        printf '[persist] XAUTHORITY not set, using %s\n' "$XAUTHORITY"
    fi
    
    printf '[persist] Launching qjackctl: %s -s\n' "$QJACKCTL_CMD"
    "$QJACKCTL_CMD" -s >/dev/null 2>&1 &
    
    # Wait for qjackctl to start
    local attempts=20
    while [ $attempts -gt 0 ]; do
        if is_qjackctl_running; then
            printf '[persist] qjackctl launched successfully\n'
            return 0
        fi
        sleep 0.5
        attempts=$((attempts - 1))
    done
    
    printf '[persist] Warning: qjackctl did not start within timeout\n'
    return 1
}

is_qjackctl_running() {
    pgrep -x qjackctl >/dev/null 2>&1
}

is_jack_started() {
    if [ "$JACK_AVAILABLE" -eq 1 ]; then
        jack_lsp >/dev/null 2>&1
    else
        # If JACK is not available, assume it's not needed (ALSA backend)
        return 0
    fi
}

wait_for_qjackctl_ready() {
    # Only wait for qjackctl if JACK is available and we're using JACK backend
    if [ "$JACK_AVAILABLE" -eq 0 ]; then
        return 0  # JACK not available, skip check
    fi
    
    printf '[persist] Waiting for qjackctl to report ready.\n'
    if wait_for_pattern_present "qjackctl" "qjackctl session(s)"; then
        printf '[persist] qjackctl detected.\n'
        return 0
    fi
    printf '[persist] qjackctl not detected within timeout.\n'
    return 1
}

wait_for_jack_ready() {
    if [ "$JACK_AVAILABLE" -eq 0 ]; then
        return 0  # JACK not available, skip check
    fi
    
    local attempts=40
    printf '[persist] Waiting for JACK to report ready via jack_lsp.\n'
    while ! jack_lsp >/dev/null 2>&1; do
        printf '[persist] jack_lsp still failing; attempts remaining: %s\n' "$attempts"
        if [ $attempts -le 0 ]; then
            printf '[persist] Warning: JACK not ready after timeout\n'
            return 1
        fi
        sleep 0.5
        attempts=$((attempts - 1))
    done
    printf '[persist] JACK reports ready.\n'
    return 0
}

capture_rust_pid() {
    if [ -n "$RUST_PID" ] && kill -0 "$RUST_PID" 2>/dev/null; then
        printf '[persist] capture_rust_pid: tracked backend alive (pid=%s)\n' "$RUST_PID"
        return 0
    fi

    local pid
    # Match using relative path pattern from script directory
    local rel_pattern="target/release/master_gui"
    pid=$(pgrep -f -n -- "$rel_pattern" 2>/dev/null || true)
    if [ -n "$pid" ]; then
        RUST_PID="$pid"
        printf '[persist] capture_rust_pid: discovered backend (pid=%s)\n' "$RUST_PID"
        return 0
    fi

    RUST_PID=""
    printf '[persist] capture_rust_pid: backend not found\n'
    return 1
}

start_rust() {
    if capture_rust_pid; then
        printf '[persist] start_rust: backend already running, skipping launch\n'
        return 0
    fi

    printf '[persist] Launching Rust backend: %s\n' "${RUST_CMD[*]}"
    setsid "${RUST_CMD[@]}" >/dev/null 2>&1 &
    RUST_PID=$!
    MONITOR_DELAY=0
    INITIAL_LAUNCH_DONE=1
    mark_waiting
    printf '[persist] start_rust: spawn complete (pid=%s)\n' "$RUST_PID"
}

kill_rust() {
    if capture_rust_pid; then
        printf '[persist] Stopping Rust backend (pid=%s)\n' "$RUST_PID"
        kill "$RUST_PID" 2>/dev/null || true
        wait "$RUST_PID" 2>/dev/null || true
        RUST_PID=""
    fi
    mark_waiting
    printf '[persist] kill_rust: backend terminated\n'
}

while true; do
    if [ "$INITIAL_LAUNCH_DONE" -eq 0 ]; then
        # Launch qjackctl FIRST if using JACK backend (before starting master_gui)
        if [ "$USE_JACK" -eq 1 ]; then
            if ! launch_qjackctl; then
                printf '[persist] Failed to launch qjackctl; retrying in 5 seconds...\n'
                sleep 5
                continue
            fi
        fi
        
        # Launch master_gui (following audmon.sh pattern: launch first, then verify)
        start_rust
        sleep 1
        continue
    fi

    if [ "$MONITOR_DELAY" -eq 0 ]; then
        # CRITICAL: Wait for BOTH qjackctl running AND JACK daemon ready BEFORE marking ready
        # Device indices are only correct when JACK is fully started
        # This matches audmon.sh behavior exactly
        if [ "$USE_JACK" -eq 1 ]; then
            if wait_for_qjackctl_ready && wait_for_jack_ready; then
                MONITOR_DELAY=1
                printf '[persist] Initial backend checks complete; monitoring enabled.\n'
                mark_ready
            else
                printf '[persist] Initial backend checks failed; cycling Rust backend.\n'
                kill_rust
                MONITOR_DELAY=0
                INITIAL_LAUNCH_DONE=0
                sleep 1
                continue
            fi
        else
            # ALSA backend - mark ready immediately
            MONITOR_DELAY=1
            printf '[persist] ALSA backend; monitoring enabled.\n'
            mark_ready
        fi
    fi

    if ! capture_rust_pid; then
        printf '[persist] Rust backend missing; starting.\n'
        # Ensure qjackctl is running before restarting master_gui
        if [ "$USE_JACK" -eq 1 ]; then
            launch_qjackctl
            wait_for_jack_ready
        fi
        start_rust
        sleep 1
        continue
    fi

    # Monitor qjackctl and restart if needed (only for JACK backend)
    if [ "$USE_JACK" -eq 1 ]; then
        if ! is_qjackctl_running; then
            printf '[persist] qjackctl died; restarting qjackctl and cycling Rust backend.\n'
            launch_qjackctl
            wait_for_jack_ready
            kill_rust
            MONITOR_DELAY=0
            INITIAL_LAUNCH_DONE=0
            continue
        fi

        if ! is_jack_started; then
            printf '[persist] JACK not responding; cycling Rust backend.\n'
            kill_rust
            MONITOR_DELAY=0
            INITIAL_LAUNCH_DONE=0
            continue
        fi
    fi

    sleep 1
done

