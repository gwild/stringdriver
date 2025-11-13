/// GPIO Board module - Rust implementation of GPIO_SD.py
/// 
/// Supports libgpiod (gpiod) for GPIO access.
/// Note: gpiozero is Python-specific and not supported in Rust.
/// 
/// Single source of truth: all configuration comes from string_driver.yaml
/// via config_loader::load_gpio_settings() - no hardcoded fallbacks.

use anyhow::{anyhow, Result};
use gethostname::gethostname;
use crate::config_loader::{GpioSettings, GpioComponents};
use std::collections::HashMap;

#[cfg(feature = "gpiod")]
use gpiocdev::chip::Chip;
#[cfg(feature = "gpiod")]
use gpiocdev::line::{Bias, Value};
#[cfg(feature = "gpiod")]
use gpiocdev::request::Request;

/// GPIO Board controller
#[derive(Debug)]
pub struct GpioBoard {
    pub exist: bool,
    pub library: Option<String>,
    pub max_steps: Option<u32>,
    
    // Hardware component placeholders
    pub z_touch_lines: Option<Vec<u32>>,
    pub x_home_line: Option<u32>,
    pub x_away_line: Option<u32>,
    pub x_limit_button: Option<u32>,
    
    // Individual line requests (for gpiod)
    #[cfg(feature = "gpiod")]
    line_requests: HashMap<u32, Request>,
    
    // Encoder tracking (software-based since we don't have hardware encoder support yet)
    encoder_steps: i32,
    
    // Distance sensor tracking
    pub distance_sensor_enabled: bool,
    last_good_distance: u32,
    
    num_touch_pins: usize,
}

impl GpioBoard {
    /// Create a new GPIO board from configuration.
    /// Loads config from string_driver.yaml for the current hostname.
    pub fn new() -> Result<Self> {
        let hostname = gethostname().to_string_lossy().to_string();
        
        // Load GPIO settings from YAML (single source of truth)
        let gpio_settings = crate::config_loader::load_gpio_settings(&hostname)?;
        
        if let Some(settings) = gpio_settings {
            if !settings.enabled {
                return Ok(Self::disabled());
            }
            
            // GPIO is enabled - require library and max_steps (fail-fast per rules)
            let library = settings.library.ok_or_else(|| {
                anyhow!("GPIO_ENABLED is true but GPIO_LIBRARY is missing for hostname '{}'", hostname)
            })?;
            
            let max_steps = settings.max_steps.ok_or_else(|| {
                anyhow!("GPIO_ENABLED is true but GPIO_MAX_STEPS is missing for hostname '{}'", hostname)
            })?;
            
            let components = settings.components.ok_or_else(|| {
                anyhow!("GPIO_ENABLED is true but GPIO_COMPONENTS is missing for hostname '{}'", hostname)
            })?;
            
            // Only support gpiod in Rust (gpiozero is Python-specific)
            if library != "gpiod" {
                return Err(anyhow!(
                    "GPIO_LIBRARY '{}' is not supported in Rust. Only 'gpiod' is supported.",
                    library
                ));
            }
            
            // Initialize gpiod components
            Self::init_gpiod(components, max_steps)
        } else {
            // GPIO not enabled for this host
            Ok(Self::disabled())
        }
    }
    
    /// Create a disabled GPIO board instance
    pub fn disabled() -> Self {
        Self {
            exist: false,
            library: None,
            max_steps: None,
            z_touch_lines: None,
            x_home_line: None,
            x_away_line: None,
            x_limit_button: None,
            #[cfg(feature = "gpiod")]
            line_requests: HashMap::new(),
            encoder_steps: 0,
            distance_sensor_enabled: false,
            last_good_distance: 0,
            num_touch_pins: 0,
        }
    }
    
