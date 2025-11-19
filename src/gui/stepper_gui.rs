use eframe::egui;
use std::thread;
use std::time::Duration;
use serialport;
use clap::Parser;
use std::fs::File;
use std::io::{Read, Write};
use std::process::Command;
use gethostname::gethostname;
use egui::Color32;
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::{Arc, Mutex};
use std::path::Path;

#[path = "../config_loader.rs"]
mod config_loader;
use config_loader::ArduinoFirmware;

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Args {
    #[arg(long)]
    debug: bool,
}

#[derive(Clone, Copy, Debug)]
struct CommandSet {
    positions_cmd: &'static [u8],
    rmove_id: u8,
    set_stepper_id: u8,
    set_accel_id: u8,
    set_speed_id: u8,
    set_min_id: u8,
    set_max_id: u8,
}

impl CommandSet {
    const fn new(
        positions_cmd: &'static [u8],
        rmove_id: u8,
        set_stepper_id: u8,
        set_accel_id: u8,
        set_speed_id: u8,
        set_min_id: u8,
        set_max_id: u8,
    ) -> Self {
        Self {
            positions_cmd,
            rmove_id,
            set_stepper_id,
            set_accel_id,
            set_speed_id,
            set_min_id,
            set_max_id,
        }
    }

    fn for_firmware(firmware: ArduinoFirmware) -> Self {
        match firmware {
            ArduinoFirmware::StringDriverV1 => CommandSet::new(b"2;", 4, 7, 8, 9, 10, 11),
            ArduinoFirmware::StringDriverV2 => CommandSet::new(b"1;", 3, 6, 7, 8, 9, 10),
        }
    }
}

#[derive(Debug)]
struct StepperGUI {
    port: Option<Box<dyn serialport::SerialPort>>,
    positions: Vec<i32>,
    connected: bool,
    tuner_port: Option<Box<dyn serialport::SerialPort>>,
    tuner_positions: Vec<i32>,
    tuner_connected: bool,
    debug_enabled: bool,
    debug_log: String,
    debug_file: Option<File>,
    port_path: String,
    tuner_port_path: Option<String>,
    string_num: usize,
    x_step_index: Option<usize>, // None means no X stepper
    z_first_index: Option<usize>, // None means no Z steppers
    tuner_first_index: Option<usize>, // None means no tuners
    tuner_num_steppers: Option<usize>, // Number of tuner steppers
    pending_positions: std::collections::HashMap<usize, i32>, // Store pending edits per stepper
    // Tuner stepper parameters (applied to all tuners)
    tuner_accel: i32,
    tuner_speed: i32,
    tuner_min: i32,
    tuner_max: i32,
    // X stepper parameters
    x_accel: i32,
    x_speed: i32,
    x_min: i32,
    x_max: i32,
    // Z stepper parameters (applied to all z steppers)
    z_accel: i32,
    z_speed: i32,
    z_min: i32,
    z_max: i32,
    z_up_step: i32,
    z_down_step: i32,
    socket_path: String,
    firmware: ArduinoFirmware,
    command_set: CommandSet,
    tuner_command_set: CommandSet,
    x_max_pos: Option<i32>, // X_MAX_POS from config for slider range
}

impl Default for StepperGUI {
    fn default() -> Self {
        Self {
            port: None,
            positions: vec![0; 13],
            connected: false,
            tuner_port: None,
            tuner_positions: Vec::new(),
            tuner_connected: false,
            debug_enabled: false,
            debug_log: String::new(),
            debug_file: None,
            port_path: String::new(),
            tuner_port_path: None,
            string_num: 0,
            x_step_index: None,
            z_first_index: None,
            tuner_first_index: None,
            tuner_num_steppers: None,
            pending_positions: std::collections::HashMap::new(),
            tuner_accel: 10000,
            tuner_speed: 250,
            tuner_min: -100000,
            tuner_max: 100000,
            x_accel: 10000,
            x_speed: 500,
            x_min: 0,
            x_max: 2600,
            z_accel: 10000,
            z_speed: 100,
            z_min: -100,
            z_max: 100,
            z_up_step: 2,
            z_down_step: -2,
            socket_path: String::new(),
            firmware: ArduinoFirmware::StringDriverV2,
            command_set: CommandSet::for_firmware(ArduinoFirmware::StringDriverV2),
            tuner_command_set: CommandSet::for_firmware(ArduinoFirmware::StringDriverV2),
            x_max_pos: None,
        }
    }
}

impl StepperGUI {
    fn write_positions_response(stream: &mut UnixStream, positions: &[i32]) -> std::io::Result<()> {
        use std::io::Write;
        let mut response = String::from("positions");
        for (idx, pos) in positions.iter().enumerate() {
            response.push(' ');
            response.push_str(&format!("{}={}", idx, pos));
        }
        response.push('\n');
        stream.write_all(response.as_bytes())?;
        stream.flush()
    }

    fn new(port_path: String, num_steppers: usize, string_num: usize, x_step_index: Option<usize>, z_first_index: Option<usize>, tuner_first_index: Option<usize>, tuner_port_path: Option<String>, tuner_num_steppers: Option<usize>, debug: bool, debug_file: Option<File>, z_up_step: i32, z_down_step: i32, firmware: ArduinoFirmware, x_max_pos: Option<i32>) -> Self {
        let mut s = Self::default();
        s.port_path = port_path;
        s.positions = vec![0; num_steppers];
        s.debug_enabled = debug;
        s.debug_file = debug_file;
        s.string_num = string_num;
        s.x_step_index = x_step_index;
        s.z_first_index = z_first_index;
        s.tuner_first_index = tuner_first_index;
        s.tuner_port_path = tuner_port_path.clone();
        s.tuner_num_steppers = tuner_num_steppers;
        s.firmware = firmware;
        let main_cmds = CommandSet::for_firmware(firmware);
        s.command_set = main_cmds;
        s.tuner_command_set = if tuner_port_path.is_some() {
            CommandSet::for_firmware(ArduinoFirmware::StringDriverV2)
        } else {
            main_cmds
        };
        if let Some(num) = tuner_num_steppers {
            s.tuner_positions = vec![0; num];
            // Set tuner min/max based on board type
            if tuner_port_path.is_some() {
                // Separate tuner board: -100000 to 100000
                s.tuner_min = -100000;
                s.tuner_max = 100000;
            } else if tuner_first_index.is_some() {
                // Main board tuners (stringdriver-1): -25000 to 25000
                s.tuner_min = -25000;
                s.tuner_max = 25000;
            }
        }
        s.z_up_step = z_up_step;
        s.z_down_step = z_down_step;
        s.log(&format!("Initialized: {} steppers, {} active string pairs", num_steppers, string_num));
        if tuner_first_index.is_some() {
            if tuner_port_path.is_some() {
                s.log(&format!("Tuners on separate board: {} steppers", tuner_num_steppers.unwrap_or(0)));
            } else {
                s.log(&format!("Tuners on main board: first_index={:?}", tuner_first_index));
            }
        }
        if debug { s.log("Debug logging enabled"); }
        // Generate socket path from port path
        let port_id = s.port_path.replace("/", "_").replace("\\", "_");
        s.socket_path = format!("/tmp/stepper_gui_{}.sock", port_id);
        s.x_max_pos = x_max_pos;
        s
    }
    
