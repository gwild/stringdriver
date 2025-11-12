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
use std::fs::OpenOptions;
use std::path::Path;
use std::io::Write;
use memmap2::Mmap;

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
    if !wait_for_shared_memory() {
        eprintln!("✗ Timeout waiting for shared memory to have results");
        eprintln!("  audio_monitor may not be running correctly");
        eprintln!("  Shared memory path: {}", shm_path);
        if Path::new(&shm_path).exists() {
            eprintln!("  File exists but may not have valid data yet");
        } else {
            eprintln!("  File does not exist");
        }
        std::process::exit(1);
    }
    println!("✓ Shared memory verified - audio_monitor is running");
    
    // Always build release binaries to ensure latest code is used
    println!("\nBuilding release binaries...");
    let build_status = Command::new("cargo")
        .args(&["build", "--release", "--bin", "stepper_gui", "--bin", "operations_gui"])
        .current_dir(&project_root)
        .status();
    
    match build_status {
        Ok(status) if status.success() => {
            println!("✓ Release binaries built successfully");
        }
        Ok(status) => {
            eprintln!("✗ Build failed with exit code: {:?}", status.code());
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
    match Command::new(&stepper_gui)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn() {
        Ok(_) => {
            println!("✓ stepper_gui launched");
        }
        Err(e) => {
            eprintln!("✗ Failed to launch stepper_gui: {}", e);
            std::process::exit(1);
        }
    }
    
    // Launch operations_gui
    println!("\nLaunching operations_gui...");
    let operations_gui = release_dir.join("operations_gui");
    match Command::new(&operations_gui)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn() {
        Ok(_) => {
            println!("✓ operations_gui launched");
        }
        Err(e) => {
            eprintln!("✗ Failed to launch operations_gui: {}", e);
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

/// Check if shared memory has valid data (any non-zero bytes indicating audmon is writing)
fn check_shared_memory_has_data() -> bool {
    let shm_path = get_shared_memory_path();
    
    // Check if file exists
    if !Path::new(&shm_path).exists() {
        return false;
    }
    
    // Try to open and read the shared memory file
    let file = match OpenOptions::new().read(true).open(&shm_path) {
        Ok(f) => f,
        Err(_) => return false,
    };
    
    let mmap = match unsafe { Mmap::map(&file) } {
        Ok(m) => m,
        Err(_) => return false,
    };
    
    // Need at least 8 bytes (one partial: f32 freq + f32 amp)
    if mmap.len() < 8 {
        return false;
    }
    
    // Check for any non-zero data in the file (audmon writes partials, so if file is all zeros, it's not ready)
    // Scan through the file looking for any non-zero bytes
    // We'll check in chunks to avoid scanning the entire 4MB file
    let check_size = mmap.len().min(8192); // Check first 8KB which should contain at least some partials
    for chunk in mmap[..check_size].chunks(8) {
        if chunk.len() >= 8 {
            // Check if this 8-byte chunk (one partial) has non-zero data
            let has_data = chunk.iter().any(|&b| b != 0);
            if has_data {
                // Verify it's a valid partial by checking if frequency is reasonable (> 0 and < 20000 Hz)
                let freq_bytes = [chunk[0], chunk[1], chunk[2], chunk[3]];
                let freq = f32::from_ne_bytes(freq_bytes);
                if freq > 0.0 && freq < 20000.0 {
                    return true;
                }
            }
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

