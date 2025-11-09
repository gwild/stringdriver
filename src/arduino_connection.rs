/// Shared Arduino connection manager
/// 
/// This module provides a single shared connection to the Arduino that can be
/// used by multiple processes. Since Arduino resets on every new connection,
/// we use a Unix socket IPC mechanism where one process (stepper_gui) owns
/// the connection and other processes (operations_gui) communicate with it.

use serialport;
use std::io::{Read, Write};
use std::time::Duration;
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::os::unix::net::{UnixListener, UnixStream};
use anyhow::{Result, anyhow};
use serde_json;

/// Command types for IPC communication
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum ArduinoCommand {
    RelMove { stepper: usize, delta: i32 },
    AbsMove { stepper: usize, position: i32 },
    Reset { stepper: usize, position: i32 },
}

/// Response from Arduino operations
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum ArduinoResponse {
    Ok,
    Error(String),
}

/// Unix socket path for IPC (fixed path so both processes can find it)
/// Uses port path to create unique socket per Arduino port
fn get_socket_path(port_path: &str) -> String {
    // Create a stable identifier from port path
    let port_id = port_path.replace("/", "_").replace("\\", "_");
    format!("/tmp/arduino_connection_{}.sock", port_id)
}

/// Arduino connection manager that handles a single shared connection
#[derive(Debug)]
pub struct ArduinoConnectionManager {
    port: Option<Box<dyn serialport::SerialPort>>,
    port_path: String,
    connected: bool,
}

impl ArduinoConnectionManager {
    pub fn new(port_path: String) -> Self {
        Self {
            port: None,
            port_path,
            connected: false,
        }
    }
    
    pub fn connect(&mut self) -> Result<()> {
        let port_path = self.port_path.clone();
        self.kill_port_users(&port_path);
        match serialport::new(self.port_path.as_str(), 115200)
            .timeout(Duration::from_secs(2))
            .open() {
            Ok(port) => {
                std::thread::sleep(Duration::from_millis(2000)); // Arduino reset delay
                self.port = Some(port);
                self.connected = true;
                Ok(())
            }
            Err(e) => {
                Err(anyhow!("Connection failed: {}", e))
            }
        }
    }
    
    fn kill_port_users(&mut self, port_path: &str) {
        let output = Command::new("/usr/bin/lsof")
            .arg("-t")
            .arg(port_path)
            .output();
        let Ok(out) = output else { return; };
        if !out.status.success() { return; };
        let pids_str = String::from_utf8_lossy(&out.stdout);
        let self_pid = std::process::id();
        for line in pids_str.lines() {
            if let Ok(pid) = line.trim().parse::<u32>() {
                if pid == self_pid { continue; }
                let _ = Command::new("kill").arg("-9").arg(pid.to_string()).output();
            }
        }
    }
    
    fn pack_i16_le(v: i16) -> [u8; 2] {
        i16::to_le_bytes(v)
    }
    
    fn pack_i32_le(v: i32) -> [u8; 4] {
        i32::to_le_bytes(v)
    }
    
    fn escape_cmdmessenger_bytes(bytes: &[u8]) -> Vec<u8> {
        let mut escaped = Vec::new();
        for &b in bytes {
            if b == b',' || b == b';' || b == b'0' || b == b'1' || b == b'2' || b == b'3' || b == b'4' || b == b'5' || b == b'6' || b == b'7' || b == b'8' || b == b'9' {
                escaped.push(0x47); // ESC
                escaped.push(b ^ 0x20);
            } else {
                escaped.push(b);
            }
        }
        escaped
    }
    
    fn send_cmd_bin(&mut self, cmd_id: u8, stepper_idx: i16, value: i32) -> Result<()> {
        if self.port.is_none() {
            return Err(anyhow!("Port not connected"));
        }
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
        if let Some(p) = self.port.as_mut() {
            p.write_all(&buf)?;
            p.flush()?;
        }
        Ok(())
    }
    
