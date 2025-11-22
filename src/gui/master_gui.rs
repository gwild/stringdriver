/// Master GUI that combines Stepper GUI, Audmon, and Operations GUI in a single window
/// 
/// Layout:
/// - Left panel: Stepper Control (400px default, resizable 300-600px)
/// - Center panel: Audio Monitor status/info (audmon runs as separate process)
/// - Right panel: Operations Control (600px default, resizable 400-800px)
/// 
/// NOTE: Full integration requires refactoring stepper_gui and operations_gui
/// to extract UI rendering into methods that can be called from panels.
/// For now, this is a proof-of-concept structure.

use eframe::egui;
use std::time::Duration;
use anyhow::Result;

// We'll need to import the GUI structs - but they're in separate binaries
// For now, create a simplified version that shows the structure

pub struct MasterGUI {
    // Placeholder - will need to initialize properly
    stepper_initialized: bool,
    operations_initialized: bool,
}

impl MasterGUI {
    pub fn new() -> Result<Self> {
        // TODO: Initialize stepper_gui and operations_gui properly
        // This requires refactoring to extract initialization logic
        
        Ok(Self {
            stepper_initialized: false,
            operations_initialized: false,
        })
    }
}

impl eframe::App for MasterGUI {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Request regular repaints
        ctx.request_repaint_after(Duration::from_millis(16));
        
        // Left panel: Stepper Control
        egui::SidePanel::left("stepper_panel")
            .resizable(true)
            .default_width(400.0)
            .min_width(300.0)
            .max_width(600.0)
            .show(ctx, |ui| {
                ui.heading("Stepper Control");
                ui.separator();
                ui.label("Stepper GUI will be embedded here");
                ui.label("(Requires refactoring StepperGUI to support panel embedding)");
                if !self.stepper_initialized {
                    ui.label("Status: Not initialized");
                }
            });
        
        // Right panel: Operations Control
        egui::SidePanel::right("operations_panel")
            .resizable(true)
            .default_width(600.0)
            .min_width(400.0)
            .max_width(800.0)
            .show(ctx, |ui| {
                ui.heading("Operations Control");
                ui.separator();
                ui.label("Operations GUI will be embedded here");
                ui.label("(Requires refactoring OperationsGUI to support panel embedding)");
                if !self.operations_initialized {
                    ui.label("Status: Not initialized");
                }
            });
        
        // Center panel: Audio Monitor status
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("Audio Monitor");
            ui.separator();
            ui.label("Audmon runs as a separate process.");
            ui.label("The audio_monitor application provides audio analysis");
            ui.label("and writes partials data to shared memory.");
            ui.separator();
            ui.label("Status: (Check if audmon process is running)");
            ui.label("(Audio visualization could be embedded here if needed)");
        });
    }
}

fn main() {
    println!("Master GUI starting...");
    env_logger::init();
    
    let gui = match MasterGUI::new() {
        Ok(gui) => gui,
        Err(e) => {
            eprintln!("Failed to create MasterGUI: {}", e);
            std::process::exit(1);
        }
    };
    
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("String Driver - Master Control")
            .with_inner_size([1800.0, 1000.0]), // Wide window for three panels
        ..Default::default()
    };
    
    if let Err(e) = eframe::run_native(
        "String Driver - Master Control",
        options,
        Box::new(|_cc| Box::new(gui)),
    ) {
        eprintln!("GUI error: {}", e);
    }
}
