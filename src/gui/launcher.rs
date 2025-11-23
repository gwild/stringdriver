/// Launcher for String Driver application
/// 
/// Two modes:
/// 1. Master GUI mode (default): Launches master_gui via master_gui.sh
///    - master_gui embeds audmon, stepper_gui, and operations_gui
///    - Single unified interface
/// 
/// 2. Separate mode (--separate flag): Launches components separately
///    - audio_monitor (audmon) via audmon.sh
///    - Waits for shared memory to have results
///    - stepper_gui
///    - operations_gui
/// 
/// Run with: 
///   cargo run --bin launcher --release              # Master GUI mode
///   cargo run --bin launcher --release -- --separate  # Separate mode

use std::process::{Command, Stdio};
use std::env;
use std::path::Path;
use std::io::Write;
use gethostname::gethostname;
use serde_yaml;

fn main() {
    let args: Vec<String> = env::args().collect();
    let separate_mode = args.iter().any(|a| a == "--separate");
    
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("String Driver Launcher");
    if separate_mode {
        println!("Mode: Separate components");
    } else {
        println!("Mode: Master GUI (unified)");
    }
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");
    
    if separate_mode {
        launch_separate_mode()
    } else {
        launch_master_gui_mode()
    }
}

fn launch_master_gui_mode() {
    // Get project root directory
    let project_root = match env::var("CARGO_MANIFEST_DIR") {
        Ok(dir) => std::path::PathBuf::from(dir),
        Err(_) => {
            eprintln!("ERROR: Could not determine project root");
            std::process::exit(1);
        }
    };
    
    let release_dir = project_root.join("target/release");
    
    // Check if GPIO is enabled for this host from YAML
    let gpio_enabled = check_gpio_enabled(&project_root);
    println!("GPIO enabled for this host: {}", gpio_enabled);
    
    // Check if master_gui binary needs rebuilding
    let master_gui_binary = release_dir.join("master_gui");
    let needs_build = check_binary_needs_build(&project_root, &master_gui_binary);
    
    if needs_build {
        println!("\nBuilding master_gui release binary...");
        println!("  (Cargo build output will appear below)\n");
        std::io::stdout().flush().ok();
        
        let mut build_args = vec!["build", "--release", "--bin", "master_gui"];
        if gpio_enabled {
            build_args.push("--features");
            build_args.push("gpiod");
        }
        
        let build_status = Command::new("cargo")
            .args(&build_args)
            .current_dir(&project_root)
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status();
        
        match build_status {
            Ok(status) if status.success() => {
                println!("\n✓ master_gui release binary built successfully");
            }
            Ok(status) => {
                eprintln!("\n✗ Build failed with exit code: {:?}", status.code());
                std::process::exit(1);
            }
            Err(e) => {
                eprintln!("\n✗ Failed to run cargo build: {}", e);
                std::process::exit(1);
            }
        }
    } else {
        println!("\n✓ master_gui release binary is up-to-date");
    }
    
    // Launch master_gui via master_gui.sh script (maintains persistence)
    println!("\nLaunching master_gui via master_gui.sh...");
    let master_gui_script = project_root.join("master_gui.sh");
    if !master_gui_script.exists() {
        eprintln!("✗ master_gui.sh not found at {}", master_gui_script.display());
        std::process::exit(1);
    }
    
    match Command::new("bash")
        .arg(&master_gui_script)
        .current_dir(&project_root)
        .spawn() {
        Ok(_) => {
            println!("✓ master_gui launched via master_gui.sh");
        }
        Err(e) => {
            eprintln!("✗ Failed to launch master_gui.sh: {}", e);
            std::process::exit(1);
        }
    }
    
    // Wait for master_gui to be ready (check status file)
    println!("\nWaiting for master_gui to initialize...");
    let status_file = project_root.join(".master_gui_status");
    let ready = wait_for_master_gui_ready(&status_file);
    if !ready {
        eprintln!("⚠ Warning: Timeout waiting for master_gui to be ready");
        eprintln!("  master_gui may still be starting up");
        eprintln!("  Status file: {}", status_file.display());
    } else {
        println!("✓ master_gui is ready");
    }
    
    println!("\n━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("Master GUI launched!");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("\nLauncher exiting (master_gui will continue running)");
}

fn launch_separate_mode() {
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
    
    // Check if audmon release binary exists and is up-to-date
    let audmon_release_dir = audmon_path.join("target/release");
    let audmon_binary = audmon_release_dir.join("audio_monitor");
    let needs_build = check_binary_needs_build(&audmon_path, &audmon_binary);
    
    if needs_build {
        println!("Building audmon release binary...");
        let build_status = Command::new("cargo")
            .args(&["build", "--release", "--bin", "audio_monitor"])
            .current_dir(&audmon_path)
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status();
        
        match build_status {
            Ok(status) if status.success() => {
                println!("✓ audmon release binary built successfully");
            }
            Ok(status) => {
                eprintln!("✗ audmon build failed with exit code: {:?}", status.code());
                std::process::exit(1);
            }
            Err(e) => {
                eprintln!("✗ Failed to run cargo build for audmon: {}", e);
                std::process::exit(1);
            }
        }
    } else {
        println!("✓ audmon release binary is up-to-date");
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
    
    // Check if binaries need rebuilding
    let stepper_gui_binary = release_dir.join("stepper_gui");
    let operations_gui_binary = release_dir.join("operations_gui");
    let stepper_needs_build = check_binary_needs_build(&project_root, &stepper_gui_binary);
    let operations_needs_build = check_binary_needs_build(&project_root, &operations_gui_binary);
    
    if stepper_needs_build || operations_needs_build {
        println!("\nBuilding release binaries...");
        if stepper_needs_build {
            println!("  stepper_gui needs rebuild");
        }
        if operations_needs_build {
            println!("  operations_gui needs rebuild");
        }
        println!("  (Cargo build output will appear below)\n");
        std::io::stdout().flush().ok();
        
        let mut build_args = vec!["build", "--release"];
        if gpio_enabled {
            build_args.push("--features");
            build_args.push("gpiod");
        }
        build_args.push("--bin");
        build_args.push("stepper_gui");
        build_args.push("--bin");
        build_args.push("operations_gui");
        
        let build_status = Command::new("cargo")
            .args(&build_args)
            .current_dir(&project_root)
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status();
        
        match build_status {
            Ok(status) if status.success() => {
                println!("\n✓ Release binaries built successfully");
            }
            Ok(status) => {
                eprintln!("\n✗ Build failed with exit code: {:?}", status.code());
                std::process::exit(1);
            }
            Err(e) => {
                eprintln!("\n✗ Failed to run cargo build: {}", e);
                std::process::exit(1);
            }
        }
    } else {
        println!("\n✓ Release binaries are up-to-date");
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
    
    // Wait for stepper_gui socket to be ready before launching operations_gui
    println!("\nWaiting for stepper_gui socket to be ready...");
    let socket_ready = wait_for_stepper_socket(&project_root);
    if !socket_ready {
        eprintln!("⚠ Warning: Timeout waiting for stepper_gui socket");
        eprintln!("  stepper_gui may not be running correctly");
        eprintln!("  Continuing anyway to launch operations_gui...");
    } else {
        println!("✓ stepper_gui socket verified - ready for operations_gui");
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

/// Wait for master_gui status file to show "ready" (event-driven polling)
/// Returns true if status is "ready", false if timeout
fn wait_for_master_gui_ready(status_file: &std::path::Path) -> bool {
    const MAX_ATTEMPTS: u32 = 60; // 60 attempts
    const POLL_INTERVAL_MS: u64 = 500; // Check every 500ms
    
    for attempt in 1..=MAX_ATTEMPTS {
        if let Ok(content) = std::fs::read_to_string(status_file) {
            let status = content.trim();
            if status == "ready" {
                return true;
            }
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

/// Get socket path for stepper_gui based on Arduino port
fn get_stepper_socket_path(project_root: &std::path::Path) -> Option<String> {
    let yaml_path = project_root.join("string_driver.yaml");
    let file = match std::fs::File::open(&yaml_path) {
        Ok(f) => f,
        Err(_) => return None,
    };
    
    let yaml: serde_yaml::Value = match serde_yaml::from_reader(file) {
        Ok(y) => y,
        Err(_) => return None,
    };
    
    let hostname = gethostname().to_string_lossy().to_string();
    
    // Search across known OS sections to find a host block matching hostname
    for os_key in ["RaspberryPi", "Ubuntu", "macOS"].iter() {
        if let Some(os_map) = yaml.get(*os_key).and_then(|v| v.as_mapping()) {
            for (k, v) in os_map.iter() {
                if k.as_str() == Some(&hostname) {
                    if let Some(host_block) = v.as_mapping() {
                        // Get ARD_PORT
                        if let Some(ard_port) = host_block.get(&serde_yaml::Value::from("ARD_PORT")) {
                            if let Some(port_str) = ard_port.as_str() {
                                // Generate socket path same way as stepper_gui.rs
                                let port_id = port_str.replace("/", "_").replace("\\", "_");
                                return Some(format!("/tmp/stepper_gui_{}.sock", port_id));
                            }
                        }
                    }
                }
            }
        }
    }
    
    None
}

/// Wait for stepper_gui socket to exist (event-driven polling)
/// Returns true if socket exists, false if timeout
fn wait_for_stepper_socket(project_root: &std::path::Path) -> bool {
    let socket_path = match get_stepper_socket_path(project_root) {
        Some(path) => path,
        None => {
            eprintln!("  Could not determine socket path from config");
            return false;
        }
    };
    
    println!("  Checking socket at: {}", socket_path);
    
    const MAX_ATTEMPTS: u32 = 30; // 30 attempts
    const POLL_INTERVAL_MS: u64 = 200; // Check every 200ms
    
    for attempt in 1..=MAX_ATTEMPTS {
        if Path::new(&socket_path).exists() {
            return true;
        }
        
        if attempt < MAX_ATTEMPTS {
            std::thread::sleep(std::time::Duration::from_millis(POLL_INTERVAL_MS));
            if attempt % 5 == 0 {
                print!(".");
                std::io::stdout().flush().ok();
            }
        }
    }
    
    false
}

/// Check if a binary needs a fresh release build
/// Returns true if binary doesn't exist or source files are newer than binary
fn check_binary_needs_build(project_root: &std::path::Path, binary_path: &std::path::Path) -> bool {
    // If binary doesn't exist, needs build
    if !binary_path.exists() {
        return true;
    }
    
    // Get binary modification time
    let binary_mtime = match std::fs::metadata(binary_path) {
        Ok(meta) => meta.modified().ok(),
        Err(_) => return true, // If we can't read binary, rebuild
    };
    
    let binary_mtime = match binary_mtime {
        Some(t) => t,
        None => return true, // Can't get mtime, rebuild
    };
    
    // Check Cargo files
    let cargo_files = ["Cargo.toml", "Cargo.lock", "build.rs"];
    for file_name in cargo_files.iter() {
        let file_path = project_root.join(file_name);
        if file_path.exists() {
            if let Ok(meta) = std::fs::metadata(&file_path) {
                if let Ok(file_mtime) = meta.modified() {
                    if file_mtime > binary_mtime {
                        return true;
                    }
                }
            }
        }
    }
    
    // Check Rust source files
    let src_dir = project_root.join("src");
    if src_dir.exists() {
        if let Ok(entries) = std::fs::read_dir(&src_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() {
                    let ext = path.extension().and_then(|s| s.to_str());
                    if ext == Some("rs") || ext == Some("toml") {
                        if let Ok(meta) = std::fs::metadata(&path) {
                            if let Ok(file_mtime) = meta.modified() {
                                if file_mtime > binary_mtime {
                                    return true;
                                }
                            }
                        }
                    }
                } else if path.is_dir() {
                    // Recursively check subdirectories
                    if check_dir_newer_than(&path, binary_mtime) {
                        return true;
                    }
                }
            }
        }
    }
    
    false
}

/// Recursively check if any files in directory are newer than given time
fn check_dir_newer_than(dir_path: &std::path::Path, threshold: std::time::SystemTime) -> bool {
    if let Ok(entries) = std::fs::read_dir(dir_path) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() {
                let ext = path.extension().and_then(|s| s.to_str());
                if ext == Some("rs") || ext == Some("toml") {
                    if let Ok(meta) = std::fs::metadata(&path) {
                        if let Ok(file_mtime) = meta.modified() {
                            if file_mtime > threshold {
                                return true;
                            }
                        }
                    }
                }
            } else if path.is_dir() {
                if check_dir_newer_than(&path, threshold) {
                    return true;
                }
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

