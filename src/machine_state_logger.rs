/// Machine State Logger for stringdriver
/// 
/// Logs machine state (stepper positions, operations, thresholds) to PostgreSQL database
/// Links to audmon's controls_id for correlation with partials data
/// 
/// Event-driven, non-blocking logging pattern (matches audmon's db_logger)

use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, SyncSender, Receiver};
use std::thread;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use log::{error, info, warn, debug};
use postgres::{Client, NoTls, Statement};
use uuid::Uuid;

use crate::config_loader::DbSettings;

// Event-driven database write commands
enum DbWriteCommand {
    InsertMachineState(MachineStateSnapshot),
    InsertOperation(OperationEvent),
}

#[derive(Clone)]
pub struct MachineStateSnapshot {
    pub state_id: Uuid,
    pub controls_id: Option<Uuid>, // Link to audmon's controls_id if available
    pub host: String,
    pub recorded_at: DateTime<Utc>,
    // Stepper positions (all steppers)
    pub stepper_positions: Vec<i32>, // Index -> position
    // Operations settings
    pub z_up_step: i32,
    pub z_down_step: i32,
    pub adjustment_level: i32,
    pub retry_threshold: i32,
    pub delta_threshold: i32,
    pub z_variance_threshold: i32,
    // Rest timings
    pub tune_rest: f32,
    pub x_rest: f32,
    pub z_rest: f32,
    pub lap_rest: f32,
    // Thresholds for z_adjust
    pub voice_count_min: Vec<i32>, // Per-channel
    pub voice_count_max: Vec<i32>, // Per-channel
    pub amp_sum_min: Vec<i32>,     // Per-channel
    pub amp_sum_max: Vec<i32>,     // Per-channel
    // Stepper enable states
    pub stepper_enabled: Vec<bool>, // Index -> enabled
}

#[derive(Clone)]
pub struct OperationEvent {
    pub operation_id: Uuid,
    pub state_id: Option<Uuid>, // Link to machine state snapshot
    pub host: String,
    pub recorded_at: DateTime<Utc>,
    pub operation_type: String, // "z_calibrate", "bump_check", "z_adjust"
    pub operation_status: String, // "started", "completed", "failed", "cancelled"
    pub message: String, // Operation result message
    pub stepper_indices: Vec<usize>, // Steppers involved
    pub final_positions: Vec<i32>, // Final positions after operation
}

pub struct MachineStateLogger {
    client: Client,
    insert_state_stmt: Statement,
    insert_operation_stmt: Statement,
}

impl MachineStateLogger {
    pub fn new(db_config: &DbSettings) -> Result<Self> {
        let connection_str = format!(
            "host={} port={} user={} password={} dbname={}",
            db_config.host,
            db_config.port,
            db_config.user,
            db_config.password,
            db_config.database,
        );

        eprintln!("━━━ MACHINE STATE DB CONNECTION ATTEMPT ━━━");
        eprintln!("  Host: {}", db_config.host);
        eprintln!("  Port: {}", db_config.port);
        eprintln!("  Database: {}", db_config.database);
        eprintln!("  User: {}", db_config.user);
        eprintln!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━::new(db_config)?;
        
        // Prepare SQL statements
        // Note: Database schema needs to be created - see DATABASE_SCHEMA.md
        let insert_state_stmt = client
            .prepare("INSERT INTO machine_state (state_id, controls_id, host, recorded_at, stepper_positions, z_up_step, z_down_step, adjustment_level, retry_threshold, delta_threshold, z_variance_threshold, tune_rest, x_rest, z_rest, lap_rest, voice_count_min, voice_count_max, amp_sum_min, amp_sum_max, stepper_enabled) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20)")
            .context("Failed to prepare machine_state insert statement")?;

        let insert_operation_stmt = client
            .prepare("INSERT INTO operations (operation_id, state_id, host, recorded_at, operation_type, operation_status, message, stepper_indices, final_positions) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)")
            .context("Failed to prepare operations insert statement")?;

        Ok(Self {
            client,
            insert_state_stmt,
            insert_operation_stmt,
        })
    }

