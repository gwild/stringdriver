/// Launcher for String Driver application
/// 
/// Builds release binaries if needed, then launches:
/// - audio_monitor (audmon) via audmon.sh
/// - Waits for shared memory to have results
/// - stepper_gui
/// - operations_gui
/// 
/// Run with: cargo run --bin launcher --release

use std::process::Command;
use std::env;
use std::path::Path;
use std::io::Write;
use gethostname::gethostname;
use serde_yaml;

fn main() {
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("String Driver Launcher");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");
    
    // Get project root directory
    let project_root = match env::var("CARGO_MANIFEST_DIR") {
        Ok(dir) => std::path::PathBuf::from(dir),
        Err(_) => {
            eprintln!("ERROR: Could not determine project root");
            std::process::exit(1);
        }
    };
    
    let release_dir = project_root.join("target/release");
    
    // Launch audmon via audmon.sh script (maintains persistence for JACK audio)
    println!("Launching audio_monitor (audmon) via audmon.sh...");
    let audmon_path = project_root.parent()
        .map(|p| p.join("audmon"))
        .unwrap_or_else(|| std::path::PathBuf::from("../audmon"));
    
    // Clone audmon if local clone doesn't exist
    if !audmon_path.exists() {
        println!("Local audmon clone not found, cloning repository...");
        let parent_dir = project_root.parent().unwrap_or(&project_root);
        let clone_status = Command::new("git")
            .args(&["clone", "git@github.com:gwild/audmon.git", "audmon"])
            .current_dir(parent_dir)
            .status();
        
        match clone_status {
            Ok(status) if status.success() => {
                println!("✓ audmon cloned successfully");
            }
            Ok(status) => {
                eprintln!("✗ git clone failed with exit code: {:?}", status.code());
                std::process::exit(1);
            }
            Err(e) => {
                eprintln!("✗ Failed to run git clone: {}", e);
                std::process::exit(1);
            }
        }
    }
    
    let audmon_script = audmon_path.join("audmon.sh");
    if !audmon_script.exists() {
        eprintln!("✗ audmon.sh not found at {}", audmon_script.display());
        std::process::exit(1);
    }
    
    match Command::new("bash")
        .arg(&audmon_script)
        .current_dir(&audmon_path)
        .spawn() {
        Ok(_) => {
            println!("✓ audio_monitor launched via audmon.sh");
        }
        Err(e) => {
            eprintln!("✗ Failed to launch audmon.sh: {}", e);
            std::process::exit(1);
        }
    }
    
    // Wait for audmon to start writing to shared memory
    println!("\nWaiting for audio_monitor to initialize and write to shared memory...");
    let shm_path = get_shared_memory_path();
    println!("  Checking shared memory at: {}", shm_path);
    let shm_ready = wait_for_shared_memory();
    if !shm_ready {
        eprintln!("⚠ Warning: Timeout waiting for shared memory to have results");
        eprintln!("  audio_monitor may not be running correctly");
        eprintln!("  Shared memory path: {}", shm_path);
        if Path::new(&shm_path).exists() {
            eprintln!("  File exists but may not have valid data yet");
        } else {
            eprintln!("  File does not exist");
        }
        eprintln!("  Continuing anyway to launch stepper_gui and operations_gui...");
    } else {
        println!("✓ Shared memory verified - audio_monitor is running");
    }
    
    // Check if GPIO is enabled for this host from YAML
    let gpio_enabled = check_gpio_enabled(&project_root);
    println!("\nGPIO enabled for this host: {}", gpio_enabled);
    
    // Always build release binaries to ensure latest code is used
    println!("\nBuilding release binaries...");
    let mut build_args = vec!["build", "--release"];
    if gpio_enabled {
        build_args.push("--features");
        build_args.push("gpiod");
    }
    build_args.push("--bin");
    build_args.push("stepper_gui");
    build_args.push("--bin");
    build_args.push("operations_gui");
    let build_output = Command::new("cargo")
        .args(&build_args)
        .current_dir(&project_root)
        .output();
    
    match build_output {
        Ok(output) if output.status.success() => {
            println!("✓ Release binaries built successfully");
        }
        Ok(output) => {
            eprintln!("✗ Build failed with exit code: {:?}", output.status.code());
            eprintln!("Build stderr:");
            eprintln!("{}", String::from_utf8_lossy(&output.stderr));
            eprintln!("Build stdout:");
            eprintln!("{}", String::from_utf8_lossy(&output.stdout));
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("✗ Failed to run cargo build: {}", e);
            std::process::exit(1);
        }
    }
    
    // Launch stepper_gui
    println!("\nLaunching stepper_gui...");
    let stepper_gui = release_dir.join("stepper_gui");
    
    // Check if binary exists
    if !stepper_gui.exists() {
        eprintln!("✗ stepper_gui binary not found at: {}", stepper_gui.display());
        std::process::exit(1);
    }
    
    match Command::new(&stepper_gui)
        .spawn() {
        Ok(child) => {
            println!("✓ stepper_gui launched (PID: {})", child.id());
        }
        Err(e) => {
            eprintln!("✗ Failed to launch stepper_gui: {}", e);
            std::process::exit(1);
        }
    }
    
    // Launch operations_gui
    println!("\nLaunching operations_gui...");
    let operations_gui = release_dir.join("operations_gui");
    
    // Check if binary exists
    if !operations_gui.exists() {
        eprintln!("✗ operations_gui binary not found at: {}", operations_gui.display());
        eprintln!("  Expected path: {}", operations_gui.display());
        eprintln!("  Release directory exists: {}", release_dir.exists());
        if release_dir.exists() {
            eprintln!("  Files in release directory:");
            if let Ok(entries) = std::fs::read_dir(&release_dir) {
                for entry in entries.flatten() {
                    if let Ok(name) = entry.file_name().into_string() {
                        eprintln!("    - {}", name);
                    }
                }
            }
        }
        std::process::exit(1);
    }
    
    match Command::new(&operations_gui)
        .spawn() {
        Ok(mut child) => {
            println!("✓ operations_gui launched (PID: {})", child.id());
            // Give it a moment to start and check if it's still running
            std::thread::sleep(std::time::Duration::from_millis(500));
            match child.try_wait() {
                Ok(Some(status)) => {
                    eprintln!("✗ operations_gui exited immediately with status: {:?}", status);
                    eprintln!("  This usually indicates a startup error - check stderr output above");
                    std::process::exit(1);
                }
                Ok(None) => {
                    println!("  operations_gui is still running");
                }
                Err(e) => {
                    eprintln!("  Warning: Could not check operations_gui status: {}", e);
                }
            }
        }
        Err(e) => {
            eprintln!("✗ Failed to launch operations_gui: {}", e);
            eprintln!("  Binary path: {}", operations_gui.display());
            eprintln!("  Error details: {:?}", e);
            std::process::exit(1);
        }
    }
    
    println!("\n━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("All applications launched!");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("\nLauncher exiting (applications will continue running)");
}

/// Get shared memory path for partials data
fn get_shared_memory_path() -> String {
    let shm_dir = if cfg!(target_os = "linux") {
        "/dev/shm"
    } else if cfg!(target_os = "macos") {
        "/tmp"
    } else {
        "/tmp"
    };
    format!("{}/audio_peaks", shm_dir)
}

/// Check if shared memory file exists and has been created by audio_monitor
/// audio_monitor creates the file when it starts, so if it exists with reasonable size, it's ready
fn check_shared_memory_has_data() -> bool {
    let shm_path = get_shared_memory_path();
    
    // Check if file exists
    if !Path::new(&shm_path).exists() {
        return false;
    }
    
    // Check file size - audio_monitor creates a file of a specific size (typically 4MB for partials)
    // If file exists and has reasonable size (> 0 bytes), audio_monitor is running
    if let Ok(metadata) = std::fs::metadata(&shm_path) {
        let size = metadata.len();
        // File exists and has some size - audio_monitor is running
        // Don't require valid audio data since there might not be audio input yet
        if size > 0 {
            return true;
        }
    }
    
    false
}

/// Wait for shared memory to have results (event-driven polling)
/// Returns true if shared memory has data, false if timeout
fn wait_for_shared_memory() -> bool {
    const MAX_ATTEMPTS: u32 = 60; // 60 attempts
    const POLL_INTERVAL_MS: u64 = 500; // Check every 500ms
    
    for attempt in 1..=MAX_ATTEMPTS {
        if check_shared_memory_has_data() {
            return true;
        }
        
        if attempt < MAX_ATTEMPTS {
            std::thread::sleep(std::time::Duration::from_millis(POLL_INTERVAL_MS));
            if attempt % 10 == 0 {
                print!(".");
                std::io::stdout().flush().ok();
            }
        }
    }
    
    false
}

/// Check if GPIO is enabled for the current hostname from YAML config
fn check_gpio_enabled(project_root: &std::path::Path) -> bool {
    let yaml_path = project_root.join("string_driver.yaml");
    let file = match std::fs::File::open(&yaml_path) {
        Ok(f) => f,
        Err(_) => return false,
    };
    
    let yaml: serde_yaml::Value = match serde_yaml::from_reader(file) {
        Ok(y) => y,
        Err(_) => return false,
    };
    
    let hostname = gethostname().to_string_lossy().to_string();
    
    // Search across known OS sections to find a host block matching hostname
    for os_key in ["RaspberryPi", "Ubuntu", "macOS"].iter() {
        if let Some(os_map) = yaml.get(*os_key).and_then(|v| v.as_mapping()) {
            for (k, v) in os_map.iter() {
                if k.as_str() == Some(&hostname) {
                    if let Some(host_block) = v.as_mapping() {
                        // Check GPIO_ENABLED
                        if let Some(gpio_enabled) = host_block.get(&serde_yaml::Value::from("GPIO_ENABLED")) {
                            if let Some(enabled) = gpio_enabled.as_bool() {
                                return enabled;
                            }
                        }
                    }
                }
            }
        }
    }
    
    false
}

