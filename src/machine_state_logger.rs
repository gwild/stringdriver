/// Machine State Logger for stringdriver
/// 
/// Non-blocking, event-driven logging at 1Hz
/// Uses existing position arrays (does NOT query Arduino - avoids blocking)
/// Links to audmon's controls_id for concurrent time-series correlation

use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, SyncSender, Receiver};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use log::{error, info, warn, debug};
use postgres::{Client, NoTls, Statement};
use uuid::Uuid;

use crate::config_loader::DbSettings;

const DB_BUFFER_FULL_MSG: &str = "DB write buffer is full.";

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
    // ALL stepper positions (array matches total number of steppers)
    pub stepper_positions: Vec<i32>,
    // ALL stepper enable states
    pub stepper_enabled: Vec<bool>,
    // Operations control settings
    pub bump_check_enable: bool,
    pub z_up_step: i32,
    pub z_down_step: i32,
    // Rest timings
    pub tune_rest: f32,
    pub x_rest: f32,
    pub z_rest: f32,
    pub lap_rest: f32,
    // Adjustment parameters
    pub adjustment_level: i32,
    pub retry_threshold: i32,
    pub delta_threshold: i32,
    pub z_variance_threshold: i32,
    // Runtime audio analysis (from partials)
    pub voice_count: Vec<i32>, // Per-channel
    pub amp_sum: Vec<f32>,     // Per-channel
    // Thresholds for z_adjust
    pub voice_count_min: Vec<i32>,
    pub voice_count_max: Vec<i32>,
    pub amp_sum_min: Vec<i32>,
    pub amp_sum_max: Vec<i32>,
}

#[derive(Clone)]
pub struct OperationEvent {
    pub operation_id: Uuid,
    pub state_id: Option<Uuid>,
    pub host: String,
    pub recorded_at: DateTime<Utc>,
    pub operation_type: String,
    pub operation_status: String,
    pub message: String,
    pub stepper_indices: Vec<usize>,
    pub final_positions: Vec<i32>,
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
            db_config.host, db_config.port, db_config.user, db_config.password, db_config.database,
        );

        eprintln!("=== MACHINE STATE DB CONNECTION ===");
        eprintln!("  Host: {}", db_config.host);
        eprintln!("  Port: {}", db_config.port);
        eprintln!("  Database: {}", db_config.database);
        eprintln!("============================================================");
        let mut client = Client::connect(&connection_str, NoTls)
            .context("Failed to connect to machine state database")?;

        let insert_state_stmt = client
            .prepare("INSERT INTO machine_state (state_id, controls_id, host, recorded_at, stepper_positions, stepper_enabled, bump_check_enable, z_up_step, z_down_step, tune_rest, x_rest, z_rest, lap_rest, adjustment_level, retry_threshold, delta_threshold, z_variance_threshold, voice_count, amp_sum, voice_count_min, voice_count_max, amp_sum_min, amp_sum_max) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20, $21, $22, $23)")
            .context("Failed to prepare machine state SQL statement.")?;

        let insert_operation_stmt = client
            .prepare("INSERT INTO operations (operation_id, state_id, host, recorded_at, operation_type, operation_status, message, stepper_indices, final_positions) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)")
            .context("Failed to prepare operations SQL statement.")?;

        Ok(Self { client, insert_state_stmt, insert_operation_stmt })
    }

    fn insert_machine_state(&mut self, snapshot: &MachineStateSnapshot) -> Result<()> {
        let state_id_str = snapshot.state_id.to_string();
        let controls_id_str = snapshot.controls_id.map(|id| id.to_string());
        self.client.execute(&self.insert_state_stmt, &[
            &state_id_str, &controls_id_str, &snapshot.host, &snapshot.recorded_at,
            &snapshot.stepper_positions, &snapshot.stepper_enabled,
            &snapshot.bump_check_enable, &(snapshot.z_up_step as i32), &(snapshot.z_down_step as i32),
            &(snapshot.tune_rest as f32), &(snapshot.x_rest as f32), &(snapshot.z_rest as f32), &(snapshot.lap_rest as f32),
            &(snapshot.adjustment_level as i32), &(snapshot.retry_threshold as i32), &(snapshot.delta_threshold as i32), &(snapshot.z_variance_threshold as i32),
            &snapshot.voice_count.iter().map(|&x| x as i32).collect::<Vec<i32>>(), &snapshot.amp_sum,
            &snapshot.voice_count_min, &snapshot.voice_count_max, &snapshot.amp_sum_min.iter().map(|&x| x as i32).collect::<Vec<i32>>(), &snapshot.amp_sum_max.iter().map(|&x| x as i32).collect::<Vec<i32>>(),
        ]).context("Failed to insert machine state record.")?;
        info!(target: "machine_state_logger", "Inserted machine state: id={}", snapshot.state_id);
        Ok(())
    }

    fn insert_operation(&mut self, event: &OperationEvent) -> Result<()> {
        let operation_id_str = event.operation_id.to_string();
        let state_id_str = event.state_id.map(|id| id.to_string());
        let stepper_indices_array: Vec<i32> = event.stepper_indices.iter().map(|&x| x as i32).collect();
        self.client.execute(&self.insert_operation_stmt, &[
            &operation_id_str, &state_id_str, &event.host, &event.recorded_at,
            &event.operation_type, &event.operation_status, &event.message,
            &stepper_indices_array, &event.final_positions,
        ]).context("Failed to insert operation record.")?;
        info!(target: "machine_state_logger", "Inserted operation: id={}, type={}", event.operation_id, event.operation_type);
        Ok(())
    }
}