    /// Initialize GPIO components using libgpiod
    #[cfg(feature = "gpiod")]
    fn init_gpiod(components: GpioComponents, max_steps: u32) -> Result<Self> {
        use gpiocdev::line::{Bias, Value};
        use gpiocdev::request::Request;
        use std::collections::HashMap;
        
        // Find a gpiochip that exposes all required pins
        let chip_path = Self::find_gpio_chip(&components)?;
        
        // Collect all pins
        let mut all_pins = Vec::new();
        
        // Z-Touch sensors
        let z_touch_pins = components.z_touch_pins.clone().unwrap_or_default();
        let num_touch_pins = z_touch_pins.len();
        for pin in &z_touch_pins {
            all_pins.push(*pin);
        }
        
        // X_HOME limit switch
        let x_home_line = components.x_home_pin;
        if let Some(pin) = x_home_line {
            if !all_pins.contains(&pin) {
                all_pins.push(pin);
            }
        }
        
        // X_AWAY limit switch
        let x_away_line = components.x_away_pin;
        if let Some(pin) = x_away_line {
            if !all_pins.contains(&pin) {
                all_pins.push(pin);
            }
        }
        
        // Single-pin ground-sense (X_LIMIT_PIN used for both home and away)
        let (x_home_line, x_away_line, x_limit_button) = if let Some(limit_pin) = components.x_limit_pin {
            if x_home_line.is_none() && x_away_line.is_none() {
                if !all_pins.contains(&limit_pin) {
                    all_pins.push(limit_pin);
                }
                (Some(limit_pin), Some(limit_pin), Some(limit_pin))
            } else {
                (x_home_line, x_away_line, None)
            }
        } else {
            (x_home_line, x_away_line, None)
        };
        
        // Request each line individually using the correct gpiocdev API
        let mut line_requests = HashMap::new();
        
        for offset in &all_pins {
            let request = Request::builder()
                .on_chip(&chip_path)
                .with_consumer("StringDriver")
                .with_line(*offset)
                .as_input()
                .with_bias(Bias::PullUp)
                .request()?;
            
            line_requests.insert(*offset, request);
        }
        
        // Note: Encoder and distance sensor require additional hardware support
        // that would need to be implemented separately (not available in basic gpiod)
        let distance_sensor_enabled = components.distance_sensor_pins.is_some();
        
        Ok(Self {
            exist: true,
            library: Some("gpiod".to_string()),
            max_steps: Some(max_steps),
            z_touch_lines: Some(z_touch_pins),
            x_home_line,
            x_away_line,
            x_limit_button,
            line_requests,
            encoder_steps: 0,
            distance_sensor_enabled,
            last_good_distance: 0,
            num_touch_pins,
        })
    }
    
    #[cfg(not(feature = "gpiod"))]
    fn init_gpiod(_components: GpioComponents, _max_steps: u32) -> Result<Self> {
        Err(anyhow!("GPIO support not compiled in. Enable 'gpiod' feature."))
    }
    
    /// Find a gpiochip that exposes all required pins
    fn find_gpio_chip(components: &GpioComponents) -> Result<String> {
        #[cfg(feature = "gpiod")]
        {
            use std::fs;
            
            let required_pins: Vec<u32> = {
                let mut pins = Vec::new();
                if let Some(ref z_pins) = components.z_touch_pins {
                    pins.extend(z_pins);
                }
                if let Some(pin) = components.x_home_pin {
                    pins.push(pin);
                }
                if let Some(pin) = components.x_away_pin {
                    pins.push(pin);
                }
                if let Some(pin) = components.x_limit_pin {
                    pins.push(pin);
                }
                pins
            };
            
            // Search for gpiochip devices
            let mut chip_paths: Vec<String> = fs::read_dir("/dev")?
                .filter_map(|entry| {
                    let entry = entry.ok()?;
                    let path = entry.path();
                    let name = path.file_name()?.to_str()?;
                    if name.starts_with("gpiochip") {
                        Some(path.to_string_lossy().to_string())
                    } else {
                        None
                    }
                })
                .collect();
            
            chip_paths.sort();
            
            // Try to find a chip that has all required pins
            for chip_path in &chip_paths {
                if let Ok(chip) = Chip::from_path(chip_path) {
                    let mut has_all_pins = true;
                    for pin in &required_pins {
                        // Check if line exists by trying to get line info
                        if chip.line_info(*pin).is_err() {
                            has_all_pins = false;
                            break;
                        }
                    }
                    if has_all_pins || required_pins.is_empty() {
                        return Ok(chip_path.clone());
                    }
                }
            }
            
            // Fallback: return first available chip
            if let Some(first_chip) = chip_paths.first() {
                return Ok(first_chip.clone());
            }
            
            Err(anyhow!("No usable gpiochip device found"))
        }
        
        #[cfg(not(feature = "gpiod"))]
        {
            Err(anyhow!("GPIO support not compiled in"))
        }
    }
    