    pub fn insert_machine_state(&mut self, snapshot: &MachineStateSnapshot) -> Result<()> {
        log::debug!(target: "machine_state_logger", "→ INSERT machine_state: id={}, host={}, steppers={}", 
                   snapshot.state_id, snapshot.host, snapshot.stepper_positions.len());
        
        let state_id_str = snapshot.state_id.to_string();
        let controls_id_str = snapshot.controls_id.map(|id| id.to_string());
        
        // Convert Vec<i32> to PostgreSQL array format
        let stepper_positions_array: Vec<i32> = snapshot.stepper_positions.clone();
        let voice_count_min_array: Vec<i32> = snapshot.voice_count_min.clone();
        let voice_count_max_array: Vec<i32> = snapshot.voice_count_max.clone();
        let amp_sum_min_array: Vec<i32> = snapshot.amp_sum_min.clone();
        let amp_sum_max_array: Vec<i32> = snapshot.amp_sum_max.clone();
        let stepper_enabled_array: Vec<bool> = snapshot.stepper_enabled.clone();
        
        self.client
            .execute(
                &self.insert_state_stmt,
                &[&state_id_str,
                  &controls_id_str,
                  &snapshot.host,
                  &snapshot.recorded_at,
                  &stepper_positions_array,
                  &(snapshot.z_up_step as i32),
                  &(snapshot.z_down_step as i32),
                  &(snapshot.adjustment_level as i32),
                  &(snapshot.retry_threshold as i32),
                  &(snapshot.delta_threshold as i32),
                  &(snapshot.z_variance_threshold as i32),
                  &(snapshot.tune_rest as f32),
                  &(snapshot.x_rest as f32),
                  &(snapshot.z_rest as f32),
                  &(snapshot.lap_rest as f32),
                  &voice_count_min_array,
                  &voice_count_max_array,
                  &amp_sum_min_array,
                  &amp_sum_max_array,
                  &stepper_enabled_array],
            )
            .context("Failed to insert machine_state row")?;

        info!(target: "machine_state_logger", "✓ Inserted machine_state row: id={}", snapshot.state_id);
        Ok(())
    }

    pub fn insert_operation(&mut self, event: &OperationEvent) -> Result<()> {
        log::debug!(target: "machine_state_logger", "→ INSERT operation: id={}, type={}, status={}", 
                   event.operation_id, event.operation_type, event.operation_status);
        
        let operation_id_str = event.operation_id.to_string();
        let state_id_str = event.state_id.map(|id| id.to_string());
        
        // Convert Vec<usize> to Vec<i32> for PostgreSQL
        let stepper_indices_array: Vec<i32> = event.stepper_indices.iter().map(|&x| x as i32).collect();
        let final_positions_array: Vec<i32> = event.final_positions.clone();
        
        self.client
            .execute(
                &self.insert_operation_stmt,
                &[&operation_id_str,
                  &state_id_str,
                  &event.host,
                  &event.recorded_at,
                  &event.operation_type,
                  &event.operation_status,
                  &event.message,
                  &stepper_indices_array,
                  &final_positions_array],
            )
            .context("Failed to insert operation row")?;

        info!(target: "machine_state_logger", "✓ Inserted operation row: id={}, type={}", event.operation_id, event.operation_type);
        Ok(())
    }

    fn is_connection_error(err: &anyhow::Error) -> bool {
        let err_str = format!("{:#}", err);
        err_str.contains("connection") || err_str.contains("Connection") || err_str.contains("timeout")
    }
}

/// Logging context for machine state (non-blocking, event-driven)
pub struct MachineStateLoggingContext {
    write_tx: Arc<Mutex<Option<SyncSender<DbWriteCommand>>>>,
    connecting: Arc<AtomicBool>,
}

impl MachineStateLoggingContext {
    pub fn new(db_config: &DbSettings) -> Result<Self> {
        let logger = MachineStateLogger::new(db_config)?;
        let (write_tx, write_rx) = mpsc::sync_channel(100);
        
        // Spawn dedicated DB writer thread
        thread::spawn(move || {
            Self::db_writer_thread(logger, write_rx);
        });
        
        Ok(Self {
            write_tx: Arc::new(Mutex::new(Some(write_tx))),
            connecting: Arc::new(AtomicBool::new(false)),
        })
    }
    
