use anyhow::{Result, Context};
use serialport::{SerialPort, DataBits, Parity, StopBits, FlowControl};
use serde_yaml;
use std::time::Duration;

#[derive(Debug)]
struct ArduinoBoard {
    port: Box<dyn SerialPort>,
    num_steppers: usize,
}

impl ArduinoBoard {
    fn new(port_path: &str, baud_rate: u32, num_steppers: usize) -> Result<Self> {
        let port = serialport::new(port_path, baud_rate)
            .data_bits(DataBits::Eight)
            .stop_bits(StopBits::One)
            .parity(Parity::None)
            .flow_control(FlowControl::None)
            .timeout(Duration::from_secs(2))
            .open()
            .with_context(|| format!("Failed to open {} at {} baud", port_path, baud_rate))?;

        std::thread::sleep(Duration::from_millis(2000)); // Arduino reset delay

        Ok(ArduinoBoard { port, num_steppers })
    }

    fn positions(&mut self) -> Result<Vec<i32>> {
        self.port.write_all(b"1;")?;
        
        let mut buffer = vec![0u8; 256];
        let bytes_read = self.port.read(&mut buffer)?;
        buffer.truncate(bytes_read);

        // Simple parsing for 13 positions (26 bytes + framing)
        let mut positions = Vec::new();
        
        // For stringdriver-3, expect 13 positions
        for i in 0..self.num_steppers {
            positions.push(0); // Placeholder - real parsing would decode binary
        }
        
        Ok(positions)
    }

    fn move_stepper(&mut self, stepper: i32, position: i32) -> Result<()> {
        let cmd = format!("2,{},{};", stepper, position);
        self.port.write_all(cmd.as_bytes())?;
        Ok(())
    }

    fn reset_all(&mut self) -> Result<()> {
        self.port.write_all(b"4;")?;
        Ok(())
    }
}

fn load_config() -> Result<(String, u32, usize)> {
    // YAML lives in the rust_driver directory (same as Cargo.toml)
    let yaml_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("string_driver.yaml");
    let yaml_str = std::fs::read_to_string(&yaml_path)?;
    let yaml: serde_yaml::Value = serde_yaml::from_str(&yaml_str)?;
    
    // Use hostname from system or default to stringdriver-3
    let hostname = std::env::var("HOSTNAME")
        .unwrap_or_else(|_| "Ubuntu".to_string());
    
    if let Some(host_block) = yaml.get(&hostname).and_then(|h| h.get("stringdriver-3")) {
        let port = host_block.get("ARD_PORT")
            .and_then(|v| v.as_str())
            .unwrap_or("/dev/ttyUSB0")
            .to_string();
        
        let baud = 115200;
        let steppers = host_block.get("ARD_NUM_STEPPERS")
            .and_then(|v| v.as_u64())
            .unwrap_or(13) as usize;
            
        Ok((port, baud, steppers))
    } else {
        Ok(("/dev/ttyUSB0".to_string(), 115200, 13))
    }
}

fn main() -> Result<()> {
    env_logger::init();
    
    let (port, baud, steppers) = load_config()?;
    println!("Connecting to Arduino on {} at {} baud ({} steppers)", port, baud, steppers);
    
    let mut arduino = ArduinoBoard::new(&port, baud, steppers)?;
    println!("Arduino connected successfully");
    
    let positions = arduino.positions()?;
    println!("Current positions: {:?}", positions);
    
    Ok(())
}


