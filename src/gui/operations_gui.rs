/// Standalone Operations GUI binary
/// 
/// Run with: cargo run --bin operations_gui

#[path = "../config_loader.rs"]
mod config_loader;
#[path = "../gpio.rs"]
mod gpio;
#[path = "../operations.rs"]
mod operations;
#[path = "../get_results.rs"]
mod get_results;
#[path = "../machine_state_logger.rs"]
mod machine_state_logger;

use eframe::egui;
use anyhow::Result;
use std::collections::HashSet;
use std::sync::{Arc, Mutex, RwLock, atomic::{AtomicBool, AtomicUsize}};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;
use std::time::{Duration, Instant};
use std::os::unix::net::UnixStream;
use std::process::Command;
use uuid::Uuid;
use chrono::Utc;

/// Type alias for partials slot (matches partials_slot::PartialsSlot pattern)
/// Using get_results::PartialsData type
type PartialsSlot = Arc<Mutex<Option<get_results::PartialsData>>>;

/// Arduino stepper operations implementation using simple Unix socket text commands
/// Sends commands like "rel_move 2 2\n" to stepper_gui's Unix socket listener
struct ArduinoStepperOps {
    socket_path: String,
    stream: Option<UnixStream>,
    connected_once: bool,
}

impl ArduinoStepperOps {
    fn socket_path_for_port(port_path: &str) -> String {
        let port_id = port_path.replace("/", "_").replace("\\", "_");
        format!("/tmp/stepper_gui_{}.sock", port_id)
    }

    fn new(port_path: &str) -> Self {
        // Generate socket path the same way as stepper_gui.rs
        let socket_path = Self::socket_path_for_port(port_path);
        println!("Initializing shared stepper socket target at {}", socket_path);
        Self {
            socket_path,
            stream: None,
            connected_once: false,
        }
    }

    fn socket_path(&self) -> String {
        self.socket_path.clone()
    }
    
    fn ensure_stream(&mut self) -> Result<&mut UnixStream> {
        if self.stream.is_none() {
            if self.connected_once {
                println!(
                    "Stepper socket connection dropped; attempting reconnect to {}",
                    self.socket_path
                );
            } else {
                println!("Connecting to stepper socket {}", self.socket_path);
            }
            let stream = UnixStream::connect(&self.socket_path)
                .map_err(|e| anyhow::anyhow!("Failed to connect to stepper_gui socket at {}: {}", self.socket_path, e))?;
            println!(
                "Stepper socket {} connection {}",
                self.socket_path,
                if self.connected_once { "re-established" } else { "established" }
            );
            self.stream = Some(stream);
            self.connected_once = true;
        }
        Ok(self.stream.as_mut().unwrap())
    }
    /// Send a text command to stepper_gui via Unix socket
    fn send_command(&mut self, cmd: &str) -> Result<()> {
        use std::io::Write;
        
        let cmd_with_newline = format!("{}
", cmd);
        println!("Stepper IPC command: {}", cmd);
        match self.ensure_stream() {
            Ok(stream) => {
                if let Err(e) = stream.write_all(cmd_with_newline.as_bytes()) {
                    println!(
                        "Stepper socket write failed ({}). Resetting connection to {}",
                        e, self.socket_path
                    );
                    // Connection probably dropped; try once more by reconnecting.
                    self.stream = None;
                    let stream = self.ensure_stream()?;
                    stream.write_all(cmd_with_newline.as_bytes())
                        .map_err(|e| anyhow::anyhow!("Failed to write command to socket: {}", e))?;
                    stream.flush()
                        .map_err(|e| anyhow::anyhow!("Failed to flush socket: {}", e))?;
                    Ok(())
                } else {
                    stream.flush()
                        .map_err(|e| anyhow::anyhow!("Failed to flush socket: {}", e))
                }
            }
            Err(e) => Err(e),
        }
    }
    
    /// Read current positions from stepper_gui (not implemented - positions tracked locally)
    /// For now, we'll track positions locally as we move steppers
    fn _get_positions(&self) -> Result<Vec<i32>> {
        // TODO: Could add a "get_positions" command to stepper_gui socket protocol
        // For now, positions are tracked locally in operations_gui
        Ok(vec![])
    }

    fn fetch_positions_from_socket(socket_path: &str) -> Result<Vec<i32>> {
        use std::io::{BufRead, BufReader, Write};
        use std::os::unix::net::UnixStream;

        let mut stream = UnixStream::connect(socket_path)
            .map_err(|e| anyhow::anyhow!("Failed to connect to stepper_gui socket at {}: {}", socket_path, e))?;
        stream
            .write_all(b"get_positions\n")
            .map_err(|e| anyhow::anyhow!("Failed to request positions: {}", e))?;
        stream
            .flush()
            .map_err(|e| anyhow::anyhow!("Failed to flush positions request: {}", e))?;

        let mut reader = BufReader::new(stream);
        let mut response = String::new();
        let bytes = reader
            .read_line(&mut response)
            .map_err(|e| anyhow::anyhow!("Failed to read positions response: {}", e))?;
        if bytes == 0 {
            return Err(anyhow::anyhow!("Stepper GUI closed positions socket without replying"));
        }
        Self::parse_positions_response(&response)
    }

    fn parse_positions_response(response: &str) -> Result<Vec<i32>> {
        let mut tokens = response.trim().split_whitespace();
        match tokens.next() {
            Some("positions") => {
                let mut entries: Vec<(usize, i32)> = Vec::new();
                let mut max_idx: Option<usize> = None;
                for token in tokens {
                    if token.is_empty() {
                        continue;
                    }
                    let (idx_str, val_str) = token
                        .split_once('=')
                        .ok_or_else(|| anyhow::anyhow!("Malformed positions token '{}'", token))?;
                    let idx = idx_str
                        .parse::<usize>()
                        .map_err(|e| anyhow::anyhow!("Invalid stepper index '{}': {}", idx_str, e))?;
                    let value = val_str
                        .parse::<i32>()
                        .map_err(|e| anyhow::anyhow!("Invalid stepper value '{}': {}", val_str, e))?;
                    if let Some(current_max) = max_idx {
                        if idx > current_max {
                            max_idx = Some(idx);
                        }
                    } else {
                        max_idx = Some(idx);
                    }
                    entries.push((idx, value));
                }
                let max_idx = max_idx.unwrap_or(0);
                let mut positions = vec![0i32; max_idx + 1];
                for (idx, value) in entries {
                    if idx < positions.len() {
                        positions[idx] = value;
                    }
                }
                Ok(positions)
            }
            Some(other) => Err(anyhow::anyhow!("Unexpected positions response '{}'", other)),
            None => Err(anyhow::anyhow!("Empty positions response")),
        }
    }
}

impl operations::StepperOperations for ArduinoStepperOps {
    fn rel_move(&mut self, stepper: usize, delta: i32) -> Result<()> {
        self.send_command(&format!("rel_move {} {}", stepper, delta))
    }
    