    /// Handle a text command from Unix socket
    fn handle_command(&mut self, cmd: &str, mut responder: Option<&mut UnixStream>) {
        let parts: Vec<&str> = cmd.trim().split_whitespace().collect();
        if parts.is_empty() {
            return;
        }
        
        match parts[0] {
            "rel_move" => {
                if parts.len() == 3 {
                    if let (Ok(stepper), Ok(delta)) = (parts[1].parse::<usize>(), parts[2].parse::<i32>()) {
                        self.log(&format!("IPC: rel_move {} {}", stepper, delta));
                        self.move_stepper_ipc(stepper, delta);
                    }
                }
            }
            "abs_move" => {
                if parts.len() == 3 {
                    if let (Ok(stepper), Ok(position)) = (parts[1].parse::<usize>(), parts[2].parse::<i32>()) {
                        self.log(&format!("IPC: abs_move {} {}", stepper, position));
                        self.set_position(stepper, position);
                    }
                }
            }
            "reset" => {
                if parts.len() == 3 {
                    if let (Ok(stepper), Ok(position)) = (parts[1].parse::<usize>(), parts[2].parse::<i32>()) {
                        self.log(&format!("IPC: reset {} {} (set_stepper - no physical move)", stepper, position));
                        self.reset_position(stepper, position);
                    }
                }
            }
            "get_positions" => {
                if let Some(stream) = responder.as_deref_mut() {
                    if let Err(e) = Self::write_positions_response(stream, &self.positions) {
                        self.log(&format!("IPC: Failed to send positions: {}", e));
                    }
                } else {
                    self.log("IPC: get_positions requested without responder stream");
                }
            }
            _ => {
                self.log(&format!("IPC: Unknown command: {}", cmd.trim()));
            }
        }
    }
    
