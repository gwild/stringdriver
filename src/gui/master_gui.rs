/// Master GUI that combines Stepper GUI, Audmon, and Operations GUI in a single window
/// 
/// Layout:
/// - Left panel: Stepper Control (400px default, resizable 300-600px)
/// - Center panel: Audio Monitor status/info (audmon runs as separate process)
/// - Right panel: Operations Control (600px default, resizable 400-800px)

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

// Include the GUI structs as modules so we can use them
// We'll include just the struct definitions and impl blocks we need
#[path = "stepper_gui.rs"]
mod stepper_gui_mod;
#[path = "operations_gui.rs"]
mod operations_gui_mod;

use eframe::egui;
use std::time::{Duration, Instant};
use anyhow::Result;
use gethostname::gethostname;
use std::fs::File;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock, mpsc};
use std::collections::VecDeque;

// Use audmon crate (added as path dependency)
use audio_monitor::plot::MyApp;
use audio_monitor::fft_analysis::{FFTConfig, MAX_SPECTROGRAPH_HISTORY};
use audio_monitor::plot::SpectrographSlice;
use audio_monitor::audio_stream::CircularBuffer;
use audio_monitor::get_results::{ResynthConfig, GuiParameter, DEFAULT_UPDATE_RATE};
use audio_monitor::presets::PresetManager;
use audio_monitor::partials_slot::PartialsSlot;
use audio_monitor::plot::SpectrumApp;
use audio_monitor::{DEFAULT_BUFFER_SIZE, DEFAULT_NUM_PARTIALS};

pub struct MasterGUI {
    stepper_gui: Option<stepper_gui_mod::StepperGUI>,
    operations_gui: Option<operations_gui_mod::OperationsGUI>,
    audmon_gui: Option<MyApp>,
}

impl MasterGUI {
    /// Check if audmon process is running
    fn check_audmon_process() -> bool {
        use std::process::Command;
        // Check for audio_monitor process
        let output = Command::new("pgrep")
            .arg("-f")
            .arg("target/release/audio_monitor")
            .output();
        
        match output {
            Ok(output) => output.status.success(),
            Err(_) => false,
        }
    }
    
    /// Read control file directly (since read_control_file is private)
    fn read_control_file_direct(control_path: &str) -> Option<(usize, usize)> {
        let content = std::fs::read_to_string(control_path).ok()?;
        let lines: Vec<&str> = content.trim().split('\n').collect();
        if lines.len() >= 3 {
            // Format: PID\nnum_channels\nnum_partials
            let num_channels = lines[1].parse::<usize>().ok()?;
            let num_partials = lines[2].parse::<usize>().ok()?;
            Some((num_channels, num_partials))
        } else {
            None
        }
    }
    
    pub fn new() -> Result<Self> {
        // Initialize stepper_gui (optional - only if Arduino is configured)
        let stepper_gui = Self::init_stepper_gui().ok();
        
        // Initialize operations_gui
        let operations_gui = operations_gui_mod::OperationsGUI::new().ok();
        
        // Initialize audmon_gui - try to create MyApp instance
        let audmon_gui = match Self::init_audmon_gui() {
            Ok(app) => Some(app),
            Err(e) => {
                eprintln!("Failed to initialize audmon GUI: {:#}", e);
                eprintln!("This may be due to missing configuration or audio device issues.");
                eprintln!("Check that audio_monitor.yaml exists and database environment variables are set.");
                None
            }
        };
        
        Ok(Self {
            stepper_gui,
            operations_gui,
            audmon_gui,
        })
    }
    
    fn init_stepper_gui() -> Result<stepper_gui_mod::StepperGUI> {
        use clap::Parser;
        
        #[derive(Parser)]
        struct Args {
            #[arg(long)]
            debug: bool,
        }
        
        let args = Args::parse();
        let mut debug_file: Option<File> = None;
        if args.debug {
            if let Ok(file) = File::create("/home/gregory/Documents/string_driver/rust_driver/run_output.log") {
                debug_file = Some(file);
            }
        }

        let hostname = gethostname().to_string_lossy().to_string();
        let settings = config_loader::load_arduino_settings(&hostname)?;
        
        // Extract all values from settings before moving/borrowing
        let mainboard_tuner_count = config_loader::mainboard_tuner_indices(&settings).len();
        let tuner_num_for_gui = if settings.ard_t_num_steppers.is_some() {
            settings.ard_t_num_steppers
        } else if mainboard_tuner_count > 0 {
            Some(mainboard_tuner_count)
        } else {
            None
        };
        
        // Only initialize if Arduino is configured
        let port = settings.port.ok_or_else(|| anyhow::anyhow!("No Arduino port configured"))?;
        let num_steppers = settings.num_steppers.ok_or_else(|| anyhow::anyhow!("No Arduino steppers configured"))?;
        
        // Extract remaining values
        let string_num = settings.string_num;
        let x_step_index = settings.x_step_index;
        let z_first_index = settings.z_first_index;
        let tuner_first_index = settings.tuner_first_index;
        let ard_t_port = settings.ard_t_port.clone();
        let firmware = settings.firmware;
        let x_max_pos = settings.x_max_pos;
        
        let ops_settings = config_loader::load_operations_settings(&hostname)
            .unwrap_or_else(|_| config_loader::OperationsSettings {
                z_up_step: Some(2),
                z_down_step: Some(-2),
                bump_check_enable: true,
                tune_rest: Some(10.0),
                x_rest: Some(10.0),
                z_rest: Some(5.0),
                lap_rest: Some(4.0),
                adjustment_level: Some(4),
                retry_threshold: Some(50),
                delta_threshold: Some(50),
                z_variance_threshold: Some(50),
                x_start: Some(100),
                x_finish: Some(100),
                x_step: Some(10),
            });
        let z_up_step = ops_settings.z_up_step.unwrap_or(2);
        let z_down_step = ops_settings.z_down_step.unwrap_or(-2);
        let x_step = ops_settings.x_step.unwrap_or(10);

        // Use firmware directly - both modules use the same enum from config_loader.rs
        // We'll use unsafe transmute since the compiler sees them as different types
        // but they're actually the same enum from the same source file
        use std::mem;
        
        let mut stepper = stepper_gui_mod::StepperGUI::new(
            port,
            num_steppers,
            string_num,
            x_step_index,
            z_first_index,
            tuner_first_index,
            ard_t_port,
            tuner_num_for_gui,
            args.debug,
            debug_file,
            z_up_step,
            z_down_step,
            // Transmute firmware enum - both are the same enum from config_loader.rs
            unsafe { mem::transmute(firmware) },
            x_max_pos,
            x_step,
        );
        
        // Auto-connect on startup
        stepper.connect();
        
        // Connect to tuner board if configured
        if settings.tuner_first_index.is_some() {
            stepper.connect_tuner();
        }
        
        Ok(stepper)
    }
    