    fn abs_move(&mut self, stepper: usize, position: i32) -> Result<()> {
        self.send_command(&format!("abs_move {} {}", stepper, position))
    }
    
    fn reset(&mut self, stepper: usize, position: i32) -> Result<()> {
        self.send_command(&format!("reset {} {}", stepper, position))
    }
    
    fn disable(&mut self, _stepper: usize) -> Result<()> {
        // Disable is handled by setting enable state in operations, not a direct Arduino command
        Ok(())
    }
}

/// Operations GUI state
struct OperationsGUI {
    operations: Arc<RwLock<operations::Operations>>,
    message: String,
    partials_slot: PartialsSlot,
    partials_per_channel: Arc<AtomicUsize>,
    selected_operation: String,
    arduino_ops: Option<Arc<Mutex<ArduinoStepperOps>>>,
    // Thresholds for z_adjust operation
    voice_count_min: Vec<i32>,  // Per-channel minimum voice count
    voice_count_max: Vec<i32>,  // Per-channel maximum voice count
    voice_count_min_logger: Option<Arc<Mutex<Vec<i32>>>>,
    voice_count_max_logger: Option<Arc<Mutex<Vec<i32>>>>,
    amp_sum_min: Vec<i32>,      // Per-channel minimum amplitude sum
    amp_sum_max: Vec<i32>,      // Per-channel maximum amplitude sum
    // Track stepper positions locally (updated as we move steppers)
    stepper_positions: Arc<Mutex<std::collections::HashMap<usize, i32>>>,
    // Exit flag to signal operations to stop
    exit_flag: Arc<AtomicBool>,
    // Operation lock to prevent concurrent execution
    operation_running: Arc<AtomicBool>,
    operation_task: Option<OperationTask>,
    repeat_enabled: bool,
    repeat_pending: Option<(String, Instant)>,
    // Machine state logging
    logging_enabled: bool,
    logger: Option<machine_state_logger::MachineStateLoggingContext>,
}

struct OperationTask {
    receiver: Receiver<OperationResult>,
}

struct OperationResult {
    operation: String,
    message: String,
    updated_positions: std::collections::HashMap<usize, i32>,
}

impl OperationsGUI {
    /// Create a new OperationsGUI instance
    fn new() -> Result<Self> {
        // Create a partials slot for shared memory updates
        let partials_slot: PartialsSlot = Arc::new(Mutex::new(None));
        let partials_per_channel = Arc::new(AtomicUsize::new(12));
        
        // Get config to know how many channels to read and Arduino port
        let hostname = gethostname::gethostname().to_string_lossy().to_string();
        let ard_settings = config_loader::load_arduino_settings(&hostname)?;
        let string_num = ard_settings.string_num;
        let port_path = ard_settings.port.clone();
        
        // Create operations with the partials slot (wrap in Arc<Mutex> for sharing with logging thread)
        let operations = Arc::new(RwLock::new(operations::Operations::new_with_partials_slot(Some(Arc::clone(&partials_slot)))?));
        
        // Create Arduino stepper operations client (connects via IPC to stepper_gui's connection)
        let arduino_ops = Arc::new(Mutex::new(ArduinoStepperOps::new(&port_path)));
        
        // Spawn a thread to periodically update the partials slot from shared memory
        let partials_slot_thread = Arc::clone(&partials_slot);
        let partials_setting_for_thread = Arc::clone(&partials_per_channel);
        thread::spawn(move || {
            loop {
                let partial_count = std::cmp::max(
                    1,
                    partials_setting_for_thread.load(std::sync::atomic::Ordering::Relaxed),
                );
                // Read from shared memory and update the slot
                if let Some(partials) = operations::Operations::read_partials_from_shared_memory(
                    string_num,
                    partial_count,
                ) {
                    if let Ok(mut slot) = partials_slot_thread.lock() {
                        *slot = Some(partials);
                    }
                }
                // Update at ~60 Hz to match GUI frame rate
                thread::sleep(Duration::from_millis(16));
            }
        });
        
        // Initialize thresholds with defaults
        let string_num = operations.read().unwrap().string_num;
        let voice_count_cap = std::cmp::max(1, partials_per_channel.load(std::sync::atomic::Ordering::Relaxed) as i32);
        let voice_count_min_default = std::cmp::min(2, voice_count_cap);
        let voice_count_min = vec![voice_count_min_default; string_num];
        let voice_count_max = vec![voice_count_cap; string_num];
        let amp_sum_min = vec![20; string_num];
        let amp_sum_max = vec![250; string_num];
        let stepper_positions: Arc<Mutex<std::collections::HashMap<usize, i32>>> = Arc::new(Mutex::new(std::collections::HashMap::new()));
        {
            let enabled_snapshot = operations.read().unwrap().get_all_stepper_enabled();
            if let Ok(mut map) = stepper_positions.lock() {
                for idx in enabled_snapshot.keys() {
                    map.entry(*idx).or_insert(0);
                }
            }
        }
        
        let stepper_roles_metadata = Arc::new({
            let ops_guard = operations.read().unwrap();
            derive_stepper_roles(&ops_guard, ard_settings.num_steppers)
        });

        // Periodically sync stepper positions from stepper_gui (1 Hz) so logger sees live data
        if let Ok(ops_guard) = arduino_ops.lock() {
            let socket_path_for_poll = ops_guard.socket_path();
            let stepper_positions_for_poll = Arc::clone(&stepper_positions);
            thread::spawn(move || {
                loop {
                    thread::sleep(Duration::from_secs(1));
                    if !std::path::Path::new(&socket_path_for_poll).exists() {
                        continue;
                    }
                    match ArduinoStepperOps::fetch_positions_from_socket(&socket_path_for_poll) {
                        Ok(values) => {
                            if let Ok(mut map) = stepper_positions_for_poll.lock() {
                                for (idx, pos) in values.iter().enumerate() {
                                    map.insert(idx, *pos);
                                }
                            }
                        }
                        Err(e) => {
                            eprintln!("Stepper position poll failed: {}", e);
                        }
                    }
                }
            });
        } else {
            eprintln!("WARNING: Failed to acquire Arduino ops lock for position polling");
        }

        // Initialize machine state logging (non-blocking, can fail silently)
        let logger = config_loader::DbSettings::from_env()
            .ok()
            .and_then(|db_config| {
                Some(machine_state_logger::MachineStateLoggingContext::new_nonblocking(db_config))
            });
        let mut voice_count_min_logger_arc: Option<Arc<Mutex<Vec<i32>>>> = None;
        let mut voice_count_max_logger_arc: Option<Arc<Mutex<Vec<i32>>>> = None;
        
        // Start 1Hz logging thread if logger available
        // Uses existing position arrays - does NOT query Arduino (non-blocking)
        if let Some(ref logger_ref) = logger {
            let logger_clone = logger_ref.clone();
            let operations_clone = Arc::clone(&operations);
            let stepper_positions_clone = Arc::clone(&stepper_positions);
            let voice_count_min_clone = Arc::new(Mutex::new(voice_count_min.clone()));
            let voice_count_max_clone = Arc::new(Mutex::new(voice_count_max.clone()));
            voice_count_min_logger_arc = Some(Arc::clone(&voice_count_min_clone));
            voice_count_max_logger_arc = Some(Arc::clone(&voice_count_max_clone));
            let amp_sum_min_clone = Arc::new(Mutex::new(amp_sum_min.clone()));
            let amp_sum_max_clone = Arc::new(Mutex::new(amp_sum_max.clone()));
            let hostname_clone = hostname.clone();
            let total_steppers = ard_settings.num_steppers;
            let stepper_roles_clone_for_logger = Arc::clone(&stepper_roles_metadata);
            thread::spawn(move || {
                use std::time::Instant;
                let mut last_log = Instant::now();
                const LOG_INTERVAL: Duration = Duration::from_secs(1); // 1Hz
                loop {
                    thread::sleep(Duration::from_millis(100));
                    if Instant::now().duration_since(last_log) >= LOG_INTERVAL {
                        if logger_clone.is_enabled() {
                            // Capture machine state from existing arrays (non-blocking, no Arduino query)
                    if let (Ok(ops), Ok(positions_map), Ok(vc_min), Ok(vc_max), Ok(amp_min), Ok(amp_max)) = 
                        (operations_clone.read(), stepper_positions_clone.lock(), 
                                 voice_count_min_clone.lock(), voice_count_max_clone.lock(),
                                 amp_sum_min_clone.lock(), amp_sum_max_clone.lock()) {
                                
                                // Build complete position array from existing HashMap
                                let mut all_positions = vec![0i32; total_steppers];
                                let mut all_enabled = vec![false; total_steppers];
                                
                                // Fill from positions map (uses existing tracked positions)
                                for (idx, &pos) in positions_map.iter() {
                                    if *idx < all_positions.len() {
                                        all_positions[*idx] = pos;
                                        all_enabled[*idx] = ops.get_stepper_enabled(*idx);
                                    }
                                }
                                
                                // Get all settings from Operations struct
                                let snapshot = machine_state_logger::MachineStateSnapshot {
                                    state_id: Uuid::new_v4(),
                                    controls_id: None, // TODO: Get from audmon shared memory
                                    host: hostname_clone.clone(),
                                    recorded_at: Utc::now(),
                                    stepper_positions: all_positions,
                                    stepper_enabled: all_enabled,
                                    bump_check_enable: ops.get_bump_check_enable(),
                                    z_up_step: ops.get_z_up_step(),
                                    z_down_step: ops.get_z_down_step(),
                                    tune_rest: ops.get_tune_rest(),
                                    x_rest: ops.get_x_rest(),
                                    z_rest: ops.get_z_rest(),
                                    lap_rest: ops.get_lap_rest(),
                                    adjustment_level: ops.get_adjustment_level(),
                                    retry_threshold: ops.get_retry_threshold(),
                                    delta_threshold: ops.get_delta_threshold(),
                                    z_variance_threshold: ops.get_z_variance_threshold(),
                                    voice_count: ops.get_voice_count().iter().map(|&x| x as i32).collect(),
                                    amp_sum: ops.get_amp_sum(),
                                    voice_count_min: vc_min.clone(),
                                    voice_count_max: vc_max.clone(),
                                    amp_sum_min: amp_min.clone(),
                                    amp_sum_max: amp_max.clone(),
                                    stepper_roles: (*stepper_roles_clone_for_logger).clone(),
                                };
                                logger_clone.insert_machine_state(&snapshot);
                            }
                        }
                        last_log = Instant::now();
                    }
                }
            });
        }
        
        Ok(Self {
            operations,
            message: String::new(),
            exit_flag: Arc::new(AtomicBool::new(false)),
            operation_running: Arc::new(AtomicBool::new(false)),
            operation_task: None,
            partials_slot,
            partials_per_channel: Arc::clone(&partials_per_channel),
            selected_operation: "None".to_string(),
            arduino_ops: Some(arduino_ops),
            voice_count_min,
            voice_count_max,
            voice_count_min_logger: voice_count_min_logger_arc,
            voice_count_max_logger: voice_count_max_logger_arc,
            amp_sum_min,
            amp_sum_max,
            stepper_positions: Arc::clone(&stepper_positions),
            repeat_enabled: false,
            repeat_pending: None,
            logging_enabled: logger.is_some(),
            logger,
        })
    }
    