    /// Start Unix socket listener in background thread
    fn start_socket_listener(app: Arc<Mutex<StepperGUI>>) {
        let socket_path = {
            let guard = app.lock().unwrap();
            guard.socket_path.clone()
        };
        
        // Remove old socket if it exists
        if Path::new(&socket_path).exists() {
            let _ = std::fs::remove_file(&socket_path);
        }
        
        thread::spawn(move || {
            let listener = match UnixListener::bind(&socket_path) {
                Ok(l) => {
                    eprintln!("Unix socket listener started at: {}", socket_path);
                    l
                }
                Err(e) => {
                    eprintln!("Failed to bind Unix socket at {}: {}", socket_path, e);
                    return;
                }
            };
            
            // Set socket permissions (read/write for user and group)
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if let Ok(metadata) = std::fs::metadata(&socket_path) {
                    let mut perms = metadata.permissions();
                    perms.set_mode(0o660);
                    let _ = std::fs::set_permissions(&socket_path, perms);
                }
            }
            
            for stream in listener.incoming() {
                match stream {
                    Ok(stream) => {
                        let app_clone = Arc::clone(&app);
                        thread::spawn(move || {
                            use std::io::{BufRead, BufReader};
                            let mut reader = BufReader::new(stream);
                            loop {
                                let mut cmd = String::new();
                                match reader.read_line(&mut cmd) {
                                    Ok(0) => break, // EOF
                                    Ok(_) => {
                                        let trimmed = cmd.trim();
                                        if trimmed.is_empty() {
                                            continue;
                                        }
                                        if let Ok(mut guard) = app_clone.lock() {
                                            let stream_ref = reader.get_mut();
                                            guard.handle_command(trimmed, Some(stream_ref));
                                        }
                                    }
                                    Err(e) => {
                                        eprintln!("Socket read error: {}", e);
                                        break;
                                    }
                                }
                            }
                        });
                    }
                    Err(e) => {
                        eprintln!("Socket accept error: {}", e);
                        // If we're getting too many errors, break to prevent infinite loop
                        if e.raw_os_error() == Some(24) {
                            eprintln!("Too many open files - breaking accept loop");
                            break;
                        }
                    }
                }
            }
        });
    }
    fn kill_port_users(&mut self, port_path: &str) {
        // Find PIDs with the port open
        let output = Command::new("/usr/bin/lsof")
            .arg("-t")
            .arg(port_path)
            .output();
        let Ok(out) = output else {
            self.log("lsof not available or failed");
            return;
        };
        if !out.status.success() {
            self.log(&format!("lsof returned status {}", out.status));
            return;
        }
        let pids_str = String::from_utf8_lossy(&out.stdout);
        let self_pid = std::process::id();
        for line in pids_str.lines() {
            if let Ok(pid) = line.trim().parse::<u32>() {
                if pid == self_pid { continue; }
                self.log(&format!("KILL port user pid={} on {}", pid, port_path));
                let _ = Command::new("/bin/kill").arg("-9").arg(pid.to_string()).status();
            }
        }
    }
    fn escape_cmdmessenger_bytes(data: &[u8]) -> Vec<u8> {
        // PyCmdMessenger escapes: field separator (','), command separator (';'), 
        // escape separator ('/'), and null bytes ('\0')
        let mut out = Vec::with_capacity(data.len() * 2); // May double in size if all bytes escaped
        for &b in data {
            match b {
                b'/' | b',' | b';' | 0 => { 
                    out.push(b'/'); 
                    out.push(b); 
                }
                _ => out.push(b),
            }
        }
        out
    }

    fn pack_i16_le(v: i16) -> [u8; 2] {
        i16::to_le_bytes(v)
    }

    fn pack_i32_le(v: i32) -> [u8; 4] {
        i32::to_le_bytes(v)
    }

    fn send_cmd_bin(&mut self, cmd_id: u8, stepper_idx: i16, value: i32) {
        // PyCmdMessenger sends "il" format: int (2 bytes) for stepper, long (4 bytes) for value
        // But Arduino reads both as int - that's fine, it just reads first 2 bytes of the long
        if self.port.is_none() { return; }
        let mut buf: Vec<u8> = Vec::with_capacity(20);
        // Command ID as ASCII digit
        buf.push(b'0' + cmd_id);
        buf.push(b',');
        // First arg: stepper index as 2-byte int
        let stepper_bytes = Self::pack_i16_le(stepper_idx);
        let escaped_stepper = Self::escape_cmdmessenger_bytes(&stepper_bytes);
        buf.extend_from_slice(&escaped_stepper);
        buf.push(b',');
        // Second arg: value as 4-byte long (Arduino reads as int, takes first 2 bytes)
        let value_bytes = Self::pack_i32_le(value);
        let escaped_value = Self::escape_cmdmessenger_bytes(&value_bytes);
        buf.extend_from_slice(&escaped_value);
        buf.push(b';');
        self.log(&format!("SEND BIN: {:?}", buf));
        if let Some(p) = self.port.as_mut() {
            let _ = p.write_all(&buf);
            let _ = p.flush();
        }
    }
    fn log(&mut self, message: &str) {
        // Always log to GUI buffer, even without debug flag
        self.debug_log.push_str(message);
        self.debug_log.push('\n');
        // Keep log size manageable
        if self.debug_log.len() > 10000 {
            self.debug_log = self.debug_log.split_off(self.debug_log.len() - 5000);
        }
        if self.debug_enabled {
            println!("DEBUG: {}", message);
            if let Some(f) = self.debug_file.as_mut() {
                let _ = f.write_all(format!("{}\n", message).as_bytes());
            }
        }
    }

    fn connect(&mut self) {
        let port_path = self.port_path.clone();
        self.kill_port_users(&port_path);
        self.log(&format!("Connecting to Arduino on {} @115200", port_path));
        match serialport::new(port_path.as_str(), 115200)
            .timeout(Duration::from_secs(2))
            .open() {
            Ok(port) => {
                self.log("Port opened, waiting 2s for Arduino reset...");
                thread::sleep(Duration::from_millis(2000));
                self.port = Some(port);
                self.connected = true;
                self.log("Connected. Requesting positions...");
                self.refresh_positions();
            }
            Err(e) => {
                self.log(&format!("Connection failed: {}", e));
            }
        }
    }

    fn refresh_positions(&mut self) {
        if self.port.is_some() {
            let send = self.command_set.positions_cmd;
            self.log(&format!("SEND: {:?}", send));
            let received = {
                let port = self.port.as_mut().unwrap();
                // Flush input buffer before command (mirror Python's flushInput)
                let _ = port.clear(serialport::ClearBuffer::Input);
                let _ = port.write_all(send);
                let _ = port.flush();
                
                // Arduino sends positions with delay(2) per position, so with 13 steppers that's ~26ms minimum
                // Wait a bit before starting to read
                thread::sleep(Duration::from_millis(50));
                
                // Read in a loop until we get complete message (ending with ';') or timeout
                let mut buffer = Vec::new();
                let start_time = std::time::Instant::now();
                let timeout = Duration::from_secs(2);
                
                while start_time.elapsed() < timeout {
                    let mut chunk = vec![0u8; 256];
                    match port.read(&mut chunk) {
                        Ok(bytes_read) if bytes_read > 0 => {
                            buffer.extend_from_slice(&chunk[..bytes_read]);
                            // Check if we have complete message (ends with ';')
                            if buffer.iter().any(|&b| b == b';') {
                                break;
                            }
                        }
                        Ok(_) => {
                            // No data available yet (timeout or empty read), wait a bit and retry
                            thread::sleep(Duration::from_millis(10));
                        }
                        Err(e) => {
                            // Timeout errors are expected - wait and retry
                            let err_str = e.to_string();
                            if err_str.contains("timeout") || err_str.contains("TimedOut") {
                                thread::sleep(Duration::from_millis(10));
                                continue;
                            }
                            // Other error - log and break
                            self.log(&format!("Read error: {}", e));
                            break;
                        }
                    }
                }
                
                if !buffer.is_empty() && buffer.iter().any(|&b| b == b';') {
                    Some(buffer)
                } else {
                    None
                }
            };

            if let Some(buffer) = received {
                self.log(&format!("RECV: {:?}", buffer));

                // Decode CmdMessenger: "1,<escaped-binary>;"
                let mut data_bytes: Vec<u8> = Vec::new();
                let mut seen_comma = false;
                let mut i = 0usize;
                while i < buffer.len() {
                    let b = buffer[i];
                    if !seen_comma {
                        if b == b',' { seen_comma = true; }
                        i += 1;
                        continue;
                    }
                    if b == b';' { break; }
                    if b == b'/' {
                        if i + 1 < buffer.len() {
                            data_bytes.push(buffer[i + 1]);
                            i += 2;
                            continue;
                        } else {
                            break;
                        }
                    }
                    if b == b',' { i += 1; continue; }
                    data_bytes.push(b);
                    i += 1;
                }

                let num = self.positions.len();
                let expected_bytes = num * 2;
                if data_bytes.len() < expected_bytes {
                    self.log(&format!(
                        "PARSE WARN: expected at least {} bytes, got {}",
                        expected_bytes, data_bytes.len()
                    ));
                }
                let mut positions = vec![0i32; num];
                for idx in 0..num {
                    let lo = idx * 2;
                    let hi = lo + 1;
                    if hi < data_bytes.len() {
                        positions[idx] = i16::from_le_bytes([data_bytes[lo], data_bytes[hi]]) as i32;
                    }
                }
                self.log(&format!("PARSED positions: {:?}", positions));
                self.positions = positions;
            } else {
                self.log("READ ERROR: failed to read from serial port");
            }
        }
    }

    fn move_stepper(&mut self, stepper: usize, delta: i32) {
        self.move_stepper_with_source("UI", stepper, delta);
    }

    fn move_stepper_ipc(&mut self, stepper: usize, delta: i32) {
        self.move_stepper_with_source("IPC", stepper, delta);
    }

    fn move_stepper_with_source(&mut self, source: &str, stepper: usize, delta: i32) {
        if self.port.is_none() {
            self.log(&format!("ERROR: Cannot move - port not connected"));
            return;
        }
        // Flush input before command (mirror Python's flush_input_before_command)
        if let Some(p) = self.port.as_mut() {
            let _ = p.clear(serialport::ClearBuffer::Input);
        }
        let s = stepper as i16;
        self.log(&format!(">>> {} MOVING stepper {} by {} (rmove command)", source, stepper, delta));
        self.send_cmd_bin(self.command_set.rmove_id, s, delta);
        self.log(&format!("Command sent, waiting for Arduino..."));
        // Arduino move is synchronous - wait for it to complete
        thread::sleep(Duration::from_millis(500));
        self.log(&format!("Refreshing positions..."));
        self.refresh_positions();
    }

    fn set_position(&mut self, stepper: usize, position: i32) {
        // CRITICAL: This is MODEL ONLY - updates internal position tracking variable
        // Does NOT send physical move command to Arduino
        // Real-world position comes from Arduino via refresh_positions() which calibrates the model
        // This maintains a parallel model that requires periodic calibration to reality
        let clamped = position.clamp(-100, 100);
        if stepper < self.positions.len() {
            self.positions[stepper] = clamped;
            self.log(&format!(">>> MODEL: Updated internal position for stepper {} to {} (code variable only, no physical move)", stepper, clamped));
        } else {
            self.log(&format!("ERROR: Stepper index {} out of range", stepper));
        }
    }

    fn reset_position(&mut self, stepper: usize, position: i32) {
        if self.port.is_none() {
            self.log(&format!("ERROR: Cannot reset position - port not connected"));
            return;
        }
        // Flush input before command
        if let Some(p) = self.port.as_mut() {
            let _ = p.clear(serialport::ClearBuffer::Input);
        }
        let s = stepper as i16;
        self.log(&format!(">>> RESETTING stepper {} to {} (set_stepper command - no physical move)", stepper, position));
        self.send_cmd_bin(self.command_set.set_stepper_id, s, position);
        self.log(&format!("Command sent, waiting for Arduino..."));
        // set_stepper is fast - just sets internal counter
        thread::sleep(Duration::from_millis(100));
        self.log(&format!("Refreshing positions..."));
        self.refresh_positions();
    }

    fn set_accel(&mut self, stepper: usize, accel: i32) {
        if self.port.is_none() {
            self.log(&format!("ERROR: Cannot set acceleration - port not connected"));
            return;
        }
        if let Some(p) = self.port.as_mut() {
            let _ = p.clear(serialport::ClearBuffer::Input);
        }
        let s = stepper as i16;
        self.log(&format!(">>> SETTING stepper {} acceleration to {} (set_accel command)", stepper, accel));
        self.send_cmd_bin(self.command_set.set_accel_id, s, accel);
    }

    fn set_speed(&mut self, stepper: usize, speed: i32) {
        if self.port.is_none() {
            self.log(&format!("ERROR: Cannot set speed - port not connected"));
            return;
        }
        if let Some(p) = self.port.as_mut() {
            let _ = p.clear(serialport::ClearBuffer::Input);
        }
        let s = stepper as i16;
        self.log(&format!(">>> SETTING stepper {} speed to {} (set_speed command)", stepper, speed));
        self.send_cmd_bin(self.command_set.set_speed_id, s, speed);
    }

    fn set_min(&mut self, axis: usize, min_val: i32) {
        if self.port.is_none() {
            self.log(&format!("ERROR: Cannot set min - port not connected"));
            return;
        }
        if let Some(p) = self.port.as_mut() {
            let _ = p.clear(serialport::ClearBuffer::Input);
        }
        let axis_idx = axis as i16;
        self.log(&format!(">>> SETTING axis {} min to {} (set_min command)", axis, min_val));
        self.send_cmd_bin(self.command_set.set_min_id, axis_idx, min_val);
    }

    fn set_max(&mut self, axis: usize, max_val: i32) {
        if self.port.is_none() {
            self.log(&format!("ERROR: Cannot set max - port not connected"));
            return;
        }
        if let Some(p) = self.port.as_mut() {
            let _ = p.clear(serialport::ClearBuffer::Input);
        }
        let axis_idx = axis as i16;
        self.log(&format!(">>> SETTING axis {} max to {} (set_max command)", axis, max_val));
        self.send_cmd_bin(self.command_set.set_max_id, axis_idx, max_val);
    }

    fn connect_tuner(&mut self) {
        if let Some(ref tuner_port_path) = self.tuner_port_path {
            let port_path = tuner_port_path.clone();
            self.kill_port_users(&port_path);
            self.log(&format!("Connecting to tuner Arduino on {} @115200", port_path));
            match serialport::new(port_path.as_str(), 115200)
                .timeout(Duration::from_secs(2))
                .open() {
                Ok(port) => {
                    self.log("Tuner port opened, waiting 2s for Arduino reset...");
                    thread::sleep(Duration::from_millis(2000));
                    self.tuner_port = Some(port);
                    self.tuner_connected = true;
                    self.log("Tuner connected. Requesting positions...");
                    self.refresh_tuner_positions();
                }
                Err(e) => {
                    self.log(&format!("Tuner connection failed: {}", e));
                }
            }
        } else if self.tuner_first_index.is_some() {
            // Tuners on main board - positions come from main board
            self.log("Tuners on main board - using main positions");
            self.tuner_connected = true;
            self.refresh_tuner_positions();
        }
    }

    fn refresh_tuner_positions(&mut self) {
        if let Some(ref mut tuner_port) = self.tuner_port {
            let send = self.command_set.positions_cmd;
            let log_msg = format!("TUNER SEND: {:?}", send);
            let _ = tuner_port; // Release borrow before logging
            self.log(&log_msg);
            
            let received = {
                let port = self.tuner_port.as_mut().unwrap();
                let _ = port.clear(serialport::ClearBuffer::Input);
                let _ = port.write_all(send);
                let _ = port.flush();
                thread::sleep(Duration::from_millis(50));
                
                let mut buffer = Vec::new();
                let start_time = std::time::Instant::now();
                let timeout = Duration::from_secs(2);
                
                while start_time.elapsed() < timeout {
                    let mut chunk = vec![0u8; 256];
                    match port.read(&mut chunk) {
                        Ok(bytes_read) if bytes_read > 0 => {
                            buffer.extend_from_slice(&chunk[..bytes_read]);
                            if buffer.iter().any(|&b| b == b';') {
                                break;
                            }
                        }
                        Ok(_) => {
                            thread::sleep(Duration::from_millis(10));
                        }
                        Err(e) => {
                            let err_str = e.to_string();
                            if err_str.contains("timeout") || err_str.contains("TimedOut") {
                                thread::sleep(Duration::from_millis(10));
                                continue;
                            }
                            let _ = port; // Release borrow before logging
                            let log_msg = format!("Tuner read error: {}", e);
                            self.log(&log_msg);
                            break;
                        }
                    }
                }
                
                if !buffer.is_empty() && buffer.iter().any(|&b| b == b';') {
                    Some(buffer)
                } else {
                    None
                }
            };

            if let Some(buffer) = received {
                let log_msg = format!("TUNER RECV: {:?}", buffer);
                self.log(&log_msg);
                let mut data_bytes: Vec<u8> = Vec::new();
                let mut seen_comma = false;
                let mut i = 0usize;
                while i < buffer.len() {
                    let b = buffer[i];
                    if !seen_comma {
                        if b == b',' { seen_comma = true; }
                        i += 1;
                        continue;
                    }
                    if b == b';' { break; }
                    if b == b'/' {
                        if i + 1 < buffer.len() {
                            data_bytes.push(buffer[i + 1]);
                            i += 2;
                            continue;
                        } else {
                            break;
                        }
                    }
                    if b == b',' { i += 1; continue; }
                    data_bytes.push(b);
                    i += 1;
                }

                let num = self.tuner_positions.len();
                let expected_bytes = num * 2;
                if data_bytes.len() < expected_bytes {
                    let log_msg = format!("TUNER PARSE WARN: expected at least {} bytes, got {}", expected_bytes, data_bytes.len());
                    self.log(&log_msg);
                }
                for idx in 0..num {
                    let lo = idx * 2;
                    let hi = lo + 1;
                    if hi < data_bytes.len() {
                        self.tuner_positions[idx] = i16::from_le_bytes([data_bytes[lo], data_bytes[hi]]) as i32;
                    }
                }
                let log_msg = format!("TUNER PARSED positions: {:?}", self.tuner_positions);
                self.log(&log_msg);
            } else {
                self.log("TUNER READ ERROR: failed to read from serial port");
            }
        } else if self.tuner_first_index.is_some() && self.tuner_connected {
            // Tuners on main board - extract from main positions
            if let Some(tuner_first) = self.tuner_first_index {
                if let Some(num) = self.tuner_num_steppers {
                    for i in 0..num {
                        let main_idx = tuner_first + i;
                        if main_idx < self.positions.len() {
                            self.tuner_positions[i] = self.positions[main_idx];
                        }
                    }
                }
            }
        }
    }

    fn move_tuner(&mut self, tuner_idx: usize, delta: i32) {
        if self.tuner_port.is_some() {
            // Tuners on separate board
            if let Some(ref mut port) = self.tuner_port {
                let _ = port.clear(serialport::ClearBuffer::Input);
            }
            let t = tuner_idx as i16;
            self.log(&format!(">>> MOVING tuner {} by {} (rmove command)", tuner_idx, delta));
            self.send_cmd_bin_tuner(self.tuner_command_set.rmove_id, t, delta);
            thread::sleep(Duration::from_millis(500));
            self.refresh_tuner_positions();
        } else if self.tuner_first_index.is_some() {
            // Tuners on main board - use main board
            if let Some(tuner_first) = self.tuner_first_index {
                let main_idx = tuner_first + tuner_idx;
                self.move_stepper(main_idx, delta);
            }
        }
    }

    fn send_cmd_bin_tuner(&mut self, cmd_id: u8, stepper_idx: i16, value: i32) {
        if self.tuner_port.is_none() { return; }
        let mut buf: Vec<u8> = Vec::with_capacity(20);
        buf.push(b'0' + cmd_id);
        buf.push(b',');
        let stepper_bytes = Self::pack_i16_le(stepper_idx);
        let escaped_stepper = Self::escape_cmdmessenger_bytes(&stepper_bytes);
        buf.extend_from_slice(&escaped_stepper);
        buf.push(b',');
        let value_bytes = Self::pack_i32_le(value);
        let escaped_value = Self::escape_cmdmessenger_bytes(&value_bytes);
        buf.extend_from_slice(&escaped_value);
        buf.push(b';');
        self.log(&format!("TUNER SEND BIN: {:?}", buf));
        if let Some(p) = self.tuner_port.as_mut() {
            let _ = p.write_all(&buf);
            let _ = p.flush();
        }
    }

    fn set_tuner_accel(&mut self, tuner_idx: usize, accel: i32) {
        if self.tuner_port.is_some() {
            // Tuners on separate board
            if let Some(ref mut port) = self.tuner_port {
                let _ = port.clear(serialport::ClearBuffer::Input);
            }
            let t = tuner_idx as i16;
            self.log(&format!(">>> SETTING tuner {} acceleration to {} (set_accel command)", tuner_idx, accel));
            self.send_cmd_bin_tuner(self.tuner_command_set.set_accel_id, t, accel);
        } else if self.tuner_first_index.is_some() {
            // Tuners on main board - use main board
            if let Some(tuner_first) = self.tuner_first_index {
                let main_idx = tuner_first + tuner_idx;
                self.set_accel(main_idx, accel);
            }
        }
    }

    fn set_tuner_speed(&mut self, tuner_idx: usize, speed: i32) {
        if self.tuner_port.is_some() {
            // Tuners on separate board
            if let Some(ref mut port) = self.tuner_port {
                let _ = port.clear(serialport::ClearBuffer::Input);
            }
            let t = tuner_idx as i16;
            self.log(&format!(">>> SETTING tuner {} speed to {} (set_speed command)", tuner_idx, speed));
            self.send_cmd_bin_tuner(self.tuner_command_set.set_speed_id, t, speed);
        } else if self.tuner_first_index.is_some() {
            // Tuners on main board - use main board
            if let Some(tuner_first) = self.tuner_first_index {
                let main_idx = tuner_first + tuner_idx;
                self.set_speed(main_idx, speed);
            }
        }
    }

    fn set_tuner_min(&mut self, tuner_idx: usize, min_val: i32) {
        if self.tuner_port.is_some() {
            // Tuners on separate board
            if let Some(ref mut port) = self.tuner_port {
                let _ = port.clear(serialport::ClearBuffer::Input);
            }
            let t = tuner_idx as i16;
            self.log(&format!(">>> SETTING tuner {} min to {} (set_min command)", tuner_idx, min_val));
            self.send_cmd_bin_tuner(self.tuner_command_set.set_min_id, t, min_val);
        } else if self.tuner_first_index.is_some() {
            // Tuners on main board - use main board
            if let Some(_tuner_first) = self.tuner_first_index {
                self.set_min(0, min_val); // Still use axis=0 for min/max
            }
        }
    }

    fn set_tuner_max(&mut self, tuner_idx: usize, max_val: i32) {
        if self.tuner_port.is_some() {
            // Tuners on separate board
            if let Some(ref mut port) = self.tuner_port {
                let _ = port.clear(serialport::ClearBuffer::Input);
            }
            let t = tuner_idx as i16;
            self.log(&format!(">>> SETTING tuner {} max to {} (set_max command)", tuner_idx, max_val));
            self.send_cmd_bin_tuner(self.tuner_command_set.set_max_id, t, max_val);
        } else if self.tuner_first_index.is_some() {
            // Tuners on main board - use main board
            if let Some(_tuner_first) = self.tuner_first_index {
                self.set_max(0, max_val); // Still use axis=0 for min/max
            }
        }
    }

    fn apply_z_params_to_all(&mut self) {
        // Apply z parameters to all z steppers using Z_FIRST_INDEX from config
        if let Some(z_first) = self.z_first_index {
            let num_z = self.string_num * 2; // Each string has 2 Z steppers (in/out)
            for i in 0..num_z {
                let stepper_idx = z_first + i;
                if stepper_idx < self.positions.len() {
                    self.set_accel(stepper_idx, self.z_accel);
                    thread::sleep(Duration::from_millis(10));
                    self.set_speed(stepper_idx, self.z_speed);
                    thread::sleep(Duration::from_millis(10));
                    // Iterate through all Z steppers for min/max too
                    self.set_min(1, self.z_min);
                    thread::sleep(Duration::from_millis(10));
                    self.set_max(1, self.z_max);
                    thread::sleep(Duration::from_millis(10));
                }
            }
        }
    }
}

