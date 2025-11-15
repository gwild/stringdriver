# Next Steps: Machine State Logging Implementation

## What We've Done

### ✅ Fixed Core Operations
- Fixed `bump_check` and `z_calibrate` erratic behavior
- Corrected `set_position()` to be model-only (no physical move)
- Added `reset_position()` using command 4 (set_stepper) for position sync without movement
- Established anti-hammering rules to protect hardware
- Created comprehensive documentation (VISION.md, CHAT_SUMMARY.md, ANTI_HAMMERING_RULE.md)

### ✅ Database Schema Designed
- Created `DATABASE_SCHEMA.md` with complete PostgreSQL schema
- `machine_state` table: captures stepper positions, config, thresholds
- `operations` table: logs operation events with results
- Links to audmon's `controls` table via `controls_id` for data correlation

## What's Next (Implementation Path)

### 1. Create Database Tables
```bash
# On database server (192.168.1.84)
psql -U GJW -d String_Driver -f create_tables.sql
```

Create `create_tables.sql` with schema from `DATABASE_SCHEMA.md`

### 2. Add DbSettings to stringdriver config_loader.rs
- Copy `DbSettings` struct from audmon
- Use same environment variables (PG_HOST, PG_PASSWORD, etc.)
- Fail-fast if database config missing when logging enabled

### 3. Create Simplified Machine State Logger Module
Key functions needed:
- `log_machine_state(positions, config) -> state_id`
- `log_operation_start(operation_type, steppers) -> operation_id`
- `log_operation_complete(operation_id, final_positions, message)`
- Non-blocking, event-driven (like audmon's db_logger)

### 4. Integrate into operations_gui
**Before starting operation:**
```rust
let state_id = logger.log_machine_state(&positions, &config);
let op_id = logger.log_operation_start("z_calibrate", &stepper_indices);
```

**After operation completes:**
```rust
logger.log_operation_complete(op_id, &final_positions, "Success");
```

### 5. Link to audmon's controls_id
**Read from shared memory:**
- audmon writes `audio_peaks` with partials
- Add `current_controls_id` to shared memory structure
- stringdriver reads it and links `machine_state.controls_id`

This enables correlation: partials ↔ controls ↔ machine_state

### 6. Position Sync Before Operations
**In operations_gui:**
```rust
// Before execute_operation()
if let Ok(current_pos) = stepper_ops.get_positions() {
    self.stepper_positions = current_pos;  // Sync from reality
}
```

This ensures operations start with accurate position model.

## Immediate Action Items

1. **Create SQL script** (`create_tables.sql`) from schema
2. **Run on database** to create tables
3. **Add DbSettings** to stringdriver config_loader
4. **Create minimal logger** (just the essential functions)
5. **Test logging** from operations_gui with one operation

## Why This Matters (Vision Connection)

This completes the **Data Foundation** phase:
- ✅ Audio analysis data (audmon → partials)
- ✅ Machine state data (stringdriver → positions + operations)
- ✅ Correlation (controls_id links both)
- ✅ Time-series (timestamps on everything)

**Next Phase Unlocked:**
- ML training on complete dataset
- Predict optimal positions from partials
- Learn relationships between machine state and audio output
- Build models for automated optimization

## Current Status

**Stable & Working:**
- Operations (bump_check, z_calibrate, z_adjust)
- Position model tracking
- Hardware protection (anti-hammering)
- Shared memory communication

**Ready to Add:**
- Database logging infrastructure
- Machine state capture
- Operation event tracking
- Data correlation with audmon

**Future (After Data Collection):**
- MindsDB ML integration
- Predictive control
- Replay system
- Automated optimization

---

**The foundation is solid. Now we add the data collection layer that enables everything else.**