    /// Append message
    fn append_message(&mut self, msg: &str) {
        if !self.message.is_empty() {
            self.message.push('\n');
        }
        self.message.push_str(msg);
    }
    
    fn sync_voice_threshold_caps(&mut self, new_cap: i32) {
        let cap = std::cmp::max(1, new_cap);
        for max_val in self.voice_count_max.iter_mut() {
            if *max_val > cap {
                *max_val = cap;
            }
        }
        for (idx, min_val) in self.voice_count_min.iter_mut().enumerate() {
            if *min_val > cap {
                *min_val = cap;
            }
            if let Some(current_max) = self.voice_count_max.get(idx) {
                if *min_val > *current_max {
                    *min_val = *current_max;
                }
            }
        }
    }
    
    fn publish_voice_thresholds_to_logger(&self) {
        if self.voice_count_min_logger.is_none() && self.voice_count_max_logger.is_none() {
            return;
        }
        let min_snapshot = self.voice_count_min.clone();
        let max_snapshot = self.voice_count_max.clone();
        if let Some(ref arc) = self.voice_count_min_logger {
            if let Ok(mut guard) = arc.lock() {
                *guard = min_snapshot.clone();
            }
        }
        if let Some(ref arc) = self.voice_count_max_logger {
            if let Ok(mut guard) = arc.lock() {
                *guard = max_snapshot;
            }
        }
    }
    