impl eframe::App for StepperGUI {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ctx, |ui| {
            if !self.connected {
                ui.label("Connecting to Arduino...");
                return;
            }
            
            // Refresh positions periodically (every 500ms)
            ctx.request_repaint_after(Duration::from_millis(500));


            // Channel colors matching plot.rs color scheme
            let channel_colors = vec![
                Color32::from_rgb(0, 0, 255),      // Blue
                Color32::from_rgb(255, 165, 0),    // Orange
                Color32::from_rgb(0, 255, 0),       // Green
                Color32::from_rgb(255, 0, 0),       // Red
                Color32::from_rgb(238, 130, 238),   // Magenta
                Color32::from_rgb(165, 42, 42),    // Brown
            ];

            // Check if X stepper exists using X_STEP_INDEX from config
            let x_offset = if let Some(x_idx) = self.x_step_index {
                if x_idx < self.positions.len() && self.positions[x_idx] >= 0 {
                    self.positions[x_idx] as f32 // Shift layout based on x-axis position
                } else {
                    0.0 // X stepper index out of range or invalid position
                }
            } else {
                0.0 // No X stepper for this host
            };

            egui::ScrollArea::vertical().show(ui, |ui| {
                // Layout steppers in pairs matching physical system:
                // (2,1), (4,3), (6,5), (8,7), (10,9), (12,11)
                // Stepper 0 is x-axis carriage (if present)
                
                // Show X stepper separately if it exists and is valid
                if let Some(x_idx) = self.x_step_index {
                    if x_idx < self.positions.len() && self.positions[x_idx] >= 0 {
                        ui.horizontal(|ui| {
                            ui.label(&format!("X-axis (Stepper {}):", x_idx));
                            let mut pos = self.positions[x_idx];
                            // Read-only horizontal slider for visualization
                            // Use X_MAX_POS from config
                            if let Some(max_range) = self.x_max_pos {
                                // Allocate wider space for slider (default slider width + 30 pixels)
                                // Use egui's default slider width, not available_width (which scales with window)
                                let default_slider_width = ui.spacing().slider_width;
                                let slider_width = default_slider_width + 30.0;
                                let slider_height = ui.spacing().interact_size.y;
                                
                                // Allocate space for the slider
                                let slider_response = ui.allocate_response(
                                    egui::vec2(slider_width, slider_height),
                                    egui::Sense::hover()
                                );
                                
                                // Draw custom slider with white indicator
                                let slider_rect = slider_response.rect;
                                let painter = ui.painter();
                                
                                // Draw slider track background
                                let track_height = 4.0;
                                let track_rect = egui::Rect::from_center_size(
                                    slider_rect.center(),
                                    egui::vec2(slider_rect.width(), track_height)
                                );
                                painter.rect_filled(track_rect, 2.0, egui::Color32::from_gray(60));
                                
                                // Calculate normalized position (0.0 to 1.0)
                                let normalized_pos = (pos as f32 + 100.0) / (max_range as f32 + 100.0);
                                let normalized_pos = normalized_pos.clamp(0.0, 1.0);
                                
                                // Draw filled portion (position indicator)
                                let fill_width = slider_rect.width() * normalized_pos;
                                let fill_rect = egui::Rect::from_min_size(
                                    slider_rect.min,
                                    egui::vec2(fill_width, track_height)
                                );
                                painter.rect_filled(fill_rect, 2.0, egui::Color32::from_gray(120));
                                
                                // Draw white indicator circle
                                let indicator_x = slider_rect.min.x + fill_width;
                                let indicator_y = slider_rect.center().y;
                                painter.circle_filled(
                                    egui::pos2(indicator_x, indicator_y),
                                    6.0,
                                    egui::Color32::WHITE
                                );
                                
                                // Skip adding the actual slider widget - we've drawn a custom one
                                // The allocated space above provides the layout
                                
                                // Editable number box (like Z steppers)
                                let current_pos = self.positions[x_idx];
                                let pending = self.pending_positions.entry(x_idx).or_insert(current_pos);
                                let response = ui.add(egui::DragValue::new(pending)
                                    .clamp_range(-100..=max_range)
                                    .speed(10.0));
                                
                                let has_focus = response.has_focus();
                                let lost_focus = response.lost_focus();
                                let enter_pressed = ui.input(|i| i.key_pressed(egui::Key::Enter));
                                
                                // Only send command when Enter is pressed (lost focus + Enter key)
                                if lost_focus && enter_pressed {
                                    let pending_value = *pending;
                                    drop(pending); // Release borrow
                                    let delta = pending_value - current_pos;
                                    if delta != 0 {
                                        self.move_stepper(x_idx, delta);
                                    }
                                    self.pending_positions.remove(&x_idx);
                                } else if !has_focus && *pending != current_pos {
                                    // Sync pending value when not editing
                                    *pending = current_pos;
                                }
                            } else {
                                // No X_MAX_POS configured - show position without slider
                                ui.label(format!("Position: {}", pos));
                            }
                        });
                        
                        // X stepper parameter controls in narrow rows
                        ui.horizontal(|ui| {
                            ui.label("X:");
                            let accel_response = ui.add(egui::DragValue::new(&mut self.x_accel).speed(100.0).prefix("Accel: "));
                            if accel_response.changed() {
                                self.set_accel(x_idx, self.x_accel);
                            }
                            let speed_response = ui.add(egui::DragValue::new(&mut self.x_speed).speed(10.0).prefix("Speed: "));
                            if speed_response.changed() {
                                self.set_speed(x_idx, self.x_speed);
                            }
                        });
                        ui.horizontal(|ui| {
                            ui.add_space(30.0); // Indent to align with above row
                            let min_response = ui.add(egui::DragValue::new(&mut self.x_min).speed(10.0).prefix("Min: "));
                            if min_response.changed() {
                                self.set_min(0, self.x_min);
                            }
                            let max_response = ui.add(egui::DragValue::new(&mut self.x_max).speed(10.0).prefix("Max: "));
                            if max_response.changed() {
                                self.set_max(0, self.x_max);
                            }
                        });
                        ui.separator();
                    }
                }
                
                // Global Z stepper parameter controls in narrow rows (below x stepper, above z sliders)
                ui.horizontal(|ui| {
                    ui.label("Z (all):");
                    let accel_response = ui.add(egui::DragValue::new(&mut self.z_accel).speed(100.0).prefix("Accel: "));
                    if accel_response.changed() {
                        self.apply_z_params_to_all();
                    }
                    let speed_response = ui.add(egui::DragValue::new(&mut self.z_speed).speed(10.0).prefix("Speed: "));
                    if speed_response.changed() {
                        self.apply_z_params_to_all();
                    }
                });
                ui.horizontal(|ui| {
                    ui.add_space(30.0); // Indent to align with above row
                    let min_response = ui.add(egui::DragValue::new(&mut self.z_min).speed(10.0).prefix("Min: "));
                    if min_response.changed() {
                        self.apply_z_params_to_all();
                    }
                    let max_response = ui.add(egui::DragValue::new(&mut self.z_max).speed(10.0).prefix("Max: "));
                    if max_response.changed() {
                        self.apply_z_params_to_all();
                    }
                });
                ui.horizontal(|ui| {
                    ui.add_space(30.0); // Indent to align with above row
                    let mut down_step = self.z_down_step;
                    let down_response = ui.add(egui::DragValue::new(&mut down_step).speed(1.0).clamp_range(-10..=-2).prefix("Down Step: "));
                    if down_response.changed() {
                        self.z_down_step = down_step;
                    }
                    let mut up_step = self.z_up_step;
                    let up_response = ui.add(egui::DragValue::new(&mut up_step).speed(1.0).clamp_range(2..=10).prefix("Up Step: "));
                    if up_response.changed() {
                        self.z_up_step = up_step;
                    }
                });
                ui.separator();

                // Arrange z-steppers in pairs using Z_FIRST_INDEX from config
                // Only show pairs for active strings/channels (from STRING_NUM in YAML)
                let num_pairs_to_show = self.string_num;
                if let Some(z_first) = self.z_first_index {
                    for row in 0..num_pairs_to_show {
                        // Z steppers are arranged as pairs: (in, out) for each string
                        // Even indices are "in", odd indices are "out"
                        // For stringdriver-3: z_first=1, pairs at (2,1), (4,3), (6,5), (8,7)
                        // For stringdriver-1: z_first=3, pairs at (4,3), (6,5)
                        let left_idx = z_first + (row * 2) + 1;  // "out" stepper (odd)
                        let right_idx = z_first + (row * 2);     // "in" stepper (even)
                        
                        if left_idx >= self.positions.len() || right_idx >= self.positions.len() {
                            break;
                        }

                        let color = channel_colors[row % channel_colors.len()];

                        ui.horizontal(|ui| {
                            // Apply horizontal offset based on x-axis carriage position
                            if x_offset > 0.0 {
                                ui.add_space(x_offset.min(500.0)); // Limit offset to reasonable screen space
                            }
                            
                            // Left stepper ("out" stepper)
                            ui.vertical(|ui| {
                                ui.label(format!("Stepper {} (out)", left_idx));
                            
                            // Horizontal layout: slider on left, number box with buttons on right (tight spacing)
                            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center).with_main_justify(false), |ui| {
                                ui.set_width(80.0); // Constrain width to keep layout tight
                                
                                // Read-only vertical slider for visualization with colored background
                                let pos_display = self.positions[left_idx];
                                let pos_normalized = (pos_display + 100) as f32 / 200.0; // Normalize -100..100 to 0..1
                                
                                // Draw colored slider area (half size: 20x100 instead of 40x200)
                                let desired_size = egui::vec2(20.0, 100.0);
                                let response = ui.allocate_response(desired_size, egui::Sense::hover());
                                let rect = response.rect;
                                let painter = ui.painter();
                                // Draw background
                                painter.rect_filled(rect, 0.0, egui::Color32::from_rgb(40, 40, 40));
                                // Draw filled portion with channel color
                                let fill_height = rect.height() * pos_normalized;
                                let fill_rect = egui::Rect::from_min_size(
                                    rect.min,
                                    egui::vec2(rect.width(), fill_height)
                                );
                                painter.rect_filled(fill_rect, 0.0, color);
                                // Draw slider thumb
                                let thumb_y = rect.min.y + rect.height() * (1.0 - pos_normalized);
                                painter.circle_filled(egui::pos2(rect.center().x, thumb_y), 4.0, Color32::WHITE);
                                
                                // Vertical stack: + button, number box, - button
                                // Number box should align with slider center (0 position)
                                ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                                    // Add space to align number box center with slider center
                                    // Slider is 100px tall, center is at 50px
                                    // Estimate: button ~20px, number box ~20px, so add ~20px space
                                    ui.add_space(20.0);
                                    
                                    // Inc (+) button above number box
                                    if ui.button("+").clicked() {
                                        self.move_stepper(left_idx, self.z_up_step);
                                    }
                                    
                                    // Use DragValue for proper number input, but only commit on Enter
                                    let current_pos = self.positions[left_idx];
                                    let pending = self.pending_positions.entry(left_idx).or_insert(current_pos);
                                    let response = ui.add(egui::DragValue::new(pending)
                                        .clamp_range(-100..=100)
                                        .speed(1.0));
                                    
                                    let has_focus = response.has_focus();
                                    let lost_focus = response.lost_focus();
                                    let enter_pressed = ui.input(|i| i.key_pressed(egui::Key::Enter));
                                    
                                    // Only send command when Enter is pressed (lost focus + Enter key)
                                    // Check this FIRST before syncing, otherwise we'll reset pending value
                                    if lost_focus && enter_pressed {
                                        let pending_value = *pending; // Capture value before any reset
                                        let _ = pending; // Release borrow
                                        self.log(&format!("DEBUG Enter pressed for left_idx={}: pending_value={}, current_pos={}", 
                                            left_idx, pending_value, current_pos));
                                        let clamped = pending_value.clamp(-100, 100);
                                        self.set_position(left_idx, clamped);
                                        self.pending_positions.insert(left_idx, clamped);
                                    } else {
                                        // Only sync pending value if user is NOT editing (widget not focused)
                                        // This prevents overwriting user's input while they're typing
                                        if !has_focus && *pending != current_pos {
                                            *pending = current_pos;
                                        }
                                    }
                                    
                                    // Dec (-) button below number box
                                    if ui.button("-").clicked() {
                                        self.move_stepper(left_idx, self.z_down_step);
                                    }
                                });
                            });
                        });
                            
                            // Right stepper ("in" stepper)
                            ui.vertical(|ui| {
                                ui.label(format!("Stepper {} (in)", right_idx));
                            
                            // Horizontal layout: slider on left, number box with buttons on right (tight spacing)
                            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center).with_main_justify(false), |ui| {
                                ui.set_width(80.0); // Constrain width to keep layout tight
                                
                                // Read-only vertical slider for visualization with colored background
                                let pos_display = self.positions[right_idx];
                                let pos_normalized = (pos_display + 100) as f32 / 200.0; // Normalize -100..100 to 0..1
                                
                                // Draw colored slider area (half size: 20x100 instead of 40x200)
                                let desired_size = egui::vec2(20.0, 100.0);
                                let response = ui.allocate_response(desired_size, egui::Sense::hover());
                                let rect = response.rect;
                                let painter = ui.painter();
                                // Draw background
                                painter.rect_filled(rect, 0.0, egui::Color32::from_rgb(40, 40, 40));
                                // Draw filled portion with channel color
                                let fill_height = rect.height() * pos_normalized;
                                let fill_rect = egui::Rect::from_min_size(
                                    rect.min,
                                    egui::vec2(rect.width(), fill_height)
                                );
                                painter.rect_filled(fill_rect, 0.0, color);
                                // Draw slider thumb
                                let thumb_y = rect.min.y + rect.height() * (1.0 - pos_normalized);
                                painter.circle_filled(egui::pos2(rect.center().x, thumb_y), 4.0, Color32::WHITE);
                                
                                // Vertical stack: + button, number box, - button
                                // Number box should align with slider center (0 position)
                                ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                                    // Add space to align number box center with slider center
                                    // Slider is 100px tall, center is at 50px
                                    // Estimate: button ~20px, number box ~20px, so add ~20px space
                                    ui.add_space(20.0);
                                    
                                    // Inc (+) button above number box
                                    if ui.button("+").clicked() {
                                        self.move_stepper(right_idx, self.z_up_step);
                                    }
                                    
                                    // Use DragValue for proper number input, but only commit on Enter
                                    let current_pos = self.positions[right_idx];
                                    let pending = self.pending_positions.entry(right_idx).or_insert(current_pos);
                                    let response = ui.add(egui::DragValue::new(pending)
                                        .clamp_range(-100..=100)
                                        .speed(1.0));
                                    
                                    let has_focus = response.has_focus();
                                    let lost_focus = response.lost_focus();
                                    let enter_pressed = ui.input(|i| i.key_pressed(egui::Key::Enter));
                                    
                                    // Only send command when Enter is pressed (lost focus + Enter key)
                                    // Check this FIRST before syncing, otherwise we'll reset pending value
                                    if lost_focus && enter_pressed {
                                        let pending_value = *pending; // Capture value before any reset
                                        let _ = pending; // Release borrow
                                        self.log(&format!("DEBUG Enter pressed for right_idx={}: pending_value={}, current_pos={}", 
                                            right_idx, pending_value, current_pos));
                                        let clamped = pending_value.clamp(-100, 100);
                                        self.set_position(right_idx, clamped);
                                        self.pending_positions.insert(right_idx, clamped);
                                    } else {
                                        // Only sync pending value if user is NOT editing (widget not focused)
                                        // This prevents overwriting user's input while they're typing
                                        if !has_focus && *pending != current_pos {
                                            *pending = current_pos;
                                        }
                                    }
                                    
                                    // Dec (-) button below number box
                                    if ui.button("-").clicked() {
                                        self.move_stepper(right_idx, self.z_down_step);
                                    }
                                });
                            });
                        });
                    });
                }
            }
                
            // Display tuner steppers as rotary dials below Z steppers
            if self.tuner_first_index.is_some() {
                if let Some(num_tuners) = self.tuner_num_steppers {
                        ui.separator();
                        
                        // Tuner parameter controls in narrow rows (above tuner dials)
                        ui.horizontal(|ui| {
                            ui.label("Tuners:");
                            let accel_response = ui.add(egui::DragValue::new(&mut self.tuner_accel).speed(100.0).prefix("Accel: "));
                            if accel_response.changed() {
                                // Apply to all tuners
                                for tuner_idx in 0..num_tuners {
                                    self.set_tuner_accel(tuner_idx, self.tuner_accel);
                                    thread::sleep(Duration::from_millis(10));
                                }
                            }
                            let speed_response = ui.add(egui::DragValue::new(&mut self.tuner_speed).speed(10.0).prefix("Speed: "));
                            if speed_response.changed() {
                                // Apply to all tuners
                                for tuner_idx in 0..num_tuners {
                                    self.set_tuner_speed(tuner_idx, self.tuner_speed);
                                    thread::sleep(Duration::from_millis(10));
                                }
                            }
                        });
                        ui.horizontal(|ui| {
                            ui.add_space(30.0); // Indent to align with above row
                            let min_response = ui.add(egui::DragValue::new(&mut self.tuner_min).speed(1000.0).prefix("Min: "));
                            if min_response.changed() {
                                // Apply to all tuners
                                for tuner_idx in 0..num_tuners {
                                    self.set_tuner_min(tuner_idx, self.tuner_min);
                                    thread::sleep(Duration::from_millis(10));
                                }
                            }
                            let max_response = ui.add(egui::DragValue::new(&mut self.tuner_max).speed(1000.0).prefix("Max: "));
                            if max_response.changed() {
                                // Apply to all tuners
                                for tuner_idx in 0..num_tuners {
                                    self.set_tuner_max(tuner_idx, self.tuner_max);
                                    thread::sleep(Duration::from_millis(10));
                                }
                            }
                        });
                        ui.separator();
                        
                        ui.label("Tuner positions:");
                        ui.horizontal(|ui| {
                            for tuner_idx in 0..num_tuners {
                                ui.vertical(|ui| {
                                    ui.label(format!("Tuner {}", tuner_idx));
                                    let channel_color = channel_colors[tuner_idx % channel_colors.len()];
                                    
                                    // Get tuner position (from separate board or main board)
                                    let tuner_pos = if tuner_idx < self.tuner_positions.len() {
                                        self.tuner_positions[tuner_idx]
                                    } else {
                                        0
                                    };
                                    
                                    // Rotary dial visualization - circular indicator
                                    let desired_size = egui::vec2(60.0, 60.0);
                                    let response = ui.allocate_response(desired_size, egui::Sense::hover());
                                    let rect = response.rect;
                                    let painter = ui.painter();
                                    
                                    // Draw circle background
                                    let radius = rect.width() / 2.0 - 2.0;
                                    painter.circle_filled(rect.center(), radius, egui::Color32::from_rgb(40, 40, 40));
                                    painter.circle_stroke(rect.center(), radius, egui::Stroke::new(2.0, channel_color));
                                    
                                    // Normalize tuner position to 0-2π for dial indicator
                                    // Tuner range: -100000 to 100000 (Tuner_Driver) or -25000 to 25000 (String_Driver)
                                    // For rotary dial, map position to angle: 0 at top (12 o'clock), clockwise
                                    let tuner_range = if self.tuner_port.is_some() {
                                        200000.0 // Separate tuner board: -100000 to 100000
                                    } else {
                                        50000.0  // Main board (stringdriver-1): -25000 to 25000
                                    };
                                    let normalized = ((tuner_pos as f32 + tuner_range / 2.0) / tuner_range).clamp(0.0, 1.0);
                                    let angle = normalized * std::f32::consts::TAU - std::f32::consts::FRAC_PI_2; // Start at top (12 o'clock)
                                    let radius = rect.width() / 2.0 - 5.0;
                                    let end_x = rect.center().x + angle.cos() * radius;
                                    let end_y = rect.center().y - angle.sin() * radius;
                                    painter.line_segment(
                                        [rect.center(), egui::pos2(end_x, end_y)],
                                        (2.0, channel_color)
                                    );
                                    
                                    // Display position value
                                    ui.label(format!("{}", tuner_pos));
                                    
                                    // Control buttons
                                    ui.horizontal(|ui| {
                                        if ui.button("-").clicked() {
                                            self.move_tuner(tuner_idx, -10);
                                        }
                                        if ui.button("+").clicked() {
                                            self.move_tuner(tuner_idx, 10);
                                        }
                                    });
                                });
                                ui.add_space(10.0);
                            }
                        });
                    }
                }
            });
            ui.collapsing("Debug log", |ui| {
                ui.horizontal(|ui| {
                    if ui.button("Clear log").clicked() {
                        self.debug_log.clear();
                    }
                    if ui.button("Copy log").clicked() {
                        let log = self.debug_log.clone();
                        ui.output_mut(|o| o.copied_text = log);
                    }
                });
                egui::ScrollArea::vertical()
                    .max_height(400.0)
                    .auto_shrink([false; 2])
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        ui.add(
                            egui::TextEdit::multiline(&mut self.debug_log)
                                .desired_width(f32::INFINITY)
                                .interactive(true)
                                .code_editor()
                        );
                    });
            });

            ctx.request_repaint_after(Duration::from_millis(500));
        });
    }
}