    pub fn rel_move(&mut self, stepper: usize, delta: i32) -> Result<()> {
        if !self.connected {
            return Err(anyhow!("Arduino not connected"));
        }
        if let Some(p) = self.port.as_mut() {
            let _ = p.clear(serialport::ClearBuffer::Input);
        }
        let s = stepper as i16;
        self.send_cmd_bin(3, s, delta)?; // rmove is command ID 3
        std::thread::sleep(Duration::from_millis(500)); // Wait for move completion
        Ok(())
    }
    
    pub fn abs_move(&mut self, stepper: usize, position: i32) -> Result<()> {
        if !self.connected {
            return Err(anyhow!("Arduino not connected"));
        }
        if let Some(p) = self.port.as_mut() {
            let _ = p.clear(serialport::ClearBuffer::Input);
        }
        let s = stepper as i16;
        self.send_cmd_bin(2, s, position)?; // amove is command ID 2
        std::thread::sleep(Duration::from_millis(500)); // Wait for move completion
        Ok(())
    }
    
    pub fn reset(&mut self, stepper: usize, position: i32) -> Result<()> {
        if !self.connected {
            return Err(anyhow!("Arduino not connected"));
        }
        if let Some(p) = self.port.as_mut() {
            let _ = p.clear(serialport::ClearBuffer::Input);
        }
        let s = stepper as i16;
        self.send_cmd_bin(4, s, position)?; // set_stepper is command ID 4
        std::thread::sleep(Duration::from_millis(100));
        Ok(())
    }
    
    
    pub fn is_connected(&self) -> bool {
        self.connected
    }
    
    /// Read positions from Arduino (stepper_gui needs this)
    pub fn read_positions(&mut self, num_steppers: usize) -> Result<Vec<i32>> {
        if !self.connected {
            return Err(anyhow!("Arduino not connected"));
        }
        
        let port = self.port.as_mut().ok_or_else(|| anyhow!("Port not available"))?;
        let send = b"1;";
        
        // Flush input buffer before command
        let _ = port.clear(serialport::ClearBuffer::Input);
        port.write_all(send)?;
        port.flush()?;
        
        // Wait for Arduino to send positions
        std::thread::sleep(Duration::from_millis(50));
        
        // Read response
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
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(e) => {
                    let err_str = e.to_string();
                    if err_str.contains("timeout") || err_str.contains("TimedOut") {
                        std::thread::sleep(Duration::from_millis(10));
                        continue;
                    }
                    return Err(anyhow!("Read error: {}", e));
                }
            }
        }
        
        if buffer.is_empty() || !buffer.iter().any(|&b| b == b';') {
            return Err(anyhow!("Failed to read positions from Arduino"));
        }
        
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
        
        let expected_bytes = num_steppers * 2;
        if data_bytes.len() < expected_bytes {
            return Err(anyhow!("Expected at least {} bytes, got {}", expected_bytes, data_bytes.len()));
        }
        
        let mut positions = vec![0i32; num_steppers];
        for idx in 0..num_steppers {
            let lo = idx * 2;
            let hi = lo + 1;
            if hi < data_bytes.len() {
                positions[idx] = i16::from_le_bytes([data_bytes[lo], data_bytes[hi]]) as i32;
            }
        }
        
        Ok(positions)
    }
    
    /// Start IPC server to handle commands from other processes
    pub fn start_ipc_server(manager: Arc<Mutex<ArduinoConnectionManager>>) -> Result<()> {
        let port_path = manager.lock().unwrap().port_path.clone();
        let socket_path = get_socket_path(&port_path);
        // Remove old socket if it exists
        let _ = std::fs::remove_file(&socket_path);
        
        let listener = UnixListener::bind(&socket_path)?;
        
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                match stream {
                    Ok(mut stream) => {
                        let manager_clone = Arc::clone(&manager);
                        std::thread::spawn(move || {
                            let mut buf = vec![0u8; 1024];
                            if let Ok(len) = stream.read(&mut buf) {
                                if let Ok(cmd) = serde_json::from_slice::<ArduinoCommand>(&buf[..len]) {
                                    let response = {
                                        let mut mgr = manager_clone.lock().unwrap();
                                        match cmd {
                                            ArduinoCommand::RelMove { stepper, delta } => {
                                                match mgr.rel_move(stepper, delta) {
                                                    Ok(_) => ArduinoResponse::Ok,
                                                    Err(e) => ArduinoResponse::Error(e.to_string()),
                                                }
                                            }
                                            ArduinoCommand::AbsMove { stepper, position } => {
                                                match mgr.abs_move(stepper, position) {
                                                    Ok(_) => ArduinoResponse::Ok,
                                                    Err(e) => ArduinoResponse::Error(e.to_string()),
                                                }
                                            }
                                            ArduinoCommand::Reset { stepper, position } => {
                                                match mgr.reset(stepper, position) {
                                                    Ok(_) => ArduinoResponse::Ok,
                                                    Err(e) => ArduinoResponse::Error(e.to_string()),
                                                }
                                            }
                                        }
                                    };
                                    let response_bytes = serde_json::to_vec(&response).unwrap_or_default();
                                    let _ = stream.write_all(&response_bytes);
                                    let _ = stream.flush();
                                }
                            }
                        });
                    }
                    Err(_) => break,
                }
            }
        });
        
        Ok(())
    }
}

