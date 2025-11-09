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

/// Operations GUI state
struct OperationsGUI {
    operations: operations::Operations,
    message: String,
    partials_slot: PartialsSlot,
}

impl OperationsGUI {
    /// Create a new OperationsGUI instance
    fn new() -> Result<Self> {
        // Create a partials slot for shared memory updates
        let partials_slot: PartialsSlot = Arc::new(Mutex::new(None));
        
        // Get string_num from config to know how many channels to read
        let hostname = gethostname::gethostname().to_string_lossy().to_string();
        let ard_settings = config_loader::load_arduino_settings(&hostname)?;
        let string_num = ard_settings.string_num;
        
        // Create operations with the partials slot
        let operations = operations::Operations::new_with_partials_slot(Some(Arc::clone(&partials_slot)))?;
        
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
        
        Ok(Self {
            operations,
            message: String::new(),
            partials_slot,
        })
    }
    
    /// Append message
    fn append_message(&mut self, msg: &str) {
        if !self.message.is_empty() {
            self.message.push('\n');
        }
        self.message.push_str(msg);
    }
}

impl eframe::App for OperationsGUI {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
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
            
            // Voice count display
            ui.label("Voice Count (per channel):");
            let voice_count = self.operations.get_voice_count();
            for (ch_idx, count) in voice_count.iter().enumerate() {
                ui.horizontal(|ui| {
                    ui.label(format!("Ch {}: {}", ch_idx, count));
                });
            }
            
            ui.separator();
            
            // Amp sum display
            ui.label("Amplitude Sum (per channel):");
            let amp_sum = self.operations.get_amp_sum();
            for (ch_idx, sum) in amp_sum.iter().enumerate() {
                ui.horizontal(|ui| {
                    ui.label(format!("Ch {}: {:.2}", ch_idx, sum));
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
            
            // Display messages
            if !self.message.is_empty() {
                ui.separator();
                ui.label("Messages:");
                ui.text_edit_multiline(&mut self.message);
            }
        });
    }
}

fn main() {
    env_logger::init();
    
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Operations Control")
            .with_inner_size([400.0, 600.0]),
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