    fn poll_operation_result(&mut self) {
        let mut should_clear = false;
        let mut schedule_repeat_op: Option<String> = None;
        if let Some(task) = self.operation_task.as_mut() {
            match task.receiver.try_recv() {
                Ok(result) => {
                    for (idx, pos) in result.updated_positions {
                        if let Ok(mut positions) = self.stepper_positions.lock() {
                            positions.insert(idx, pos);
                        }
                    }
                    self.append_message(&result.message);
                    self.operation_running.store(false, std::sync::atomic::Ordering::Relaxed);
                    should_clear = true;
                    if self.repeat_enabled && self.selected_operation == result.operation {
                        schedule_repeat_op = Some(result.operation.clone());
                    }
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    self.append_message("Operation worker disconnected unexpectedly");
                    self.operation_running.store(false, std::sync::atomic::Ordering::Relaxed);
                    should_clear = true;
                }
            }
        }

        if should_clear {
            self.operation_task = None;
        }

        if let Some(op) = schedule_repeat_op {
            if self.repeat_enabled {
                let lap_rest = self.operations.read().unwrap().get_lap_rest().max(0.0);
                let wait = if lap_rest <= 0.0 {
                    Duration::from_secs(0)
                } else {
                    Duration::from_secs_f32(lap_rest)
                };
                let deadline = Instant::now() + wait;
                self.repeat_pending = Some((op.clone(), deadline));
                self.append_message(&format!(
                    "Repeat enabled - waiting {:.2}s before re-running {}",
                    lap_rest,
                    op
                ));
            }
        }

        self.try_start_scheduled_repeat();
    }


    /// Execute the selected operation
    fn execute_operation(&mut self) {
        if self.operation_running.load(std::sync::atomic::Ordering::Relaxed) {
            self.append_message("Operation already running - please wait");
            return;
        }

        self.poll_operation_result();

        if self.operation_task.is_some() {
            self.append_message("Operation still completing - please wait");
            return;
        }

        let selected_operation = self.selected_operation.clone();
        if selected_operation == "None" {
            self.append_message("No operation selected");
            return;
        }

        self.start_operation(selected_operation);
    }

    fn try_start_scheduled_repeat(&mut self) {
        if self.repeat_pending.is_none() {
            return;
        }
        if self.operation_running.load(std::sync::atomic::Ordering::Relaxed) || self.operation_task.is_some() {
            return;
        }
        if let Some((op_name, deadline)) = self.repeat_pending.clone() {
            if Instant::now() >= deadline {
                self.repeat_pending = None;
                self.append_message(&format!("Repeat interval elapsed - re-running {}", op_name));
                self.start_operation(op_name);
            }
        }
    }

    fn start_operation(&mut self, operation: String) {
        let arduino_ops = match self.arduino_ops.as_ref() {
            Some(ops) => Arc::clone(ops),
            None => {
                self.append_message("Arduino connection client not available");
                return;
            }
        };

        let z_indices = self.operations.read().unwrap().get_z_stepper_indices();
        if z_indices.is_empty() {
            self.append_message("No Z steppers configured");
            return;
        }

        match operation.as_str() {
            "z_calibrate" => self.append_message("Executing Z Calibrate..."),
            "z_adjust" => self.append_message("Executing Z Adjust..."),
            "bump_check" => self.append_message("Executing Bump Check..."),
            _ => {
                self.append_message("No operation selected");
                return;
            }
        }

        let max_idx = z_indices.iter().max().copied().unwrap_or(0);
        let mut positions = vec![0i32; max_idx + 1];
        let current_positions_snapshot = self.stepper_positions
            .lock()
            .map(|map| map.clone())
            .unwrap_or_default();
        for &idx in &z_indices {
            if idx < positions.len() {
                positions[idx] = current_positions_snapshot.get(&idx).copied().unwrap_or(0);
            }
        }
        let mut max_positions = std::collections::HashMap::new();
        for &idx in &z_indices {
            max_positions.insert(idx, 100);
        }

        let min_thresholds: Vec<f32> = self.amp_sum_min.iter().map(|&v| v as f32).collect();
        let max_thresholds: Vec<f32> = self.amp_sum_max.iter().map(|&v| v as f32).collect();
        let min_voices: Vec<usize> = self.voice_count_min.iter().map(|&v| v.max(0) as usize).collect();
        let max_voices: Vec<usize> = self.voice_count_max.iter().map(|&v| v.max(0) as usize).collect();

        let operations = Arc::clone(&self.operations);
        let exit_flag = Arc::clone(&self.exit_flag);
        let z_indices_clone = z_indices.clone();
        let operation_label = operation.clone();

        let (tx, rx) = mpsc::channel();
        self.operation_task = Some(OperationTask { receiver: rx });
        self.operation_running.store(true, std::sync::atomic::Ordering::Relaxed);

        thread::spawn(move || {
            let mut local_positions = positions;
            let op_name = operation_label;
            let operation_result = {
                let mut stepper_client = match arduino_ops.lock() {
                    Ok(guard) => guard,
                    Err(_) => {
                        let _ = tx.send(OperationResult {
                            operation: op_name.clone(),
                            message: "Error: Arduino client lock poisoned".to_string(),
                            updated_positions: std::collections::HashMap::new(),
                        });
                        return;
                    }
                };
                let ops_guard = match operations.read() {
                    Ok(guard) => guard,
                    Err(_) => {
                        let _ = tx.send(OperationResult {
                            operation: op_name.clone(),
                            message: "Error: Operations lock poisoned".to_string(),
                            updated_positions: std::collections::HashMap::new(),
                        });
                        return;
                    }
                };

                match op_name.as_str() {
                    "z_calibrate" => ops_guard.z_calibrate(&mut *stepper_client, &mut local_positions, &max_positions, Some(&exit_flag)),
                    "z_adjust" => ops_guard.z_adjust(
                        &mut *stepper_client,
                        &mut local_positions,
                        &max_positions,
                        &min_thresholds,
                        &max_thresholds,
                        &min_voices,
                        &max_voices,
                        Some(&exit_flag),
                    ),
                    "bump_check" => ops_guard.bump_check(
                        None,
                        &mut local_positions,
                        &max_positions,
                        &mut *stepper_client,
                        Some(&exit_flag),
                    ),
                    _ => Err(anyhow::anyhow!("Unsupported operation")),
                }
            };

            let message = match op_name.as_str() {
                "bump_check" => match operation_result {
                    Ok(msg) => {
                        if msg.trim().is_empty() {
                            "Bump check complete (no bumps detected).".to_string()
                        } else {
                            msg
                        }
                    }
                    Err(e) => format!("Bump check error: {}", e),
                },
                _ => match operation_result {
                    Ok(msg) => msg,
                    Err(e) => format!("Error: {}", e),
                },
            };

            let mut updated_positions = std::collections::HashMap::new();
            for &idx in &z_indices_clone {
                if idx < local_positions.len() {
                    updated_positions.insert(idx, local_positions[idx]);
                }
            }

            let _ = tx.send(OperationResult { operation: op_name, message, updated_positions });
        });
    }