/// Client for communicating with the shared Arduino connection
pub struct ArduinoConnectionClient {
    socket_path: String,
}

impl ArduinoConnectionClient {
    pub fn new(port_path: &str) -> Self {
        Self {
            socket_path: get_socket_path(port_path),
        }
    }
    
    fn send_command(&self, cmd: ArduinoCommand) -> Result<ArduinoResponse> {
        let mut stream = UnixStream::connect(&self.socket_path)?;
        let cmd_bytes = serde_json::to_vec(&cmd)?;
        stream.write_all(&cmd_bytes)?;
        stream.flush()?;
        
        let mut buf = vec![0u8; 1024];
        let len = stream.read(&mut buf)?;
        let response: ArduinoResponse = serde_json::from_slice(&buf[..len])?;
        Ok(response)
    }
    
    pub fn rel_move(&self, stepper: usize, delta: i32) -> Result<()> {
        match self.send_command(ArduinoCommand::RelMove { stepper, delta })? {
            ArduinoResponse::Ok => Ok(()),
            ArduinoResponse::Error(e) => Err(anyhow!("{}", e)),
        }
    }
    
    pub fn abs_move(&self, stepper: usize, position: i32) -> Result<()> {
        match self.send_command(ArduinoCommand::AbsMove { stepper, position })? {
            ArduinoResponse::Ok => Ok(()),
            ArduinoResponse::Error(e) => Err(anyhow!("{}", e)),
        }
    }
    
    pub fn reset(&self, stepper: usize, position: i32) -> Result<()> {
        match self.send_command(ArduinoCommand::Reset { stepper, position })? {
            ArduinoResponse::Ok => Ok(()),
            ArduinoResponse::Error(e) => Err(anyhow!("{}", e)),
        }
    }
}

/// Global connection manager instance (singleton within a process)
static CONNECTION_MANAGER: Mutex<Option<Arc<Mutex<ArduinoConnectionManager>>>> = Mutex::new(None);

/// Get or create the shared connection manager (for stepper_gui - owns the connection)
pub fn get_connection_manager(port_path: String) -> Result<Arc<Mutex<ArduinoConnectionManager>>> {
    let mut manager = CONNECTION_MANAGER.lock().unwrap();
    if manager.is_none() {
        let mut conn = ArduinoConnectionManager::new(port_path.clone());
        conn.connect()?;
        let arc_conn = Arc::new(Mutex::new(conn));
        ArduinoConnectionManager::start_ipc_server(Arc::clone(&arc_conn))?;
        *manager = Some(arc_conn);
    }
    Ok(manager.as_ref().unwrap().clone())
}

/// Check if connection manager exists
pub fn has_connection_manager() -> bool {
    CONNECTION_MANAGER.lock().unwrap().is_some()
}

