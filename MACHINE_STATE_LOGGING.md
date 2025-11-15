# Machine State Logging Implementation Guide

## Overview
Complete machine state logging captures ALL stepper positions, ALL operations control settings, and runtime audio analysis data. Links to audmon's `controls_id` for concurrent time-series correlation.

## Database Schema

✅ **Created:** `create_tables.sql` - Run this on PostgreSQL to create tables
✅ **Documented:** `DATABASE_SCHEMA.md` - Complete schema documentation

## What Gets Captured

### From Operations Struct (`src/operations.rs`)
- **All Settings:**
  - `bump_check_enable` (boolean)
  - `z_up_step`, `z_down_step` (integers)
  - `tune_rest`, `x_rest`, `z_rest`, `lap_rest` (floats, seconds)
  - `adjustment_level`, `retry_threshold`, `delta_threshold`, `z_variance_threshold` (integers)
  
- **Runtime Audio Analysis (from partials):**
  - `voice_count` (Vec<usize>, per-channel)
  - `amp_sum` (Vec<f32>, per-channel)
  
- **Thresholds (from operations_gui):**
  - `voice_count_min`, `voice_count_max` (per-channel arrays)
  - `amp_sum_min`, `amp_sum_max` (per-channel arrays)

### From Stepper Positions
- **ALL stepper positions** - Must get from stepper_gui via IPC or operations_gui's local tracking
- **ALL stepper enabled states** - From Operations struct's `stepper_enabled` HashMap

### From audmon (for correlation)
- **controls_id** (UUID) - Links to audmon's controls table
  - Currently NOT in shared memory
  - Options:
    1. Add controls_id to shared memory structure in audmon
    2. Query database for most recent controls_id for this host
    3. Accept None for now, link later via timestamp matching

## Implementation Steps

### 1. Add DbSettings to config_loader.rs
Copy `DbSettings` struct from audmon:
```rust
#[derive(Debug, Clone)]
pub struct DbSettings {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub password: String,
    pub database: String,
}

impl DbSettings {
    pub fn from_env() -> Result<Self> {
        // Same as audmon - reads from .env file
        // PG_HOST, PG_PORT, PG_USER, PG_PASSWORD, PG_DATABASE
    }
}
```

### 2. Create Machine State Logger Module
Key functions:
```rust
pub struct MachineStateLogger {
    // Event-driven, non-blocking (like audmon's db_logger)
}

impl MachineStateLogger {
    pub fn log_machine_state(
        &self,
        controls_id: Option<Uuid>,  // From audmon, can be None
        host: &str,
        stepper_positions: &[i32],  // ALL steppers
        stepper_enabled: &[bool],   // ALL steppers
        operations: &Operations,     // Get all settings from here
        voice_count: &[usize],      // From operations.get_voice_count()
        amp_sum: &[f32],           // From operations.get_amp_sum()
        thresholds: &Thresholds,    // From operations_gui
    ) -> Result<Uuid>;  // Returns state_id
    
    pub fn log_operation_start(
        &self,
        state_id: Uuid,
        operation_type: &str,
        stepper_indices: &[usize],
    ) -> Result<Uuid>;  // Returns operation_id
    
    pub fn log_operation_complete(
        &self,
        operation_id: Uuid,
        final_positions: &[i32],
        message: &str,
    ) -> Result<()>;
}
```

### 3. Integration Points

**In operations_gui.rs:**
```rust
// Before operation starts:
let state_id = logger.log_machine_state(
    controls_id,  // Get from audmon or None
    &hostname,
    &stepper_positions,  // Get ALL positions from stepper_gui
    &stepper_enabled_vec,  // Convert HashMap to Vec
    &operations,  // Operations struct
    &operations.get_voice_count(),
    &operations.get_amp_sum(),
    &thresholds,
);

let op_id = logger.log_operation_start(state_id, "z_calibrate", &stepper_indices);

// After operation completes:
logger.log_operation_complete(op_id, &final_positions, "Success");
```

### 4. Getting ALL Stepper Positions

**Option A:** Query stepper_gui via IPC
```rust
// Send "get_positions" command to stepper_gui Unix socket
// Returns all positions
```

**Option B:** Track in operations_gui
```rust
// operations_gui already tracks stepper_positions
// But need to ensure it has ALL steppers, not just Z steppers
```

**Option C:** Read from Arduino directly
```rust
// Use arduino_connection::read_positions()
// But this requires direct Arduino access
```

### 5. Getting controls_id from audmon

**Current:** Not available in shared memory

**Solution Options:**

**Option 1:** Add to shared memory (requires audmon change)
```rust
// In audmon: Add controls_id to shared memory structure
// In stringdriver: Read controls_id from shared memory
```

**Option 2:** Query database (simpler, no audmon change)
```rust
// Query: SELECT controls_id FROM controls 
//        WHERE host = ? ORDER BY recorded_at DESC LIMIT 1
// Use most recent controls_id for this host
```

**Option 3:** Timestamp matching (post-processing)
```rust
// Log with NULL controls_id
// Later: Match by timestamp proximity (within 1 second)
// UPDATE machine_state SET controls_id = ... WHERE ...
```

## Current Status

✅ Database schema designed and documented
✅ SQL creation script created (`create_tables.sql`)
✅ All fields identified from codebase
⏳ Logger module implementation (next step)
⏳ Integration into operations_gui (after logger)
⏳ controls_id linking strategy (decide on approach)

## Next Actions

1. **Run SQL script** on database:
   ```bash
   psql -U GJW -d String_Driver -f create_tables.sql
   ```

2. **Add DbSettings** to `src/config_loader.rs`

3. **Create logger module** `src/machine_state_logger.rs`:
   - Event-driven, non-blocking pattern (like audmon)
   - Captures all required fields
   - Handles optional controls_id

4. **Integrate into operations_gui**:
   - Log before/after operations
   - Get all stepper positions
   - Link to controls_id

5. **Decide on controls_id strategy**:
   - Add to shared memory? (requires audmon change)
   - Query database? (simpler)
   - Post-process matching? (flexible)

## Rules Compliance

✅ **No fallbacks** - Fail-fast if database config missing
✅ **Event-driven** - Non-blocking logging pattern
✅ **Single source of truth** - All settings from Operations struct
✅ **No hammering** - Logging doesn't affect hardware operations
✅ **No renames** - Using existing field names

