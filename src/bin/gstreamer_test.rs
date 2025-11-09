/// Standalone GStreamer test binary
/// 
/// Run with: cargo run --bin gstreamer_test
/// 
/// Tests GStreamer pipeline building and configuration loading
/// without requiring actual GStreamer installation.

#[path = "../config_loader.rs"]
mod config_loader;
#[path = "../gstreamer.rs"]
mod gstreamer;

use anyhow::Result;
use gethostname::gethostname;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use std::thread;

fn test_pipeline_building() -> Result<()> {
    println!("=== Testing Pipeline Building ===");
    
    // Load config from config_loader (single source of truth)
    let hostname = gethostname().to_string_lossy().to_string();
    let config = match config_loader::gstreamer_env_for(&hostname) {
        Ok(envs) => envs,
        Err(e) => {
            println!("⚠ Could not load GStreamer config: {}", e);
            println!("  Skipping pipeline building tests (config required)");
            return Ok(());
        }
    };
    
    // Test with Icecast
    let (pipeline, _) = gstreamer::build_pipeline(&config, true);
    println!("✓ Pipeline with Icecast:");
    println!("  {}", pipeline);
    assert!(pipeline.contains("shout2send"), "Pipeline should contain shout2send sink");
    if let Some(host) = config.get("ICECAST_HOST") {
        assert!(pipeline.contains(&format!("ip={}", host)), "Pipeline should contain Icecast host");
    }
    if let Some(mount) = config.get("ICECAST_MOUNT") {
        let mount_normalized = if mount.starts_with('/') { mount.clone() } else { format!("/{}", mount) };
        assert!(pipeline.contains(&format!("mount={}", mount_normalized)), "Pipeline should contain mount point");
    }
    assert!(pipeline.contains("queue"), "Pipeline should contain queue");
    
    // Test without Icecast
    let (pipeline_no_ice, _) = gstreamer::build_pipeline(&config, false);
    println!("✓ Pipeline without Icecast:");
    println!("  {}", pipeline_no_ice);
    assert!(pipeline_no_ice.contains("fakesink"), "Pipeline without Icecast should use fakesink");
    assert!(!pipeline_no_ice.contains("shout2send"), "Pipeline without Icecast should not contain shout2send");
    
    // Test with second queue (if enabled in config)
    if config.get("GST_SECOND_QUEUE_ENABLE").map(|v| v == "true").unwrap_or(false) {
        let (pipeline_2q, _) = gstreamer::build_pipeline(&config, true);
        println!("✓ Pipeline with second queue:");
        println!("  {}", pipeline_2q);
        // Count queue occurrences (should be at least 2)
        let queue_count = pipeline_2q.matches("queue").count();
        assert!(queue_count >= 2, "Expected at least 2 queues, found {}", queue_count);
    }
    
    println!("✓ All pipeline building tests passed!\n");
    Ok(())
}

fn test_config_loading() -> Result<()> {
    println!("=== Testing Configuration Loading ===");
    
    let hostname = gethostname().to_string_lossy().to_string();
    println!("Hostname: {}", hostname);
    
    match config_loader::gstreamer_env_for(&hostname) {
        Ok(envs) => {
            println!("✓ GStreamer configuration loaded for {}", hostname);
            println!("  Configuration keys:");
            for (key, value) in &envs {
                if key.contains("PASSWORD") {
                    println!("    {} = [REDACTED]", key);
                } else {
                    println!("    {} = {}", key, value);
                }
            }
            
            // Validate required keys
            let required = ["GSTREAMER_AUDIO_SRC", "GSTREAMER_CONVERT", "GSTREAMER_ENCODER"];
            let mut missing = Vec::new();
            for key in &required {
                if !envs.contains_key(*key) {
                    missing.push(*key);
                }
            }
            
            if !missing.is_empty() {
                println!("⚠ Missing required keys: {:?}", missing);
            } else {
                println!("✓ All required keys present");
            }
        }
        Err(e) => {
            println!("⚠ No GStreamer configuration found: {}", e);
            println!("  (This is OK if hostname '{}' is not in gstreamer.yaml)", hostname);
        }
    }
    
    println!();
    Ok(())
}

