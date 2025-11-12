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
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

/// Type alias for partials slot (matches partials_slot::PartialsSlot pattern)
/// Using get_results::PartialsData type
type PartialsSlot = Arc<Mutex<Option<get_results::PartialsData>>>;

/// Arduino stepper operations implementation using simple Unix socket text commands
/// Sends commands like "rel_move 2 2\n" to stepper_gui's Unix socket listener
struct ArduinoStepperOps {
    socket_path: String,
}

impl ArduinoStepperOps {
    fn new(port_path: &str) -> Self {
        // Generate socket path the same way as stepper_gui.rs
        let port_id = port_path.replace("/", "_").replace("\\", "_");
        let socket_path = format!("/tmp/stepper_gui_{}.sock", port_id);
        Self { socket_path }
    }
    
    /// Send a text command to stepper_gui via Unix socket
    fn send_command(&self, cmd: &str) -> Result<()> {
        use std::os::unix::net::UnixStream;
        use std::io::Write;
        
        let mut stream = UnixStream::connect(&self.socket_path)
            .map_err(|e| anyhow::anyhow!("Failed to connect to stepper_gui socket at {}: {}", self.socket_path, e))?;
        
        let cmd_with_newline = format!("{}\n", cmd);
        stream.write_all(cmd_with_newline.as_bytes())
            .map_err(|e| anyhow::anyhow!("Failed to write command to socket: {}", e))?;
        stream.flush()
            .map_err(|e| anyhow::anyhow!("Failed to flush socket: {}", e))?;
        
        Ok(())
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
            partials_slot,
            selected_operation: "None".to_string(),
            arduino_ops: Some(arduino_ops),
            voice_count_min: vec![2; string_num],
            voice_count_max: vec![12; string_num],
            amp_sum_min: vec![20; string_num],
            amp_sum_max: vec![250; string_num],
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
        
        // Get current positions (stub - will need to read from Arduino)
        let z_indices = self.operations.get_z_stepper_indices();
        let mut positions = vec![0i32; z_indices.iter().max().copied().unwrap_or(0) + 1];
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
                        None,
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
                        None,
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
            _ => {
                self.append_message("No operation selected");
            }
        }
    }
}

impl eframe::App for OperationsGUI {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Request continuous repaints for smooth meter updates
        ctx.request_repaint_after(Duration::from_millis(16)); // ~60 Hz update rate
        
        // Update audio analysis from partials slot using get_results module
        let partials = get_results::read_partials_from_slot(&self.partials_slot);
        self.operations.update_audio_analysis_with_partials(partials);
        
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("Operations Control");
            
            ui.separator();
            
            // Bump check enable checkbox
            let mut bump_check_enable = self.operations.get_bump_check_enable();
            ui.checkbox(&mut bump_check_enable, "Bump Check Enable");
            if bump_check_enable != self.operations.get_bump_check_enable() {
                self.operations.set_bump_check_enable(bump_check_enable);
                self.append_message(&format!("Bump check {}", if bump_check_enable { "enabled" } else { "disabled" }));
            }
            
            ui.separator();
            
            // Bump check repeat spinbox
            ui.horizontal(|ui| {
                ui.label("Bump Check Repeat:");
                let mut repeat = self.operations.get_bump_check_repeat() as i32;
                let mut drag = egui::DragValue::new(&mut repeat);
                drag = drag.clamp_range(1..=100);
                if ui.add(drag).changed() {
                    self.operations.set_bump_check_repeat(repeat as u32);
                    self.append_message(&format!("Bump check repeat set to {}", repeat));
                }
            });
            
            ui.separator();
            
            // Z up step spinbox
            ui.horizontal(|ui| {
                ui.label("Z Up Step:");
                let mut z_up_step = self.operations.get_z_up_step();
                let mut drag = egui::DragValue::new(&mut z_up_step);
                drag = drag.clamp_range(2..=10);
                if ui.add(drag).changed() {
                    self.operations.set_z_up_step(z_up_step);
                    self.append_message(&format!("Z up step set to {}", z_up_step));
                }
            });
            
            ui.separator();
            
            // Z down step spinbox
            ui.horizontal(|ui| {
                ui.label("Z Down Step:");
                let mut z_down_step = self.operations.get_z_down_step();
                let mut drag = egui::DragValue::new(&mut z_down_step);
                drag = drag.clamp_range(-10..=-2);
                if ui.add(drag).changed() {
                    self.operations.set_z_down_step(z_down_step);
                    self.append_message(&format!("Z down step set to {}", z_down_step));
                }
            });
            
            ui.separator();
            
            // Bump disable threshold spinbox
            ui.horizontal(|ui| {
                ui.label("Bump Disable Threshold:");
                let mut threshold = self.operations.get_bump_disable_threshold();
                let mut drag = egui::DragValue::new(&mut threshold);
                drag = drag.clamp_range(1..=100);
                if ui.add(drag).changed() {
                    self.operations.set_bump_disable_threshold(threshold);
                    self.append_message(&format!("Bump disable threshold set to {}", threshold));
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
            
            for &stepper_idx in &z_indices {
                ui.horizontal(|ui| {
                    let mut enabled = self.operations.get_stepper_enabled(stepper_idx);
                    let is_bumping = bump_map.get(&stepper_idx).copied().unwrap_or(false);
                    
                    // Create label with bump status indicator
                    let status_indicator = if is_bumping { " 🔴" } else { " ⚪" };
                    let label = format!("Stepper {} (Z{}){}", 
                        stepper_idx, 
                        stepper_idx - self.operations.z_first_index,
                        status_indicator);
                    
                    if ui.checkbox(&mut enabled, &label).changed() {
                        self.operations.set_stepper_enabled(stepper_idx, enabled);
                        self.append_message(&format!("Stepper {} {}", stepper_idx, if enabled { "enabled" } else { "disabled" }));
                    }
                    
                    // Show bump status text
                    if is_bumping {
                        ui.colored_label(egui::Color32::RED, "BUMPING");
                    }
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
                    });
                
                if ui.button("Execute").clicked() && self.selected_operation != "None" {
                    self.execute_operation();
                }
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
    env_logger::init();
    
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Operations Control")
            .with_inner_size([420.0, 1200.0]),
        ..Default::default()
    };
    
    if let Err(e) = eframe::run_native(
        "Operations Control",
        options,
        Box::new(|_cc| {
            match OperationsGUI::new() {
                Ok(gui) => Box::new(gui),
                Err(e) => {
                    eprintln!("Failed to create OperationsGUI: {}", e);
                    std::process::exit(1);
                }
            }
        }),
    ) {
        eprintln!("GUI error: {}", e);
        std::process::exit(1);
    }
}