    pub fn new_nonblocking(db_config: DbSettings) -> Self {
        info!(target: "machine_state_logger", "Starting with logging disabled (no blocking connect at startup)");
        
        let write_tx = Arc::new(Mutex::new(None));
        let connecting = Arc::new(AtomicBool::new(false));
        
        // Spawn background connection attempt
        let write_tx_clone = Arc::clone(&write_tx);
        let connecting_clone = Arc::clone(&connecting);
        thread::spawn(move || {
            connecting_clone.store(true, Ordering::Relaxed);
            match MachineStateLogger::new(&db_config) {
                Ok(logger) => {
                    let (tx, rx) = mpsc::sync_channel(100);
                    *write_tx_clone.lock().unwrap() = Some(tx);
                    connecting_clone.store(false, Ordering::Relaxed);
                    info!(target: "machine_state_logger", "Background DB connection successful");
                    Self::db_writer_thread(logger, rx);
                }
                Err(e) => {
                    connecting_clone.store(false, Ordering::Relaxed);
                    warn!(target: "machine_state_logger", "Background DB connection failed: {}", e);
                }
            }
        });
        
        Self {
            write_tx,
            connecting,
        }
    }
    
    // Dedicated DB writer thread - event-driven, processes commands from channel
    fn db_writer_thread(mut logger: MachineStateLogger, write_rx: Receiver<DbWriteCommand>) {
        info!(target: "machine_state_db_writer", "DB writer thread started");
        let mut commands_processed = 0;
        let mut errors = 0;
        
        loop {
            match write_rx.recv() {
                Ok(DbWriteCommand::InsertMachineState(snapshot)) => {
                    commands_processed += 1;
                    if let Err(e) = logger.insert_machine_state(&snapshot) {
                        errors += 1;
                        error!(target: "machine_state_db_writer", "Failed to insert machine_state: {:#}", e);
                        if MachineStateLogger::is_connection_error(&e) {
                            error!(target: "machine_state_db_writer", "Connection lost - shutting down writer thread");
                            break;
                        }
                    }
                }
                Ok(DbWriteCommand::InsertOperation(event)) => {
                    commands_processed += 1;
                    if let Err(e) = logger.insert_operation(&event) {
                        errors += 1;
                        error!(target: "machine_state_db_writer", "Failed to insert operation: {:#}", e);
                        if MachineStateLogger::is_connection_error(&e) {
                            error!(target: "machine_state_db_writer", "Connection lost - shutting down writer thread");
                            break;
                        }
                    }
                }
                Err(_) => {
                    info!(target: "machine_state_db_writer", "Write channel closed - shutting down");
                    break;
                }
            }
        }
        
        info!(target: "machine_state_db_writer", "DB writer thread stopped. Processed: {}, Errors: {}", commands_processed, errors);
    }

    pub fn insert_machine_state(&self, snapshot: &MachineStateSnapshot) {
        if let Ok(guard) = self.write_tx.lock() {
            if let Some(tx) = guard.as_ref() {
                // Non-blocking send - if channel is full or disconnected, drop the write
                match tx.try_send(DbWriteCommand::InsertMachineState(snapshot.clone())) {
                    Ok(_) => {},
                    Err(std::sync::mpsc::TrySendError::Full(_)) => {
                        warn!(target: "machine_state_logger", "DB write buffer full (falling behind) - dropping machine_state snapshot");
                    }
                    Err(std::sync::mpsc::TrySendError::Disconnected(_)) => {
                        debug!(target: "machine_state_logger", "Writer thread disconnected");
                    }
                }
            }
        }
    }

    pub fn insert_operation(&self, event: &OperationEvent) {
        if let Ok(guard) = self.write_tx.lock() {
            if let Some(tx) = guard.as_ref() {
                // Non-blocking send - if channel is full or disconnected, drop the write
                match tx.try_send(DbWriteCommand::InsertOperation(event.clone())) {
                    Ok(_) => {},
                    Err(std::sync::mpsc::TrySendError::Full(_)) => {
                        warn!(target: "machine_state_logger", "DB write buffer full (falling behind) - dropping operation event");
                    }
                    Err(std::sync::mpsc::TrySendError::Disconnected(_)) => {
                        debug!(target: "machine_state_logger", "Writer thread disconnected");
                    }
                }
            }
        }
    }
    
    pub fn ensure_connected(&self) {
        // TODO: Implement reconnection logic if needed
    }
}

