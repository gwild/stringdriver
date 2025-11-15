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

use eframe::egui;
use anyhow::Result;
use std::sync::{Arc, Mutex, atomic::AtomicBool};
use std::thread;
use std::time::Duration;
use std::os::unix::net::UnixStream;
use std::process::Command;

/// Type alias for partials slot (matches partials_slot::PartialsSlot pattern)
/// Using get_results::PartialsData type
type PartialsSlot = Arc<Mutex<Option<get_results::PartialsData>>>;

/// Arduino stepper operations implementation using simple Unix socket text commands
/// Sends commands like "rel_move 2 2\n" to stepper_gui's Unix socket listener
struct ArduinoStepperOps {
    socket_path: String,
    stream: Option<UnixStream>,
}

impl ArduinoStepperOps {
    fn new(port_path: &str) -> Self {
        // Generate socket path the same way as stepper_gui.rs
        let port_id = port_path.replace("/", "_").replace("\\", "_");
        let socket_path = format!("/tmp/stepper_gui_{}.sock", port_id);
        Self { socket_path, stream: None }
    }
    
    fn ensure_stream(&mut self) -> Result<&mut UnixStream> {
        if self.stream.is_none() {
            let stream = UnixStream::connect(&self.socket_path)
                .map_err(|e| anyhow::anyhow!("Failed to connect to stepper_gui socket at {}: {}", self.socket_path, e))?;
            self.stream = Some(stream);
        }
        Ok(self.stream.as_mut().unwrap())
    }
    