/// Logging context - non-blocking, event-driven
#[derive(Clone)]
pub struct MachineStateLoggingContext {
    write_tx: Arc<Mutex<Option<SyncSender<DbWriteCommand>>>>,
    enabled: Arc<AtomicBool>,
}

impl MachineStateLoggingContext {
    pub fn new(db_config: &DbSettings) -> Result<Self> {
        let logger = MachineStateLogger::new(db_config)?;
        let (write_tx, write_rx) = mpsc::sync_channel(100);
        thread::spawn(move || {
            Self::db_writer_thread(logger, write_rx);
        });
        Ok(Self {
            write_tx: Arc::new(Mutex::new(Some(write_tx))),
            enabled: Arc::new(AtomicBool::new(true)),
        })
    }

    pub fn new_nonblocking(db_config: DbSettings) -> Self {
        let write_tx = Arc::new(Mutex::new(None));
        let enabled = Arc::new(AtomicBool::new(false));
        let write_tx_clone = Arc::clone(&write_tx);
        let enabled_clone = Arc::clone(&enabled);
        thread::spawn(move || {
            match MachineStateLogger::new(&db_config) {
                Ok(logger) => {
                    let (tx, rx) = mpsc::sync_channel(100);
                    *write_tx_clone.lock().unwrap() = Some(tx);
                    enabled_clone.store(true, Ordering::Relaxed);
                    Self::db_writer_thread(logger, rx);
                }
                Err(e) => warn!(target: "machine_state_logger", "Background DB connection failed: {}", e),
            }
        });
        Self { write_tx, enabled }
    }

    fn db_writer_thread(mut logger: MachineStateLogger, write_rx: Receiver<DbWriteCommand>) {
        info!(target: "machine_state_db_writer", "DB writer thread is active.");
        let mut commands_processed = 0;
        let mut errors = 0;
        loop {
            match write_rx.recv() {
                Ok(DbWriteCommand::InsertMachineState(snapshot)) => {
                    commands_processed += 1;
                    if let Err(e) = logger.insert_machine_state(&snapshot) {
                        errors += 1;
                        error!(target: "machine_state_db_writer", "Failed to insert: {:#}", e);
                    }
                }
                Ok(DbWriteCommand::InsertOperation(event)) => {
                    commands_processed += 1;
                    if let Err(e) = logger.insert_operation(&event) {
                        errors += 1;
                        error!(target: "machine_state_db_writer", "Failed to insert: {:#}", e);
                    }
                }
                Err(_) => break,
            }
        }
        info!(target: "machine_state_db_writer", "DB writer stopped. Processed: {}, Errors: {}", commands_processed, errors);
    }

    pub fn insert_machine_state(&self, snapshot: &MachineStateSnapshot) {
        if !self.enabled.load(Ordering::Relaxed) { return; }
        if let Ok(guard) = self.write_tx.lock() {
            if let Some(tx) = guard.as_ref() {
                match tx.try_send(DbWriteCommand::InsertMachineState(snapshot.clone())) {
                    Ok(_) => {},
                    Err(std::sync::mpsc::TrySendError::Full(_)) => {
                        warn!(target: "machine_state_logger", "{}", DB_BUFFER_FULL_MSG);
                    }
                    Err(_) => {},
                }
            }
        }
    }

    pub fn insert_operation(&self, event: &OperationEvent) {
        if !self.enabled.load(Ordering::Relaxed) { return; }
        if let Ok(guard) = self.write_tx.lock() {
            if let Some(tx) = guard.as_ref() {
                match tx.try_send(DbWriteCommand::InsertOperation(event.clone())) {
                    Ok(_) => {},
                    Err(std::sync::mpsc::TrySendError::Full(_)) => {
                        warn!(target: "machine_state_logger", "DB write buffer is full.");
                    }
                    Err(_) => {},
                }
            }
        }
    }

    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::Relaxed);
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }
}