    /// Check the state of Z-touch sensors
    /// Returns array of bools if button_index is None, single bool if button_index is Some
    pub fn press_check(&self, button_index: Option<usize>) -> Result<Vec<bool>> {
        if !self.exist || self.z_touch_lines.is_none() {
            let num_pins = self.num_touch_pins;
            return Ok(vec![false; num_pins]);
        }
        
        #[cfg(feature = "gpiod")]
        {
            if let Some(ref z_pins) = self.z_touch_lines {
                let mut results = Vec::new();
                
                if let Some(idx) = button_index {
                    if idx < z_pins.len() {
                        let pin = z_pins[idx];
                        if let Some(request) = self.line_requests.get(&pin) {
                            // Touch is TRUE when line is LOW (INACTIVE) - pulled up, active low
                            let value = request.value(pin)?;
                            results.push(value == Value::Inactive);
                        } else {
                            results.push(false);
                        }
                    } else {
                        results.push(false);
                    }
                } else {
                    // Return all Z-touch states
                    for pin in z_pins {
                        if let Some(request) = self.line_requests.get(pin) {
                            let value = request.value(*pin)?;
                            let is_touching = value == Value::Inactive;
                            results.push(is_touching);
                        } else {
                            results.push(false);
                        }
                    }
                }
                
                Ok(results)
            } else {
                Ok(vec![false; self.num_touch_pins])
            }
        }
        
        #[cfg(not(feature = "gpiod"))]
        {
            Ok(vec![false; self.num_touch_pins])
        }
    }
    
    /// Check the X home limit switch
    pub fn x_home_check(&self) -> Result<bool> {
        if !self.exist {
            return Ok(false);
        }
        
        #[cfg(feature = "gpiod")]
        {
            if let Some(pin) = self.x_home_line {
                if let Some(request) = self.line_requests.get(&pin) {
                    let value = request.value(pin)?;
                    // Active low: pressed when line is LOW (0)
                    return Ok(value == Value::Inactive);
                }
            }
        }
        
        Ok(false)
    }
    
    /// Check the X away limit switch
    pub fn x_away_check(&self) -> Result<bool> {
        if !self.exist {
            return Ok(false);
        }
        
        #[cfg(feature = "gpiod")]
        {
            if let Some(pin) = self.x_away_line {
                if let Some(request) = self.line_requests.get(&pin) {
                    let value = request.value(pin)?;
                    // Active low: pressed when line is LOW (0)
                    return Ok(value == Value::Inactive);
                }
            }
        }
        
        Ok(false)
    }
    
    /// Get encoder step count (software tracking)
    /// Note: Real hardware encoder would require additional implementation
    pub fn get_encoder_steps(&self) -> i32 {
        self.encoder_steps * 2
    }
    
    /// Set encoder step count (software tracking)
    pub fn set_encoder_steps(&mut self, steps: i32) {
        self.encoder_steps = steps / 2;
    }
    
    /// Get distance from ultrasonic sensor
    /// Note: This requires additional hardware support beyond basic gpiod
    /// For now, returns a placeholder value
    pub fn get_distance(&self) -> Result<u32> {
        if !self.exist || !self.distance_sensor_enabled {
            return Ok(0);
        }
        
        // TODO: Implement actual distance sensor reading
        // This would require pulse timing or additional hardware abstraction
        // For now, return last known good value
        Ok(self.last_good_distance)
    }
    
    /// Cleanup GPIO resources
    pub fn gpio_quit(&mut self) {
        if !self.exist {
            return;
        }
        
        #[cfg(feature = "gpiod")]
        {
            // Requests are automatically released when dropped
            self.line_requests.clear();
        }
        
        println!("GPIO resources released.");
    }
}

impl Drop for GpioBoard {
    fn drop(&mut self) {
        self.gpio_quit();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_gpio_disabled() {
        let gpio = GpioBoard::disabled();
        assert!(!gpio.exist);
    }
}
