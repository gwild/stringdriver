use std::process::{Command, Child, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use std::thread;
use log::{info, warn, error, debug};
use std::collections::BTreeMap;
use std::net::TcpStream;

/// Check if Icecast server is reachable
pub(crate) fn check_icecast_connectivity(host: &str, port: u16) -> bool {
    info!(target: "gstreamer", "Testing Icecast connectivity: {}:{}", host, port);
    match TcpStream::connect_timeout(
        &format!("{}:{}", host, port).parse().unwrap(),
        Duration::from_secs(5)
    ) {
        Ok(_) => {
            info!(target: "gstreamer", "â Icecast server reachable at {}:{}", host, port);
            true
        }
        Err(e) => {
            warn!(target: "gstreamer", "â  Icecast server not reachable: {}", e);
            false
        }
    }
}

/// Build GStreamer pipeline string from config
pub(crate) fn build_pipeline(config: &BTreeMap<String, String>, use_icecast: bool) -> (String, Vec<String>) {
    let audio_src = config.get("GSTREAMER_AUDIO_SRC")
        .expect("GSTREAMER_AUDIO_SRC missing");
    let convert = config.get("GSTREAMER_CONVERT")
        .expect("GSTREAMER_CONVERT missing");
    let encoder = config.get("GSTREAMER_ENCODER")
        .expect("GSTREAMER_ENCODER missing");
    
    let default_50 = "50".to_string();
    let default_0 = "0".to_string();
    
    let queue_buffers = config.get("GST_QUEUE_BUFFERS").unwrap_or(&default_50);
    let queue_time = config.get("GST_QUEUE_TIME").unwrap_or(&default_0);
    let queue_bytes = config.get("GST_QUEUE_BYTES").unwrap_or(&default_0);
    
    let mut pipeline = format!(
        "{} ! queue max-size-buffers={} max-size-time={} max-size-bytes={} leaky=2 ! {}",
        audio_src, queue_buffers, queue_time, queue_bytes, convert
    );
    
    // Optional second queue
    if config.get("GST_SECOND_QUEUE_ENABLE").map(|v| v == "true").unwrap_or(false) {
        let q2_buffers = config.get("GST_SECOND_QUEUE_BUFFERS").unwrap_or(&default_50);
        let q2_time = config.get("GST_SECOND_QUEUE_TIME").unwrap_or(&default_0);
        let q2_bytes = config.get("GST_SECOND_QUEUE_BYTES").unwrap_or(&default_0);
        
        info!(target: "gstreamer", "Adding second queue to pipeline");
        pipeline.push_str(&format!(
            " ! queue max-size-buffers={} max-size-time={} max-size-bytes={} leaky=2",
            q2_buffers, q2_time, q2_bytes
        ));
    }
    
    pipeline.push_str(&format!(" ! {}", encoder));
    
    let mut sink_props = Vec::new();
    
    if use_icecast {
        let sink = config.get("GSTREAMER_SINK").expect("GSTREAMER_SINK missing");
        
        // Build shout2send properties and embed them directly in pipeline string
        let mut props = Vec::new();
        if let Some(host) = config.get("ICECAST_HOST") {
            props.push(format!("ip={}", host));
        }
        if let Some(port) = config.get("ICECAST_PORT") {
            props.push(format!("port={}", port));
        }
        if let Some(password) = config.get("ICECAST_PASSWORD") {
            props.push(format!("password={}", password));
        }
        if let Some(mount) = config.get("ICECAST_MOUNT") {
            // Normalize mount point: add leading / if missing (matches gmixer.sh)
            let mount_normalized = if mount.starts_with('/') {
                mount.clone()
            } else {
                format!("/{}", mount)
            };
            props.push(format!("mount={}", mount_normalized));
        }
        if let Some(stream_name) = config.get("GSTREAMER_STREAM_NAME") {
            props.push(format!("streamname={}", stream_name));
        }
        if let Some(sync) = config.get("SHOUT2SEND_SYNC") {
            props.push(format!("sync={}", sync));
        }
        
        // Embed properties directly in pipeline string after sink element
        pipeline.push_str(&format!(" ! {} {}", sink, props.join(" ")));
        
        info!(target: "gstreamer", "Using shout2send sink with Icecast");
    } else {
        pipeline.push_str(" ! fakesink");
        warn!(target: "gstreamer", "Using fakesink (no Icecast connection)");
    }
    
    (pipeline, sink_props)
}

/// Launch and monitor GStreamer pipeline
pub fn start_gstreamer_thread(
    config: BTreeMap<String, String>,
    shutdown_flag: Arc<AtomicBool>,
) {
    thread::spawn(move || {
        info!(target: "gstreamer", "âââ GStreamer thread STARTED âââ");
        
        // Log configuration
        debug!(target: "gstreamer", "Configuration loaded:");
        for (key, value) in &config {
            if key.contains("PASSWORD") {
                debug!(target: "gstreamer", "  {} = [REDACTED]", key);
            } else {
                debug!(target: "gstreamer", "  {} = {}", key, value);
            }
        }
        
        // Check Icecast connectivity
        let use_icecast = if let (Some(host), Some(port_str)) = 
            (config.get("ICECAST_HOST"), config.get("ICECAST_PORT")) {
            if let Ok(port) = port_str.parse::<u16>() {
                check_icecast_connectivity(host, port)
            } else {
                warn!(target: "gstreamer", "Invalid ICECAST_PORT: {}", port_str);
                false
            }
        } else {
            warn!(target: "gstreamer", "ICECAST_HOST or ICECAST_PORT missing");
            false
        };
        
        // Build pipeline
        let (pipeline, _sink_props) = build_pipeline(&config, use_icecast);
        info!(target: "gstreamer", "Pipeline: {}", pipeline);
        
        let mut gst_child: Option<Child> = None;
        
        while !shutdown_flag.load(Ordering::Relaxed) {
            // Start GStreamer if not running
            if gst_child.is_none() {
                info!(target: "gstreamer", "JACK available - starting GStreamer pipeline");
                
                let mut cmd = Command::new("gst-launch-1.0");
                cmd.env("GST_DEBUG", "3");
                
                // Split pipeline string into arguments (properties are already embedded in pipeline)
                for arg in pipeline.split_whitespace() {
                    cmd.arg(arg);
                }
                
                cmd.stdout(Stdio::piped());
                cmd.stderr(Stdio::piped());
                
                match cmd.spawn() {
                    Ok(mut child) => {
                        let pid = child.id();
                        info!(target: "gstreamer", "â GStreamer launched (PID: {})", pid);
                        
                        // Spawn threads to log stdout/stderr
                        if let Some(stdout) = child.stdout.take() {
                            use std::io::{BufRead, BufReader};
                            thread::spawn(move || {
                                let reader = BufReader::new(stdout);
                                for line in reader.lines().flatten() {
                                    debug!(target: "gstreamer::stdout", "{}", line);
                                }
                            });
                        }
                        
                        if let Some(stderr) = child.stderr.take() {
                            use std::io::{BufRead, BufReader};
                            thread::spawn(move || {
                                let reader = BufReader::new(stderr);
                                for line in reader.lines().flatten() {
                                    // Log ERROR messages at error level, others at debug
                                    if line.contains("ERROR:") || line.contains("WARNING:") {
                                        error!(target: "gstreamer::stderr", "{}", line);
                                    } else {
                                    debug!(target: "gstreamer::stderr", "{}", line);
                                    }
                                }
                            });
                        }
                        
                        gst_child = Some(child);
                    }
                    Err(e) => {
                        error!(target: "gstreamer", "â Failed to launch GStreamer: {}", e);
                        thread::sleep(Duration::from_secs(5));
                        continue;
                    }
                }
            }
            
            // Check if GStreamer process is still alive
            if let Some(ref mut child) = gst_child {
                match child.try_wait() {
                    Ok(Some(status)) => {
                        warn!(target: "gstreamer", "GStreamer exited with status: {}", status);
                        gst_child = None;
                        thread::sleep(Duration::from_secs(2));
                    }
                    Ok(None) => {
                        // Still running
                        thread::sleep(Duration::from_millis(500));
                    }
                    Err(e) => {
                        error!(target: "gstreamer", "Error checking GStreamer status: {}", e);
                        gst_child = None;
                    }
                }
            }
        }
        
        // Cleanup on shutdown
        if let Some(mut child) = gst_child {
            info!(target: "gstreamer", "Stopping GStreamer on shutdown");
            let _ = child.kill();
            let _ = child.wait();
        }
        
        info!(target: "gstreamer", "âââ GStreamer thread SHUTDOWN âââ");
    });
}