    /// Kill all processes and close GUI
    fn kill_all(&mut self) {
        self.append_message("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
        self.append_message("KILL ALL triggered - shutting down everything...");
        self.append_message("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
        
        // Set exit flag to stop any running operations
        self.exit_flag.store(true, std::sync::atomic::Ordering::Relaxed);
        
        // Run kill script
        let script_path = std::env::current_dir()
            .unwrap_or_default()
            .join("kill_all.sh");
        
        if script_path.exists() {
            match Command::new("bash")
                .arg(&script_path)
                .output()
            {
                Ok(output) => {
                    let stdout = String::from_utf8_lossy(&output.stdout);
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    if !stdout.is_empty() {
                        self.append_message(&stdout);
                    }
                    if !stderr.is_empty() {
                        self.append_message(&format!("Errors: {}", stderr));
                    }
                }
                Err(e) => {
                    self.append_message(&format!("Failed to run kill script: {}", e));
                }
            }
        } else {
            self.append_message(&format!("Kill script not found at: {}", script_path.display()));
            // Fallback: try pkill directly with SIGKILL (-9) - matching exact patterns from launcher
            // stepper_gui and operations_gui are launched directly by launcher
            let _ = Command::new("pkill")
                .args(&["-9", "-f", "stepper_gui"])
                .output();
            let _ = Command::new("pkill")
                .args(&["-9", "-f", "operations_gui"])
                .output();
            // audio_monitor is launched by persist script at target/release/audio_monitor
            let _ = Command::new("pkill")
                .args(&["-9", "-f", "target/release/audio_monitor"])
                .output();
            let _ = Command::new("pkill")
                .args(&["-9", "-f", "audio_monitor"])
                .output();
            // persist script runs in xterm "Persist Monitor"
            let _ = Command::new("pkill")
                .args(&["-9", "-f", "Persist Monitor"])
                .output();
            let _ = Command::new("pkill")
                .args(&["-9", "-f", "audmon.sh"])
                .output();
            // qjackctl is checked by persist, may be launched separately
            let _ = Command::new("pkill")
                .args(&["-9", "-f", "qjackctl"])
                .output();
            let _ = Command::new("pkill")
                .args(&["-9", "qjackctl"])
                .output();
            // launcher
            let _ = Command::new("pkill")
                .args(&["-9", "-f", "launcher"])
                .output();
            self.append_message("Sent kill signals directly");
        }
        
        // Close this window by exiting process
        // Give a moment for kill script to run, then exit
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(500));
            std::process::exit(0);
        });
    }
}