fn test_icecast_connectivity() -> Result<()> {
    println!("=== Testing Icecast Connectivity ===");
    
    // Test with invalid host (should fail quickly)
    println!("Testing invalid host (should fail)...");
    let result = std::net::TcpStream::connect_timeout(
        &"127.0.0.1:1".parse().unwrap(),
        Duration::from_secs(1)
    );
    match result {
        Ok(_) => println!("  Unexpected: connection succeeded"),
        Err(e) => println!("  ✓ Correctly failed: {}", e),
    }
    
    // Try to get actual Icecast config from YAML
    let hostname = gethostname().to_string_lossy().to_string();
    match config_loader::gstreamer_env_for(&hostname) {
        Ok(envs) => {
            if let (Some(host), Some(port_str)) = (envs.get("ICECAST_HOST"), envs.get("ICECAST_PORT")) {
                println!("\nTesting configured Icecast host ({}:{})...", host, port_str);
                if let Ok(port) = port_str.parse::<u16>() {
                    let result = std::net::TcpStream::connect_timeout(
                        &format!("{}:{}", host, port).parse().unwrap(),
                        Duration::from_secs(2)
                    );
                    match result {
                        Ok(_) => {
                            println!("  ✓ Icecast server is reachable at {}:{}", host, port);
                        }
                        Err(e) => {
                            println!("  ⚠ Icecast server not reachable at {}:{}: {}", host, port, e);
                            println!("     (This is OK if Icecast is not running)");
                        }
                    }
                } else {
                    println!("  ⚠ Invalid ICECAST_PORT in config: {}", port_str);
                }
            } else {
                println!("\n⚠ ICECAST_HOST or ICECAST_PORT not found in config");
            }
        }
        Err(e) => {
            println!("\n⚠ Could not load GStreamer config: {}", e);
            println!("  Cannot test Icecast connectivity without config");
        }
    }
    
    println!();
    Ok(())
}

fn test_gstreamer_thread() -> Result<()> {
    println!("=== Testing GStreamer Thread Launch ===");
    
    // Load config from config_loader (single source of truth)
    let hostname = gethostname().to_string_lossy().to_string();
    let config = match config_loader::gstreamer_env_for(&hostname) {
        Ok(envs) => envs,
        Err(e) => {
            println!("⚠ Could not load GStreamer config: {}", e);
            println!("  Cannot test thread launch without config");
            return Ok(());
        }
    };
    
    let shutdown_flag = Arc::new(AtomicBool::new(false));
    
    println!("Starting GStreamer thread (will attempt to connect)...");
    println!("Note: This requires gst-launch-1.0 to be installed");
    println!("      and may fail if GStreamer is not available\n");
    
    gstreamer::start_gstreamer_thread(config, Arc::clone(&shutdown_flag));
    
    // Let it run for a few seconds
    println!("Thread started, waiting 3 seconds...");
    thread::sleep(Duration::from_secs(3));
    
    println!("Shutting down...");
    shutdown_flag.store(true, Ordering::Relaxed);
    
    thread::sleep(Duration::from_secs(1));
    println!("✓ Thread shutdown test completed\n");
    Ok(())
}

fn main() -> Result<()> {
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("GStreamer Module Test");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");
    
    // Initialize logging
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    
    // Run tests
    test_pipeline_building()?;
    test_config_loading()?;
    test_icecast_connectivity()?;
    
    // Only test thread launch if user wants (requires gstreamer)
    // Check command line args for --test-thread flag
    let test_thread = std::env::args().any(|arg| arg == "--test-thread");
    if test_thread {
        test_gstreamer_thread()?;
    } else {
        println!("Skipping thread launch test (use --test-thread flag to test)\n");
    }
    
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("All tests completed!");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    
    Ok(())
}