fn main() {
    let args = Args::parse();
    let mut debug_file: Option<File> = None;
    if args.debug {
        if let Ok(file) = File::create("/home/gregory/Documents/string_driver/rust_driver/run_output.log") {
            debug_file = Some(file);
        }
    }

    // Load ARD_PORT and ARD_NUM_STEPPERS from string_driver.yaml (fail-fast)
    let hostname = gethostname().to_string_lossy().to_string();
    let settings = match config_loader::load_arduino_settings(&hostname) {
        Ok(s) => s,
        Err(e) => panic!("Missing/invalid Arduino settings in YAML for host '{}': {}", hostname, e),
    };

    // Calculate default x_finish: X_MAX_POS - 100
    let default_x_finish = if let Some(max_pos) = settings.x_max_pos {
        if max_pos > 0 {
            max_pos - 100
        } else {
            100
        }
    } else {
        100
    };
    
    // Use X_MAX_POS for X slider max range
    let x_slider_max: Option<i32> = settings.x_max_pos;
    
    // Load operations settings for z_up_step and z_down_step
    let ops_settings = match config_loader::load_operations_settings(&hostname) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Warning: Could not load operations settings: {}. Using defaults.", e);
            config_loader::OperationsSettings {
                z_up_step: Some(2),
                z_down_step: Some(-2),
                bump_check_enable: false,
                tune_rest: Some(10.0),
                x_rest: Some(10.0),
                z_rest: Some(5.0),
                lap_rest: Some(4.0),
                adjustment_level: Some(4),
                retry_threshold: Some(50),
                delta_threshold: Some(50),
                z_variance_threshold: Some(50),
                x_start: Some(100),
                x_finish: Some(default_x_finish),
                x_step: Some(10),
            }
        }
    };
    let z_up_step = ops_settings.z_up_step.unwrap_or(2);
    let z_down_step = ops_settings.z_down_step.unwrap_or(-2);

    let mainboard_tuner_count = config_loader::mainboard_tuner_indices(&settings).len();
    let tuner_num_for_gui = if settings.ard_t_num_steppers.is_some() {
        settings.ard_t_num_steppers
    } else if mainboard_tuner_count > 0 {
        Some(mainboard_tuner_count)
    } else {
        None
    };

    let mut app = StepperGUI::new(
        settings.port.clone(),
        settings.num_steppers,
        settings.string_num,
        settings.x_step_index,
        settings.z_first_index,
        settings.tuner_first_index,
        settings.ard_t_port.clone(),
        tuner_num_for_gui,
        args.debug,
        debug_file,
        z_up_step,
        z_down_step,
        settings.firmware,
        x_slider_max // Use GPIO_MAX_STEPS for slider range
    );
    
    // Auto-connect on startup (mirror Python's automatic arduino_init)
    app.connect();
    
    // Connect to tuner board if configured
    if settings.tuner_first_index.is_some() {
        app.connect_tuner();
    }
    
    // If connection failed, show error but still launch GUI
    if !app.connected {
        eprintln!("WARNING: Failed to connect to Arduino at {}", settings.port);
    }
    
    // Start Unix socket listener for IPC commands
    // We need to share the app with the listener thread, so we wrap it in Arc<Mutex<>>
    let app_arc = Arc::new(Mutex::new(app));
    StepperGUI::start_socket_listener(Arc::clone(&app_arc));
    
    // Create a wrapper that implements App and locks/unlocks the inner app
    struct AppWrapper {
        app: Arc<Mutex<StepperGUI>>,
    }
    
    impl eframe::App for AppWrapper {
        fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
            if let Ok(mut guard) = self.app.lock() {
                guard.update(ctx, frame);
            }
        }
    }
    
    let wrapper = AppWrapper { app: app_arc };

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([400.0, 800.0]) // Tall narrow window
            .with_position(egui::pos2(0.0, 0.0)), // Left side of screen
        ..Default::default()
    };
    let _ = eframe::run_native(
        "Stepper Control",
        options,
        Box::new(|_cc| Box::new(wrapper))
    );
}