impl eframe::App for OperationsGUI {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        // Check exit flag and close window if set
        if self.exit_flag.load(std::sync::atomic::Ordering::Relaxed) {
            // Request close via viewport command
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        
        // Request continuous repaints for smooth meter updates
        ctx.request_repaint_after(Duration::from_millis(16)); // ~60 Hz update rate
        
        // Poll for any finished background operations before rendering
        self.poll_operation_result();
        
        // Update audio analysis from partials slot using get_results module
        let partials = get_results::read_partials_from_slot(&self.partials_slot);
        self.operations.read().unwrap().update_audio_analysis_with_partials(partials);
        
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("Operations Control");
            
            // Machine state logging + exit controls
            ui.horizontal(|ui| {
                ui.label("Machine State Logging:");
                if let Some(ref logger) = self.logger {
                    let mut enabled = logger.is_enabled();
                    if ui.checkbox(&mut enabled, "Enabled").changed() {
                        logger.set_enabled(enabled);
                        self.logging_enabled = enabled;
                        self.append_message(&format!("Machine state logging {}", if enabled { "enabled" } else { "disabled" }));
                    }
                } else {
                    ui.label("(Database not configured)");
                }

                ui.add_space(16.0);
                if ui
                    .add(
                        egui::Button::new(
                            egui::RichText::new("EXIT")
                                .color(egui::Color32::from_rgb(220, 32, 32))
                                .strong(),
                        )
                        .min_size(egui::vec2(70.0, 28.0)),
                    )
                    .clicked()
                {
                    self.kill_all();
                }
            });
            
            ui.separator();
            
            // Adjustment parameters
            ui.heading("Adjustment Parameters");
            
            ui.horizontal(|ui| {
                let current_enabled = self.operations.read().unwrap().get_bump_check_enable();
                let mut bump_enabled = current_enabled;
                if ui.checkbox(&mut bump_enabled, "Bump check enabled").changed() {
                    self.operations.read().unwrap().set_bump_check_enable(bump_enabled);
                    self.append_message(&format!("Bump check {}", if bump_enabled { "enabled" } else { "disabled" }));
                    if !bump_enabled {
                        self.repeat_pending = None;
                    }
                }
                ui.label("When off, bump_check commands just report pass");
            });
            
            ui.horizontal(|ui| {
                ui.label("Adjustment Level:");
                let mut adjustment_level = self.operations.read().unwrap().get_adjustment_level();
                let mut drag = egui::DragValue::new(&mut adjustment_level);
                drag = drag.clamp_range(1..=100);
                if ui.add(drag).changed() {
                    self.operations.read().unwrap().set_adjustment_level(adjustment_level);
                    self.append_message(&format!("Adjustment level set to {}", adjustment_level));
                }
            });
            
            ui.horizontal(|ui| {
                ui.label("Retry Threshold:");
                let mut retry_threshold = self.operations.read().unwrap().get_retry_threshold();
                let mut drag = egui::DragValue::new(&mut retry_threshold);
                drag = drag.clamp_range(1..=1000);
                if ui.add(drag).changed() {
                    self.operations.read().unwrap().set_retry_threshold(retry_threshold);
                    self.append_message(&format!("Retry threshold set to {}", retry_threshold));
                }
            });
            
            ui.horizontal(|ui| {
                ui.label("Delta Threshold:");
                let mut delta_threshold = self.operations.read().unwrap().get_delta_threshold();
                let mut drag = egui::DragValue::new(&mut delta_threshold);
                drag = drag.clamp_range(1..=1000);
                if ui.add(drag).changed() {
                    self.operations.read().unwrap().set_delta_threshold(delta_threshold);
                    self.append_message(&format!("Delta threshold set to {}", delta_threshold));
                }
            });
            
            ui.horizontal(|ui| {
                ui.label("Z Variance Threshold:");
                let mut z_variance_threshold = self.operations.read().unwrap().get_z_variance_threshold();
                let mut drag = egui::DragValue::new(&mut z_variance_threshold);
                drag = drag.clamp_range(1..=1000);
                if ui.add(drag).changed() {
                    self.operations.read().unwrap().set_z_variance_threshold(z_variance_threshold);
                    self.append_message(&format!("Z variance threshold set to {}", z_variance_threshold));
                }
            });
            
            ui.separator();
            
            // Rest timing values
            ui.heading("Timing (Rest Values)");
            
            ui.horizontal(|ui| {
                ui.label("Tune Rest:");
                let mut tune_rest = self.operations.read().unwrap().get_tune_rest();
                let mut drag = egui::DragValue::new(&mut tune_rest).speed(0.1);
                drag = drag.clamp_range(0.0..=100.0);
                if ui.add(drag).changed() {
                    self.operations.read().unwrap().set_tune_rest(tune_rest);
                    self.append_message(&format!("Tune rest set to {:.2}", tune_rest));
                }
            });
            
            ui.horizontal(|ui| {
                ui.label("X Rest:");
                let mut x_rest = self.operations.read().unwrap().get_x_rest();
                let mut drag = egui::DragValue::new(&mut x_rest).speed(0.1);
                drag = drag.clamp_range(0.0..=100.0);
                if ui.add(drag).changed() {
                    self.operations.read().unwrap().set_x_rest(x_rest);
                    self.append_message(&format!("X rest set to {:.2}", x_rest));
                }
            });
            
            ui.horizontal(|ui| {
                ui.label("Z Rest:");
                let mut z_rest = self.operations.read().unwrap().get_z_rest();
                let mut drag = egui::DragValue::new(&mut z_rest).speed(0.1);
                drag = drag.clamp_range(0.0..=100.0);
                if ui.add(drag).changed() {
                    self.operations.read().unwrap().set_z_rest(z_rest);
                    self.append_message(&format!("Z rest set to {:.2}", z_rest));
                }
            });
            
            ui.horizontal(|ui| {
                ui.label("Lap Rest:");
                let mut lap_rest = self.operations.read().unwrap().get_lap_rest();
                let mut drag = egui::DragValue::new(&mut lap_rest).speed(0.1);
                drag = drag.clamp_range(0.0..=100.0);
                if ui.add(drag).changed() {
                    self.operations.read().unwrap().set_lap_rest(lap_rest);
                    self.append_message(&format!("Lap rest set to {:.2}", lap_rest));
                }
            });
            
            ui.separator();
            
            // Audio analysis display
            ui.heading("Audio Analysis");
            ui.horizontal(|ui| {
                ui.label("Partials / channel:");
                let mut partials_setting = self.partials_per_channel.load(std::sync::atomic::Ordering::Relaxed) as i32;
                if ui
                    .add(egui::DragValue::new(&mut partials_setting).clamp_range(1..=64))
                    .changed()
                {
                    let new_value = std::cmp::max(1, partials_setting);
                    self.partials_per_channel
                        .store(new_value as usize, std::sync::atomic::Ordering::Relaxed);
                    self.sync_voice_threshold_caps(new_value);
                    self.publish_voice_thresholds_to_logger();
                    self.append_message(&format!("Set partials/channel to {}", new_value));
                }
            });
            
            // Voice count display with horizontal meters and thresholds
            ui.horizontal(|ui| {
                ui.label("Voice Count (per channel):");
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    ui.label("Thresholds");
                });
            });
            
            let voice_count = self.operations.read().unwrap().get_voice_count();
            let voice_cap = std::cmp::max(
                1,
                self.partials_per_channel.load(std::sync::atomic::Ordering::Relaxed) as i32,
            );
            let mut thresholds_changed = false;
            for (ch_idx, count) in voice_count.iter().enumerate() {
                ui.horizontal(|ui| {
                    // Ensure we have enough elements in the vectors
                    if ch_idx >= self.voice_count_max.len() {
                        self.voice_count_max.resize(ch_idx + 1, voice_cap);
                    }
                    if ch_idx >= self.voice_count_min.len() {
                        let min_default = std::cmp::min(2, voice_cap);
                        self.voice_count_min.resize(ch_idx + 1, min_default);
                    }
                    
                    // Left column: Channel label and meter
                    ui.label(format!("Ch {}:", ch_idx));
                    let count_val = *count as i32;
                    let min_threshold = self.voice_count_min[ch_idx];
                    let max_threshold = self.voice_count_max[ch_idx];
                    
                    // Determine color: blue (below min), green (in range), red (above max)
                    let color = if count_val < min_threshold {
                        egui::Color32::from_rgb(0, 100, 255) // Blue
                    } else if count_val <= max_threshold {
                        egui::Color32::from_rgb(0, 200, 0) // Green
                    } else {
                        egui::Color32::from_rgb(255, 0, 0) // Red
                    };
                    
                    let max_threshold_f = max_threshold as f32;
                    let progress = if max_threshold_f > 0.0 {
                        (count_val as f32 / max_threshold_f.max(1.0)).min(1.0)
                    } else {
                        0.0
                    };
                    let progress_bar = egui::ProgressBar::new(progress)
                        .fill(color)
                        .text(format!("{}", count))
                        .desired_width(200.0);
                    ui.add(progress_bar);
                    
                    // Right column: Threshold controls
                    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        let mut max_val = self.voice_count_max[ch_idx];
                        let mut min_val = self.voice_count_min[ch_idx];
                        
                        ui.label("min");
                        ui.add(egui::DragValue::new(&mut min_val).clamp_range(0..=voice_cap));
                        ui.label("max");
                        ui.add(egui::DragValue::new(&mut max_val).clamp_range(0..=voice_cap));
                        
                        if max_val != self.voice_count_max[ch_idx] {
                            self.voice_count_max[ch_idx] = max_val;
                            thresholds_changed = true;
                        }
                        if min_val != self.voice_count_min[ch_idx] {
                            self.voice_count_min[ch_idx] = min_val;
                            thresholds_changed = true;
                        }
                        if self.voice_count_min[ch_idx] > self.voice_count_max[ch_idx] {
                            self.voice_count_min[ch_idx] = self.voice_count_max[ch_idx];
                            thresholds_changed = true;
                        }
                    });
                });
            }
            if thresholds_changed {
                self.publish_voice_thresholds_to_logger();
            }
            
            ui.separator();
            
            // Amp sum display with horizontal meters and thresholds
            ui.horizontal(|ui| {
                ui.label("Amplitude Sum (per channel):");
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    ui.label("Thresholds");
                });
            });
            
            let amp_sum = self.operations.read().unwrap().get_amp_sum();
            for (ch_idx, sum) in amp_sum.iter().enumerate() {
                ui.horizontal(|ui| {
                    // Ensure we have enough elements in the vectors
                    if ch_idx >= self.amp_sum_max.len() {
                        self.amp_sum_max.resize(ch_idx + 1, 250);
                    }
                    if ch_idx >= self.amp_sum_min.len() {
                        self.amp_sum_min.resize(ch_idx + 1, 20);
                    }
                    
                    // Left column: Channel label and meter
                    ui.label(format!("Ch {}:", ch_idx));
                    let sum_val = *sum;
                    let min_threshold = self.amp_sum_min[ch_idx] as f32;
                    let max_threshold = self.amp_sum_max[ch_idx] as f32;
                    
                    // Determine color: blue (below min), green (in range), red (above max)
                    let color = if sum_val < min_threshold {
                        egui::Color32::from_rgb(0, 100, 255) // Blue
                    } else if sum_val <= max_threshold {
                        egui::Color32::from_rgb(0, 200, 0) // Green
                    } else {
                        egui::Color32::from_rgb(255, 0, 0) // Red
                    };
                    
                    let progress = if max_threshold > 0.0 {
                        (sum_val / max_threshold).min(1.0)
                    } else {
                        0.0
                    };
                    let progress_bar = egui::ProgressBar::new(progress)
                        .fill(color)
                        .text(format!("{:.2}", sum))
                        .desired_width(200.0);
                    ui.add(progress_bar);
                    
                    // Right column: Threshold controls
                    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        let mut max_val = self.amp_sum_max[ch_idx];
                        let mut min_val = self.amp_sum_min[ch_idx];
                        
                        ui.label("min");
                        ui.add(egui::DragValue::new(&mut min_val).clamp_range(0..=250));
                        ui.label("max");
                        ui.add(egui::DragValue::new(&mut max_val).clamp_range(0..=250));
                        
                        if max_val != self.amp_sum_max[ch_idx] {
                            self.amp_sum_max[ch_idx] = max_val;
                        }
                        if min_val != self.amp_sum_min[ch_idx] {
                            self.amp_sum_min[ch_idx] = min_val;
                        }
                    });
                });
            }
            
            ui.separator();
            
            // Stepper enable/disable checkboxes
            ui.heading("Stepper Enable/Disable");
            ui.label("(Controls which steppers participate in operations/bump_check)");

            let (z_indices, bump_status, num_pairs, z_first, x_step_index, tuner_indices) = {
                let ops_guard = self.operations.read().unwrap();
                (
                    ops_guard.get_z_stepper_indices(),
                    ops_guard.get_bump_status(),
                    ops_guard.string_num,
                    ops_guard.z_first_index,
                    ops_guard.x_step_index(),
                    ops_guard.tuner_indices(),
                )
            };

            if let Some(x_idx) = x_step_index {
                ui.horizontal(|ui| {
                    let mut enabled = self.operations.read().unwrap().get_stepper_enabled(x_idx);
                    if ui.checkbox(&mut enabled, format!("Stepper {} (X)", x_idx)).changed() {
                        self.operations.read().unwrap().set_stepper_enabled(x_idx, enabled);
                        self.append_message(&format!("Stepper {} {}", x_idx, if enabled { "enabled" } else { "disabled" }));
                    }
                });
            }

            if !tuner_indices.is_empty() {
                ui.label("Tuners:");
                for (t_idx, step_idx) in tuner_indices.iter().enumerate() {
                    let mut enabled = self.operations.read().unwrap().get_stepper_enabled(*step_idx);
                    if ui.checkbox(&mut enabled, format!("Stepper {} (T{})", step_idx, t_idx)).changed() {
                        self.operations.read().unwrap().set_stepper_enabled(*step_idx, enabled);
                        self.append_message(&format!("Stepper {} {}", step_idx, if enabled { "enabled" } else { "disabled" }));
                    }
                }
            }

            let bump_map: std::collections::HashMap<usize, bool> = bump_status.iter().cloned().collect();
            
            // Arrange steppers in pairs matching stepper_gui layout:
            // Left column: "out" stepper (odd index, Stepper2)
            // Right column: "in" stepper (even index, Stepper1)
            
            for row in 0..num_pairs {
                let left_idx = z_first + (row * 2) + 1;  // "out" stepper (odd)
                let right_idx = z_first + (row * 2);     // "in" stepper (even)
                
                // Check if indices are valid
                if !z_indices.contains(&left_idx) || !z_indices.contains(&right_idx) {
                    continue;
                }
                
                ui.horizontal(|ui| {
                    // Left column: "out" stepper (Stepper2)
                    ui.vertical(|ui| {
                        let mut enabled = self.operations.read().unwrap().get_stepper_enabled(left_idx);
                        let is_bumping = bump_map.get(&left_idx).copied().unwrap_or(false);
                        
                        let label = format!("Stepper {} (Z{})", 
                            left_idx, 
                            left_idx - z_first,
                        );
                        
                        ui.horizontal(|ui| {
                            if ui.checkbox(&mut enabled, &label).changed() {
                                self.operations.read().unwrap().set_stepper_enabled(left_idx, enabled);
                                self.append_message(&format!("Stepper {} {}", left_idx, if enabled { "enabled" } else { "disabled" }));
                            }
                            
                            let dot_color = if is_bumping {
                                egui::Color32::from_rgb(220, 0, 0)
                            } else {
                                egui::Color32::from_gray(120)
                            };
                            let (rect, _) = ui.allocate_exact_size(egui::Vec2::new(14.0, 14.0), egui::Sense::hover());
                            ui.painter().circle_filled(rect.center(), 5.0, dot_color);
                        });
                    });
                    
                    // Right column: "in" stepper (Stepper1)
                    ui.vertical(|ui| {
                        let mut enabled = self.operations.read().unwrap().get_stepper_enabled(right_idx);
                        let is_bumping = bump_map.get(&right_idx).copied().unwrap_or(false);
                        
                        let label = format!("Stepper {} (Z{})", 
                            right_idx, 
                            right_idx - z_first,
                        );
                        
                        ui.horizontal(|ui| {
                            if ui.checkbox(&mut enabled, &label).changed() {
                                self.operations.read().unwrap().set_stepper_enabled(right_idx, enabled);
                                self.append_message(&format!("Stepper {} {}", right_idx, if enabled { "enabled" } else { "disabled" }));
                            }
                            
                            let dot_color = if is_bumping {
                                egui::Color32::from_rgb(220, 0, 0)
                            } else {
                                egui::Color32::from_gray(120)
                            };
                            let (rect, _) = ui.allocate_exact_size(egui::Vec2::new(14.0, 14.0), egui::Sense::hover());
                            ui.painter().circle_filled(rect.center(), 5.0, dot_color);
                        });
                    });
                });
            }
            
            ui.separator();
            
            // Operations dropdown menu
            ui.heading("Operations");
            ui.horizontal(|ui| {
                ui.label("Select Operation:");
                egui::ComboBox::from_id_source("operation_select")
                    .selected_text(&self.selected_operation)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.selected_operation, "None".to_string(), "None");
                        ui.selectable_value(&mut self.selected_operation, "z_calibrate".to_string(), "Z Calibrate");
                        ui.selectable_value(&mut self.selected_operation, "z_adjust".to_string(), "Z Adjust");
                        ui.selectable_value(&mut self.selected_operation, "bump_check".to_string(), "Bump Check");
                    });
                
                ui.horizontal(|ui| {
                    if ui.button("Execute").clicked() {
                        self.repeat_pending = None;
                        self.execute_operation();
                    }
                    let mut repeat_flag = self.repeat_enabled;
                    if ui.checkbox(&mut repeat_flag, "Repeat").changed() {
                        self.repeat_enabled = repeat_flag;
                        if !repeat_flag {
                            self.repeat_pending = None;
                        }
                    }
                });
            });
            
            ui.separator();
            
            // Display messages (debug log style)
            ui.collapsing("Messages", |ui| {
                ui.horizontal(|ui| {
                    if ui.button("Copy log").clicked() {
                        let log = self.message.clone();
                        ui.output_mut(|o| o.copied_text = log);
                    }
                });
                egui::ScrollArea::vertical()
                    .max_height(400.0)
                    .auto_shrink([false; 2])
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        ui.add(
                            egui::TextEdit::multiline(&mut self.message)
                                .desired_width(f32::INFINITY)
                                .interactive(true)
                                .code_editor()
                        );
                    });
            });
        });
    }
}

