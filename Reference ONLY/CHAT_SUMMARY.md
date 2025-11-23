# Chat Summary - Operations Fixes and Anti-Hammering Rules

## Date: Current Session

## Overview
This session focused on fixing erratic operations behavior in stringdriver, establishing anti-hammering rules to protect hardware, and correcting misunderstandings about position tracking.

---

## Key Issues Identified and Fixed

### 1. Operations "Spazzed Out" Behavior

**Problem:** Operations (bump_check, z_calibrate) were behaving erratically.

**Root Causes Found:**
- **Critical Bug**: `stepper_gui.rs` `reset` handler was calling `set_position()` which used `amove` (command 2) - a PHYSICAL move
- Operations expected `reset` to use `set_stepper` (command 4) - NO physical move, just sets position counter
- Position tracking mismatch: operations_gui initialized positions to 0, not synced with stepper_gui's actual positions

**Fixes Applied:**
1. Created `reset_position()` method in `stepper_gui.rs` that uses command 4 (set_stepper) - no physical movement
2. Updated `reset` IPC handler to call `reset_position()` instead of `set_position()`
3. Fixed `bump_check` logic to check initial bump state first, only process actually bumping steppers
4. Fixed `z_calibrate` to update positions array when resetting to max_pos
5. Added operation lock (`operation_running` flag) to prevent concurrent operations

**Files Modified:**
- `src/gui/stepper_gui.rs`: Added `reset_position()` method, fixed reset handler
- `src/operations.rs`: Fixed bump_check and z_calibrate logic
- `src/gui/operations_gui.rs`: Added operation lock to prevent concurrent execution

---

### 2. Position Tracking Model Clarification

**Critical Understanding:**
- `set_position()` is **MODEL ONLY** - updates internal `self.positions[]` array (code variable)
- Does NOT send physical move commands to Arduino
- Real-world position comes from Arduino via `refresh_positions()` which calibrates the model
- This maintains a parallel model that requires periodic calibration to reality ("keeping a toe in the water")

**Fix Applied:**
- Changed `set_position()` to ONLY update internal model, removed all Arduino communication
- Added clear comments explaining this is model-only

**Before:**
```rust
fn set_position() {
    // Sent amove command (command 2) - PHYSICAL MOVE
    self.send_cmd_bin(2, s, clamped);
    self.refresh_positions();
}
```

**After:**
```rust
fn set_position() {
    // MODEL ONLY - updates internal position tracking variable
    // Does NOT send physical move command to Arduino
    // Real-world position comes from Arduino via refresh_positions()
    self.positions[stepper] = clamped;
}
```

---

### 3. Anti-Hammering Rule Established

**Rule Created:** `/home/gregory/stringdriver/ANTI_HAMMERING_RULE.md`

**Definition of "Hammering":**
- Rapid repeated commands to steppers/Arduino without adequate rest periods
- Excessive GPIO polling (checking sensors faster than necessary)
- Tight loops that send commands faster than hardware can safely handle
- Operations that could stress or damage physical hardware

**Requirements:**
1. **BEFORE making ANY code change** involving hardware operations, MUST REQUEST PERMISSION if it could result in hammering
2. **MUST VERIFY** existing code for hammering before modifying
3. **MUST DOCUMENT** any intentional rapid operations with justification

**Current Rest Periods (from config):**
- `z_rest`: Default 1.0s (configurable)
- `x_rest`: Default 5.0s (configurable)
- `tune_rest`: Default 5.0s (configurable)

**Safe Patterns Verified:**
- ✅ All stepper moves respect rest periods via `rest_z()`, `rest_x()`, `rest_tune()`
- ✅ `bump_check`: Uses `rest_z()` between iterations, has MAX_ITERATIONS (50) limit
- ✅ `z_calibrate`: Uses `rel_move_z()` (includes rest) + `rest_z()`, GPIO checked once per cycle
- ✅ `refresh_positions()`: Called after moves with 500ms wait, after reset with 100ms wait
- ✅ All physical moves use `rel_move` with small steps (default ±2)

**No Large Sudden Moves Found:**
- All physical moves use `rel_move` with small step sizes (default ±2)
- `set_position()` is now model-only (no physical move)
- `reset_position()` sets Arduino's counter without moving

---

## Architecture Understanding

### System Overview
- **audmon** (analysis engine): Processes audio → FFT → extracts partials → writes to shared memory (`/dev/shm/audio_peaks`)
- **stringdriver** (physical interface): Reads partials from shared memory → controls Arduino steppers → logs machine state → closed-loop feedback

### Data Flow
```
Physical System → Audio Input → audmon (FFT/Partials) → Shared Memory → stringdriver → Arduino Steppers → Physical System
                                                                              ↓
                                                                        Database Logging
```

### Position Tracking Model
- **Internal Model**: `self.positions[]` array in `stepper_gui` - code variable only
- **Real-World Calibration**: `refresh_positions()` reads actual positions from Arduino (command "1;")
- **Model Maintenance**: Model is updated manually via `set_position()` and calibrated periodically via `refresh_positions()`
- **Physical Moves**: Only via `move_stepper()` using `rel_move` (command 3) with small deltas

---

## Code Changes Summary

### Files Modified

1. **`src/gui/stepper_gui.rs`**
   - Added `reset_position()` method using command 4 (set_stepper)
   - Fixed `reset` IPC handler to use `reset_position()`
   - Fixed `set_position()` to be model-only (removed Arduino communication)

2. **`src/operations.rs`**
   - Fixed `bump_check()`: Check initial bump state first, only process bumping steppers
   - Fixed `z_calibrate()`: Update positions array when resetting, check sensor before moving

3. **`src/gui/operations_gui.rs`**
   - Added `operation_running` flag to prevent concurrent operations
   - Added operation lock check in `execute_operation()`

### Files Created

1. **`ANTI_HAMMERING_RULE.md`**
   - Comprehensive rule document for preventing hardware hammering
   - Examples of safe vs unsafe operations
   - Current rest period defaults

---

## Rules Compliance

**Verified Compliance:**
- ✅ No fallbacks (fail-fast pattern maintained)
- ✅ No timeouts (only event-driven waits)
- ✅ Single source of truth maintained
- ✅ No renames (preserved existing naming)
- ✅ No hammering (all operations respect rest periods)
- ✅ No large sudden moves (all moves use small deltas)

---

## Remaining Considerations

1. **Position Sync**: Operations start with positions initialized to 0. After first operation completes, positions should be correct since operations update them. Consider initializing from stepper_gui if needed.

2. **IPC abs_move**: Currently calls model-only `set_position()`. If physical absolute moves are needed, should use different function.

3. **Two Cursor Instances**: Confirmed that Cursor instances don't directly interact - each operates independently on its own workspace.

---

## Next Steps (Not Implemented)

- ML integration (MindsDB mentioned)
- Replay from data back to machine
- Model training/feedback loop
- "Stems on a leafy branch" visualization

---

## Key Learnings

1. **Position Model vs Physical Reality**: Critical distinction between internal position tracking (model) and actual hardware position (reality). Model requires periodic calibration.

2. **Command Types Matter**: 
   - Command 2 (amove): Physical absolute move
   - Command 3 (rmove): Physical relative move  
   - Command 4 (set_stepper): Sets position counter WITHOUT moving

3. **Hardware Protection**: Always respect rest periods, use small step sizes, avoid rapid repeated operations.

4. **Operations Logic**: Must match surfer.py behavior - check state before acting, only process what needs processing.

