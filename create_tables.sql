-- Database Schema Creation Script for Machine State Logging
-- Run with: psql -U GJW -d String_Driver -f create_tables.sql

-- Machine State Table
-- Captures ALL stepper positions, ALL operations control settings, and runtime audio analysis data
CREATE TABLE IF NOT EXISTS machine_state (
    state_id UUID PRIMARY KEY,
    controls_id TEXT,  -- Links to audmon's controls table (text in audmon schema)
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

CREATE INDEX IF NOT EXISTS idx_machine_state_recorded_at ON machine_state(recorded_at);
CREATE INDEX IF NOT EXISTS idx_machine_state_controls_id ON machine_state(controls_id);
CREATE INDEX IF NOT EXISTS idx_machine_state_host ON machine_state(host);

-- Operations Table
-- Logs operation events (z_calibrate, bump_check, z_adjust) with results
CREATE TABLE IF NOT EXISTS operations (
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

CREATE INDEX IF NOT EXISTS idx_operations_recorded_at ON operations(recorded_at);
CREATE INDEX IF NOT EXISTS idx_operations_state_id ON operations(state_id);
CREATE INDEX IF NOT EXISTS idx_operations_type ON operations(operation_type);
CREATE INDEX IF NOT EXISTS idx_operations_host ON operations(host);

-- Example query to verify tables exist
SELECT 
    table_name,
    column_name,
    data_type
FROM information_schema.columns
WHERE table_name IN ('machine_state', 'operations')
ORDER BY table_name, ordinal_position;