fn derive_stepper_roles(ops: &operations::Operations, total_steppers: usize) -> Vec<machine_state_logger::StepperRoleEntry> {
    let mut roles = Vec::new();
    let mut seen = HashSet::new();

    let mut push_entry = |idx: usize, role: &str, string_index: Option<usize>| {
        if idx < total_steppers && seen.insert(idx) {
            roles.push(machine_state_logger::StepperRoleEntry {
                stepper_index: idx,
                role: role.to_string(),
                string_index,
            });
        }
    };

    for string_idx in 0..ops.string_num {
        let z_in_idx = ops.z_first_index + (string_idx * 2);
        let z_out_idx = z_in_idx + 1;
        push_entry(z_in_idx, "z_in", Some(string_idx));
        push_entry(z_out_idx, "z_out", Some(string_idx));
    }

    if let Some(x_idx) = ops.x_step_index() {
        push_entry(x_idx, "x_axis", None);
    }

    for (t_idx, step_idx) in ops.tuner_indices().iter().enumerate() {
        push_entry(*step_idx, "tuner", Some(t_idx));
    }

    for idx in 0..total_steppers {
        if !seen.contains(&idx) {
            roles.push(machine_state_logger::StepperRoleEntry {
                stepper_index: idx,
                role: "other".to_string(),
                string_index: None,
            });
        }
    }

    roles.sort_by_key(|entry| entry.stepper_index);
    roles
}

