/// Launcher for String Driver application
/// 
/// Builds release binaries if needed, then launches:
/// - persist.sh
/// - audio_streaming (main GUI)
/// - stepper_gui
/// - operations_gui
/// 
/// Run with: cargo run --bin launcher --release

use std::process::Command;
use std::env;

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
    let binaries = vec![
        "audio_streaming",  // Needed by persist.sh
        "stepper_gui",
        "operations_gui",
    ];
    
    // Check and build release binaries if needed
    println!("Checking release binaries...");
    let mut needs_build = false;
    for bin_name in &binaries {
        let bin_path = release_dir.join(bin_name);
        if !bin_path.exists() {
            println!("  ⚠ {} not found at {}", bin_name, bin_path.display());
            needs_build = true;
        } else {
            println!("  ✓ {} found", bin_name);
        }
    }
    
    if needs_build {
        println!("\nBuilding release binaries...");
        let build_status = Command::new("cargo")
            .args(&["build", "--release", "--bin", "audio_streaming", "--bin", "stepper_gui", "--bin", "operations_gui"])
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
    } else {
        println!("\nAll release binaries exist, skipping build");
    }
    
    // Launch persist.sh (which will launch and monitor audio_streaming)
    println!("\nLaunching persist.sh...");
    let persist_script = project_root.join("persist.sh");
    if persist_script.exists() {
        match Command::new("bash")
            .arg(&persist_script)
            .current_dir(&project_root)
            .spawn() {
            Ok(_) => {
                println!("✓ persist.sh launched (will launch audio_streaming)");
            }
            Err(e) => {
                eprintln!("✗ Failed to launch persist.sh: {}", e);
                std::process::exit(1);
            }
        }
    } else {
        eprintln!("✗ persist.sh not found at {}", persist_script.display());
        std::process::exit(1);
    }
    
    // Small delay before launching other GUIs
    std::thread::sleep(std::time::Duration::from_millis(1000));
    
    // Launch other GUIs
    println!("\nLaunching additional GUIs...");
    
    // Launch stepper_gui
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