    /// Send a text command to stepper_gui via Unix socket
    fn send_command(&mut self, cmd: &str) -> Result<()> {
        use std::io::Write;
        
        let cmd_with_newline = format!("{}\n", cmd);
        match self.ensure_stream() {
            Ok(stream) => {
                if let Err(e) = stream.write_all(cmd_with_newline.as_bytes()) {
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
    operations: operations::Operations,
    message: String,
    partials_slot: PartialsSlot,
    selected_operation: String,
    arduino_ops: Option<ArduinoStepperOps>,
    // Thresholds for z_adjust operation
    voice_count_min: Vec<i32>,  // Per-channel minimum voice count
    voice_count_max: Vec<i32>,  // Per-channel maximum voice count
    amp_sum_min: Vec<i32>,      // Per-channel minimum amplitude sum
    amp_sum_max: Vec<i32>,      // Per-channel maximum amplitude sum
    // Track stepper positions locally (updated as we move steppers)
    stepper_positions: std::collections::HashMap<usize, i32>,
    // Exit flag to signal operations to stop
    exit_flag: Arc<AtomicBool>,
}

impl OperationsGUI {
    /// Create a new OperationsGUI instance
    fn new() -> Result<Self> {
        // Create a partials slot for shared memory updates
        let partials_slot: PartialsSlot = Arc::new(Mutex::new(None));
        
        // Get config to know how many channels to read and Arduino port
        let hostname = gethostname::gethostname().to_string_lossy().to_string();
        let ard_settings = config_loader::load_arduino_settings(&hostname)?;
        let string_num = ard_settings.string_num;
        let port_path = ard_settings.port.clone();
        
        // Create operations with the partials slot
        let operations = operations::Operations::new_with_partials_slot(Some(Arc::clone(&partials_slot)))?;
        
        // Create Arduino stepper operations client (connects via IPC to stepper_gui's connection)
        let arduino_ops = ArduinoStepperOps::new(&port_path);
        
        // Spawn a thread to periodically update the partials slot from shared memory
        let partials_slot_thread = Arc::clone(&partials_slot);
        thread::spawn(move || {
            const DEFAULT_NUM_PARTIALS: usize = 12;
            loop {
                // Read from shared memory and update the slot
                if let Some(partials) = operations::Operations::read_partials_from_shared_memory(
                    string_num,
                    DEFAULT_NUM_PARTIALS
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
        let string_num = operations.string_num;
        Ok(Self {
            operations,
            message: String::new(),
            exit_flag: Arc::new(AtomicBool::new(false)),
            partials_slot,
            selected_operation: "None".to_string(),
            arduino_ops: Some(arduino_ops),
            voice_count_min: vec![2; string_num],
            voice_count_max: vec![12; string_num],
            amp_sum_min: vec![20; string_num],
            amp_sum_max: vec![250; string_num],
            stepper_positions: std::collections::HashMap::new(),
        })
    }
    
    /// Append message
    fn append_message(&mut self, msg: &str) {
        if !self.message.is_empty() {
            self.message.push('\n');
        }
        self.message.push_str(msg);
    }
    
    /// Execute the selected operation
    fn execute_operation(&mut self) {
        // Check if arduino_ops is available first
        if self.arduino_ops.is_none() {
            self.append_message("Arduino connection client not available");
            return;
        }
        
        // Get current positions - use tracked positions, defaulting to 0
        let z_indices = self.operations.get_z_stepper_indices();
        let max_idx = z_indices.iter().max().copied().unwrap_or(0);
        let mut positions = vec![0i32; max_idx + 1];
        for &idx in &z_indices {
            positions[idx] = self.stepper_positions.get(&idx).copied().unwrap_or(0);
        }
        let mut max_positions = std::collections::HashMap::new();
        for &idx in &z_indices {
            max_positions.insert(idx, 100); // Default max position
        }
        
        match self.selected_operation.as_str() {
            "z_calibrate" => {
                self.append_message("Executing Z Calibrate...");
                // Use scoped block to limit borrow lifetime
                let result = {
                    let stepper_ops = self.arduino_ops.as_mut().unwrap();
                    self.operations.z_calibrate(
                        stepper_ops,
                        &mut positions,
                        &max_positions,
                        Some(&self.exit_flag),
                    )
                };
                match result {
                    Ok(msg) => {
                        self.append_message(&msg);
                    }
                    Err(e) => {
                        self.append_message(&format!("Error: {}", e));
                    }
                }
            }
            "z_adjust" => {
                self.append_message("Executing Z Adjust...");
                // Use thresholds from GUI
                let min_thresholds: Vec<f32> = self.amp_sum_min.iter().map(|&v| v as f32).collect();
                let max_thresholds: Vec<f32> = self.amp_sum_max.iter().map(|&v| v as f32).collect();
                let min_voices: Vec<usize> = self.voice_count_min.iter().map(|&v| v.max(0) as usize).collect();
                let max_voices: Vec<usize> = self.voice_count_max.iter().map(|&v| v.max(0) as usize).collect();
                
                // Use scoped block to limit borrow lifetime
                let result = {
                    let stepper_ops = self.arduino_ops.as_mut().unwrap();
                    self.operations.z_adjust(
                        stepper_ops,
                        &mut positions,
                        &min_thresholds,
                        &max_thresholds,
                        &min_voices,
                        &max_voices,
                        Some(&self.exit_flag),
                    )
                };
                match result {
                    Ok(msg) => {
                        // Update tracked positions after z_adjust (handles negative positions)
                        for &idx in &z_indices {
                            if let Some(&pos) = positions.get(idx) {
                                self.stepper_positions.insert(idx, pos);
                            }
                        }
                        self.append_message(&msg);
                    }
                    Err(e) => {
                        self.append_message(&format!("Error: {}", e));
                    }
                }
            }
            "bump_check" => {
                self.append_message("Executing Bump Check...");
                if z_indices.is_empty() {
                    self.append_message("No Z steppers configured");
                    return;
                }
                let result = {
                    let stepper_ops = self.arduino_ops.as_mut().unwrap();
                    self.operations.bump_check(
                        None,
                        &mut positions,
                        &max_positions,
                        stepper_ops,
                        Some(&self.exit_flag),
                    )
                };
                match result {
                    Ok(msg) => {
                        for &idx in &z_indices {
                            if idx < positions.len() {
                                self.stepper_positions.insert(idx, positions[idx]);
                            }
                        }
                        if msg.trim().is_empty() {
                            self.append_message("Bump check complete (no bumps detected).");
                        } else {
                            self.append_message(&msg);
                        }
                    }
                    Err(e) => self.append_message(&format!("Bump check error: {}", e)),
                }
            }
            _ => {
                self.append_message("No operation selected");
            }
        }
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
            // Fallback: try pkill directly
            let _ = Command::new("pkill")
                .args(&["-f", "stepper_gui"])
                .output();
            let _ = Command::new("pkill")
                .args(&["-f", "operations_gui"])
                .output();
            let _ = Command::new("pkill")
                .args(&["-f", "audio_monitor"])
                .output();
            let _ = Command::new("pkill")
                .args(&["-f", "audmon"])
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
        
        // Update audio analysis from partials slot using get_results module
        let partials = get_results::read_partials_from_slot(&self.partials_slot);
        self.operations.update_audio_analysis_with_partials(partials);
        
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("Operations Control");
            
            ui.separator();
            
            // Adjustment parameters
            ui.heading("Adjustment Parameters");
            
            ui.horizontal(|ui| {
                ui.label("Adjustment Level:");
                let mut adjustment_level = self.operations.get_adjustment_level();
                let mut drag = egui::DragValue::new(&mut adjustment_level);
                drag = drag.clamp_range(1..=100);
                if ui.add(drag).changed() {
                    self.operations.set_adjustment_level(adjustment_level);
                    self.append_message(&format!("Adjustment level set to {}", adjustment_level));
                }
            });
            
            ui.horizontal(|ui| {
                ui.label("Retry Threshold:");
                let mut retry_threshold = self.operations.get_retry_threshold();
                let mut drag = egui::DragValue::new(&mut retry_threshold);
                drag = drag.clamp_range(1..=1000);
                if ui.add(drag).changed() {
                    self.operations.set_retry_threshold(retry_threshold);
                    self.append_message(&format!("Retry threshold set to {}", retry_threshold));
                }
            });
            
            ui.horizontal(|ui| {
                ui.label("Delta Threshold:");
                let mut delta_threshold = self.operations.get_delta_threshold();
                let mut drag = egui::DragValue::new(&mut delta_threshold);
                drag = drag.clamp_range(1..=1000);
                if ui.add(drag).changed() {
                    self.operations.set_delta_threshold(delta_threshold);
                    self.append_message(&format!("Delta threshold set to {}", delta_threshold));
                }
            });
            
            ui.horizontal(|ui| {
                ui.label("Z Variance Threshold:");
                let mut z_variance_threshold = self.operations.get_z_variance_threshold();
                let mut drag = egui::DragValue::new(&mut z_variance_threshold);
                drag = drag.clamp_range(1..=1000);
                if ui.add(drag).changed() {
                    self.operations.set_z_variance_threshold(z_variance_threshold);
                    self.append_message(&format!("Z variance threshold set to {}", z_variance_threshold));
                }
            });
            
            ui.separator();
            
            // Rest timing values
            ui.heading("Timing (Rest Values)");
            
            ui.horizontal(|ui| {
                ui.label("Tune Rest:");
                let mut tune_rest = self.operations.get_tune_rest();
                let mut drag = egui::DragValue::new(&mut tune_rest).speed(0.1);
                drag = drag.clamp_range(0.0..=100.0);
                if ui.add(drag).changed() {
                    self.operations.set_tune_rest(tune_rest);
                    self.append_message(&format!("Tune rest set to {:.2}", tune_rest));
                }
            });
            
            ui.horizontal(|ui| {
                ui.label("X Rest:");
                let mut x_rest = self.operations.get_x_rest();
                let mut drag = egui::DragValue::new(&mut x_rest).speed(0.1);
                drag = drag.clamp_range(0.0..=100.0);
                if ui.add(drag).changed() {
                    self.operations.set_x_rest(x_rest);
                    self.append_message(&format!("X rest set to {:.2}", x_rest));
                }
            });
            
            ui.horizontal(|ui| {
                ui.label("Z Rest:");
                let mut z_rest = self.operations.get_z_rest();
                let mut drag = egui::DragValue::new(&mut z_rest).speed(0.1);
                drag = drag.clamp_range(0.0..=100.0);
                if ui.add(drag).changed() {
                    self.operations.set_z_rest(z_rest);
                    self.append_message(&format!("Z rest set to {:.2}", z_rest));
                }
            });
            
            ui.horizontal(|ui| {
                ui.label("Lap Rest:");
                let mut lap_rest = self.operations.get_lap_rest();
                let mut drag = egui::DragValue::new(&mut lap_rest).speed(0.1);
                drag = drag.clamp_range(0.0..=100.0);
                if ui.add(drag).changed() {
                    self.operations.set_lap_rest(lap_rest);
                    self.append_message(&format!("Lap rest set to {:.2}", lap_rest));
                }
            });
            
            ui.separator();
            
            // Audio analysis display
            ui.heading("Audio Analysis");
            
            // Voice count display with horizontal meters and thresholds
            ui.horizontal(|ui| {
                ui.label("Voice Count (per channel):");
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    ui.label("Thresholds");
                });
            });
            
            let voice_count = self.operations.get_voice_count();
            const NUM_PARTIALS: f32 = 12.0; // Number of partials per channel
            for (ch_idx, count) in voice_count.iter().enumerate() {
                ui.horizontal(|ui| {
                    // Ensure we have enough elements in the vectors
                    if ch_idx >= self.voice_count_max.len() {
                        self.voice_count_max.resize(ch_idx + 1, 12);
                    }
                    if ch_idx >= self.voice_count_min.len() {
                        self.voice_count_min.resize(ch_idx + 1, 2);
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
                        ui.add(egui::DragValue::new(&mut min_val).clamp_range(0..=12));
                        ui.label("max");
                        ui.add(egui::DragValue::new(&mut max_val).clamp_range(0..=12));
                        
                        if max_val != self.voice_count_max[ch_idx] {
                            self.voice_count_max[ch_idx] = max_val;
                        }
                        if min_val != self.voice_count_min[ch_idx] {
                            self.voice_count_min[ch_idx] = min_val;
                        }
                    });
                });
            }
            
            ui.separator();
            
            // Amp sum display with horizontal meters and thresholds
            ui.horizontal(|ui| {
                ui.label("Amplitude Sum (per channel):");
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    ui.label("Thresholds");
                });
            });
            
            let amp_sum = self.operations.get_amp_sum();
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
            
            let z_indices = self.operations.get_z_stepper_indices();
            let bump_status = self.operations.get_bump_status();
            let bump_map: std::collections::HashMap<usize, bool> = bump_status.iter().cloned().collect();
            
            // Arrange steppers in pairs matching stepper_gui layout:
            // Left column: "out" stepper (odd index, Stepper2)
            // Right column: "in" stepper (even index, Stepper1)
            let num_pairs = self.operations.string_num;
            let z_first = self.operations.z_first_index;
            
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
                        let mut enabled = self.operations.get_stepper_enabled(left_idx);
                        let is_bumping = bump_map.get(&left_idx).copied().unwrap_or(false);
                        
                        let status_indicator = if is_bumping { " 🔴" } else { " ⚪" };
                        let label = format!("Stepper {} (Z{}){}", 
                            left_idx, 
                            left_idx - z_first,
                            status_indicator);
                        
                        if ui.checkbox(&mut enabled, &label).changed() {
                            self.operations.set_stepper_enabled(left_idx, enabled);
                            self.append_message(&format!("Stepper {} {}", left_idx, if enabled { "enabled" } else { "disabled" }));
                        }
                        
                        if is_bumping {
                            ui.colored_label(egui::Color32::RED, "BUMPING");
                        }
                    });
                    
                    // Right column: "in" stepper (Stepper1)
                    ui.vertical(|ui| {
                        let mut enabled = self.operations.get_stepper_enabled(right_idx);
                        let is_bumping = bump_map.get(&right_idx).copied().unwrap_or(false);
                        
                        let status_indicator = if is_bumping { " 🔴" } else { " ⚪" };
                        let label = format!("Stepper {} (Z{}){}", 
                            right_idx, 
                            right_idx - z_first,
                            status_indicator);
                        
                        if ui.checkbox(&mut enabled, &label).changed() {
                            self.operations.set_stepper_enabled(right_idx, enabled);
                            self.append_message(&format!("Stepper {} {}", right_idx, if enabled { "enabled" } else { "disabled" }));
                        }
                        
                        if is_bumping {
                            ui.colored_label(egui::Color32::RED, "BUMPING");
                        }
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
                    if ui.button("Execute").clicked() && self.selected_operation != "None" {
                        self.execute_operation();
                    }
                    if ui.button("KILL ALL").clicked() {
                        self.kill_all();
                    }
                });
            });
            
            ui.separator();
            
            // Display messages (debug log style)
            ui.collapsing("Messages", |ui| {
                egui::ScrollArea::vertical()
                    .max_height(400.0)
                    .auto_shrink([false; 2])
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        ui.add(
                            egui::TextEdit::multiline(&mut self.message)
                                .desired_width(f32::INFINITY)
                                .interactive(false)
                        );
                    });
            });
        });
    }
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