fn main() {
    println!("Operations GUI starting...");
    env_logger::init();
    
    println!("Creating OperationsGUI instance...");
    let gui_result = OperationsGUI::new();
    let gui = match gui_result {
        Ok(gui) => {
            println!("✓ OperationsGUI created successfully");
            gui
        }
        Err(e) => {
            eprintln!("✗ Failed to create OperationsGUI: {}", e);
            eprintln!("Error details: {:?}", e);
            std::process::exit(1);
        }
    };
    
    println!("Initializing GUI window...");
    // Position in top right: assume screen width ~1920, window width 420
    // Position at x = screen_width - window_width - margin
    let window_width = 420.0;
    let screen_width = 1920.0; // Default, will be adjusted by window manager if needed
    let top_right_x = screen_width - window_width - 20.0; // 20px margin from right edge
    
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Operations Control")
            .with_inner_size([window_width, 1200.0])
            .with_position(egui::pos2(top_right_x, 0.0)), // Top right
        ..Default::default()
    };
    
    println!("Starting eframe::run_native...");
    if let Err(e) = eframe::run_native(
        "Operations Control",
        options,
        Box::new(|_cc| {
            println!("✓ GUI window created, entering event loop");
            Box::new(gui)
        }),
    ) {
        eprintln!("✗ GUI error: {}", e);
        eprintln!("Error details: {:?}", e);
        std::process::exit(1);
    }
    
    println!("Operations GUI exiting");
}

