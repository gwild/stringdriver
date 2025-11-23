# Database Schema for Machine State Logging

## Overview
This schema extends the existing audmon database to include machine state tracking from stringdriver. This enables correlation between audio analysis (partials) and physical machine state for ML training.

## Tables

### `machine_state`
Stores complete snapshots of stringdriver machine state: ALL stepper positions, ALL operations control settings, and runtime-calculated audio analysis data.

```sql
CREATE TABLE machine_state (
    state_id UUID PRIMARY KEY,
    controls_id UUID,  -- Links to audmon's controls table for concurrent time-series correlation
    host VARCHAR(255) NOT NULL,
    recorded_at TIMESTAMP WITH TIME ZONE NOT NULL,
    
    -- ALL stepper positions (array matches total number of steppers on system)
    stepper_positions INTEGER[] NOT NULL,
    
    -- ALL stepper enable states (array matches total number of steppers)
    stepper_enabled BOOLEAN[] NOT NULL,
    
    -- Operations control settings
    bump_check_enable BOOLEAN NOT NULL,
    z_up_step INTEGER NOT NULL,
    z_down_step INTEGER NOT NULL,
    
    -- Rest timings (seconds)
    tune_rest REAL NOT NULL,
    x_rest REAL NOT NULL,
    z_rest REAL NOT NULL,
    lap_rest REAL NOT NULL,
    
    -- Adjustment parameters
    adjustment_level INTEGER NOT NULL,
    retry_threshold INTEGER NOT NULL,
    delta_threshold INTEGER NOT NULL,
    z_variance_threshold INTEGER NOT NULL,
    
    -- Runtime-calculated audio analysis data (from partials, per-channel)
    voice_count INTEGER[] NOT NULL,  -- Per-channel voice count from partials
    amp_sum REAL[] NOT NULL,         -- Per-channel amplitude sum from partials
    
    -- Thresholds for z_adjust (per-channel arrays)
    voice_count_min INTEGER[] NOT NULL,
    voice_count_max INTEGER[] NOT NULL,
    amp_sum_min INTEGER[] NOT NULL,
    amp_sum_max INTEGER[] NOT NULL,
    
    FOREIGN KEY (controls_id) REFERENCES controls(controls_id) ON DELETE SET NULL
);

CREATE INDEX idx_machine_state_recorded_at ON machine_state(recorded_at);
CREATE INDEX idx_machine_state_controls_id ON machine_state(controls_id);
CREATE INDEX idx_machine_state_host ON machine_state(host);
```

### `operations`
Logs operation events (z_calibrate, bump_check, z_adjust) with results.

```sql
CREATE TABLE operations (
    operation_id UUID PRIMARY KEY,
    state_id UUID,  -- Links to machine_state snapshot before operation
    host VARCHAR(255) NOT NULL,
    recorded_at TIMESTAMP WITH TIME ZONE NOT NULL,
    
    operation_type VARCHAR(50) NOT NULL,  -- 'z_calibrate', 'bump_check', 'z_adjust'
    operation_status VARCHAR(50) NOT NULL,  -- 'started', 'completed', 'failed', 'cancelled'
    message TEXT,  -- Operation result message
    
    stepper_indices INTEGER[] NOT NULL,  -- Which steppers were involved
    final_positions INTEGER[] NOT NULL,  -- Final positions after operation
    
    FOREIGN KEY (state_id) REFERENCES machine_state(state_id) ON DELETE SET NULL
);

CREATE INDEX idx_operations_recorded_at ON operations(recorded_at);
CREATE INDEX idx_operations_state_id ON operations(state_id);
CREATE INDEX idx_operations_type ON operations(operation_type);
CREATE INDEX idx_operations_host ON operations(host);
```

## Relationships

```
controls (audmon)
    ↓ (controls_id)
machine_state (stringdriver) ←─┐
    ↓ (state_id)                │
operations (stringdriver) ──────┘

partials (audmon)
    ↓ (controls_id links to same controls row)
```

## Usage Pattern

1. **Before data collection:**
   - stringdriver captures current machine state → `machine_state` table
   - Gets `state_id`

2. **During data collection:**
   - audmon captures audio → extracts partials
   - Creates `controls` snapshot → `partials` rows (linked by `controls_id`)
   - stringdriver can link its `machine_state.controls_id` to the same `controls_id`

3. **During operations:**
   - Operation starts → log to `operations` table (status='started')
   - Operation completes → update or insert new row (status='completed', final_positions)
   - Operation fails → log (status='failed', message with error)

## Queries for ML Training

### Get complete dataset (partials + machine state)
```sql
SELECT 
    c.controls_id,
    c.recorded_at,
    p.channel,
    p.frequency,
    p.amplitude,
    ms.stepper_positions,
    ms.z_up_step,
    ms.adjustment_level,
    -- ... other state fields
FROM controls c
JOIN partials p ON c.controls_id = p.controls_id
JOIN machine_state ms ON c.controls_id = ms.controls_id
WHERE c.host = 'stringdriver-3'
  AND c.recorded_at BETWEEN '2025-01-01' AND '2025-01-31'
ORDER BY c.recorded_at, p.channel, p.partial_index;
```

### Track operation history
```sql
SELECT 
    o.recorded_at,
    o.operation_type,
    o.operation_status,
    o.stepper_indices,
    o.final_positions,
    ms.stepper_positions AS initial_positions
FROM operations o
LEFT JOIN machine_state ms ON o.state_id = ms.state_id
WHERE o.host = 'stringdriver-3'
  AND o.recorded_at > NOW() - INTERVAL '7 days'
ORDER BY o.recorded_at DESC;
```

### Find calibration events
```sql
SELECT 
    recorded_at,
    operation_type,
    message,
    stepper_indices
FROM operations
WHERE operation_type = 'z_calibrate'
  AND operation_status = 'completed'
ORDER BY recorded_at DESC
LIMIT 10;
```

## Implementation Notes

- Use UUID v4 for all IDs
- `controls_id` can be NULL if machine state captured independently of audio
- Arrays must match dimensions (e.g., `stepper_positions` length = number of steppers)
- Timestamps use UTC with timezone awareness
- Foreign keys use `ON DELETE SET NULL` to preserve historical data even if parent deleted