    /// Initialize audmon GUI (MyApp instance)
    fn init_audmon_gui() -> Result<MyApp> {
        // Use the full init_audmon() function which starts all audio processing threads
        // This ensures database logging, GStreamer, ChucK, and all recovery mechanisms are active
        let (my_app, _shutdown_flag) = audio_monitor::init::init_audmon()
            .map_err(|e| anyhow::anyhow!("Failed to initialize audmon: {}", e))?;
        
        Ok(my_app)
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
                egui::ScrollArea::vertical().show(ui, |ui| {
                    if let Some(ref mut stepper) = self.stepper_gui {
                        stepper.render_ui(ui, ctx);
                    } else {
                        ui.label("Stepper Control");
                ui.separator();
                        ui.label("Arduino not configured or initialization failed");
                }
                });
            });
        
        // Right panel: Operations Control
        egui::SidePanel::right("operations_panel")
            .resizable(true)
            .default_width(600.0)
            .min_width(400.0)
            .max_width(800.0)
            .show(ctx, |ui| {
                if let Some(ref mut ops) = self.operations_gui {
                    // Handle pre-rendering logic that OperationsGUI::update() normally does
                    if ops.exit_flag.load(std::sync::atomic::Ordering::Relaxed) 
                        && !ops.operation_running.load(std::sync::atomic::Ordering::Relaxed) {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        return;
                    }
                    ctx.request_repaint_after(Duration::from_millis(16));
                    ops.poll_operation_result();
                    let partials = get_results::read_partials_from_slot(&ops.partials_slot);
                    ops.operations.read().unwrap().update_audio_analysis_with_partials(partials);
                    ops.reconcile_voice_count_cap();
                    
                    ops.render_ui(ui, ctx);
                } else {
                    ui.label("Operations Control");
                ui.separator();
                    ui.label("Initialization failed");
                }
            });
        
        // Center panel: Audio Monitor GUI (full audmon interface)
        egui::CentralPanel::default().show(ctx, |ui| {
            if let Some(ref mut audmon_gui) = self.audmon_gui {
                // Update partials from shared memory before rendering
                let shm_dir = if cfg!(target_os = "linux") {
                    "/dev/shm"
                } else if cfg!(target_os = "macos") {
                    "/tmp"
                } else {
                    "/tmp"
                };
                let control_path = format!("{}/audio_control", shm_dir);
                
                // Read partials from shared memory and update MyApp
                if let Some((num_channels, num_partials)) = Self::read_control_file_direct(&control_path) {
                    if let Some(partials) = operations::Operations::read_partials_from_shared_memory(
                        num_channels,
                        num_partials
                    ) {
                        audmon_gui.update_from_partials(partials);
                    }
                }
                
                // Render the full audmon GUI content (we're already in a CentralPanel)
                audmon_gui.render_ui_in_panel(ui, ctx);
            } else {
                // Fallback: show status if audmon_gui not initialized
                ui.heading("Audio Monitor (audmon)");
                ui.separator();
                ui.label("❌ audmon GUI initialization failed");
                ui.label("");
                ui.label("Common causes:");
                ui.label("• Missing audio_monitor.yaml configuration");
                ui.label("• Missing database environment variables (DB_PASSWORD, etc.)");
                ui.label("• Audio device not available");
                ui.label("• PortAudio initialization failed");
                ui.label("");
                ui.label("Check the console/terminal for detailed error messages.");
            }
        });
        
        // Render crosstalk trainer window if needed (must be outside the CentralPanel)
        if let Some(ref mut audmon_gui) = self.audmon_gui {
            audmon_gui.render_crosstalk_trainer(ctx);
        }
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
