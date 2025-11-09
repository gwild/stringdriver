/// Configuration loader for string_driver.yaml
/// 
/// Single source of truth: all configuration comes from string_driver.yaml
/// This module loads Arduino, Operations, and GPIO settings for GUI applications.

use serde_yaml;
use anyhow::{anyhow, Result};
use std::fs::File;
use std::path::PathBuf;

// -------------------- Arduino (carriage) config --------------------

#[derive(Debug, Clone)]
pub struct ArduinoSettings {
    pub port: String,
    pub num_steppers: usize,
    pub string_num: usize,
    pub x_step_index: Option<usize>, // None means no X stepper
    pub x_max_pos: Option<i32>, // X_MAX_POS from YAML
    pub z_first_index: Option<usize>, // None means no Z steppers
    pub tuner_first_index: Option<usize>, // None means no tuners
    pub ard_t_port: Option<String>, // None means tuners on main board or no tuners
    pub ard_t_num_steppers: Option<usize>, // Number of tuner steppers
}

/// Load ARD_PORT and ARD_NUM_STEPPERS for a given hostname from string_driver.yaml.
/// Fails loudly if required keys are missing.
pub fn load_arduino_settings(hostname: &str) -> Result<ArduinoSettings> {
    let yaml_path: PathBuf = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("string_driver.yaml");
    let file = File::open(&yaml_path)
        .map_err(|e| anyhow!("Missing required string_driver.yaml at {:?}: {}", yaml_path, e))?;
    let yaml: serde_yaml::Value = serde_yaml::from_reader(file)?;

    // Search across known OS sections to find a host block matching hostname
    let mut host_block: Option<&serde_yaml::Mapping> = None;
    for os_key in ["RaspberryPi", "Ubuntu", "macOS"].iter() {
        if let Some(os_map) = yaml.get(*os_key).and_then(|v| v.as_mapping()) {
            for (k, v) in os_map.iter() {
                if k.as_str() == Some(hostname) {
                    host_block = v.as_mapping();
                    break;
                }
            }
        }
        if host_block.is_some() { break; }
    }

    let host_block = host_block.ok_or_else(|| anyhow!("No host entry for '{}' in string_driver.yaml", hostname))?;

    let ard_port = host_block.get(&serde_yaml::Value::from("ARD_PORT"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("ARD_PORT missing for '{}' in string_driver.yaml", hostname))?
        .to_string();

    let num = host_block.get(&serde_yaml::Value::from("ARD_NUM_STEPPERS"))
        .and_then(|v| v.as_i64())
        .ok_or_else(|| anyhow!("ARD_NUM_STEPPERS missing for '{}' in string_driver.yaml", hostname))? as usize;

    let string_num = host_block.get(&serde_yaml::Value::from("STRING_NUM"))
        .and_then(|v| v.as_i64())
        .unwrap_or(0) as usize; // Default to 0 if not specified

    let x_step_index = host_block.get(&serde_yaml::Value::from("X_STEP_INDEX"))
        .and_then(|v| v.as_i64())
        .map(|v| v as usize);

    let x_max_pos = host_block.get(&serde_yaml::Value::from("X_MAX_POS"))
        .and_then(|v| v.as_i64())
        .map(|v| v as i32);

    let z_first_index = host_block.get(&serde_yaml::Value::from("Z_FIRST_INDEX"))
        .and_then(|v| v.as_i64())
        .map(|v| v as usize);

    let tuner_first_index = host_block.get(&serde_yaml::Value::from("TUNER_FIRST_INDEX"))
        .and_then(|v| {
            if v.is_null() {
                None
            } else {
                v.as_i64().map(|v| v as usize)
            }
        });

    let ard_t_port = host_block.get(&serde_yaml::Value::from("ARD_T_PORT"))
        .and_then(|v| {
            if v.is_null() {
                None
            } else {
                v.as_str().map(|s| s.to_string())
            }
        });

    let ard_t_num_steppers = host_block.get(&serde_yaml::Value::from("ARD_T_NUM_STEPPERS"))
        .and_then(|v| v.as_i64())
        .map(|v| v as usize);

    Ok(ArduinoSettings {
        port: ard_port,
        num_steppers: num,
        string_num,
        x_step_index,
        x_max_pos,
        z_first_index,
        tuner_first_index,
        ard_t_port,
        ard_t_num_steppers,
    })
}

// -------------------- Operations config --------------------

#[derive(Debug, Clone)]
pub struct OperationsSettings {
    pub z_up_step: Option<i32>,
    pub z_down_step: Option<i32>,
    pub bump_check_enable: bool,
    pub bump_check_repeat: u32,
    pub bump_disable_threshold: i32,
}

/// Load operations settings for a given hostname from string_driver.yaml.
/// Fails loudly if required keys are missing.
pub fn load_operations_settings(hostname: &str) -> Result<OperationsSettings> {
    let yaml_path: PathBuf = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("string_driver.yaml");
    let file = File::open(&yaml_path)
        .map_err(|e| anyhow!("Missing required string_driver.yaml at {:?}: {}", yaml_path, e))?;
    let yaml: serde_yaml::Value = serde_yaml::from_reader(file)?;

    // Search across known OS sections to find a host block matching hostname
    let mut host_block: Option<&serde_yaml::Mapping> = None;
    for os_key in ["RaspberryPi", "Ubuntu", "macOS"].iter() {
        if let Some(os_map) = yaml.get(*os_key).and_then(|v| v.as_mapping()) {
            for (k, v) in os_map.iter() {
                if k.as_str() == Some(hostname) {
                    host_block = v.as_mapping();
                    break;
                }
            }
        }
        if host_block.is_some() { break; }
    }

    let host_block = host_block.ok_or_else(|| anyhow!("No host entry for '{}' in string_driver.yaml", hostname))?;

    let z_up_step = host_block.get(&serde_yaml::Value::from("Z_UP_STEP"))
        .and_then(|v| v.as_i64())
        .map(|v| v as i32);

    let z_down_step = host_block.get(&serde_yaml::Value::from("Z_DOWN_STEP"))
        .and_then(|v| v.as_i64())
        .map(|v| v as i32);

    let bump_check_enable = host_block.get(&serde_yaml::Value::from("BUMP_CHECK_ENABLE"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let bump_check_repeat = host_block.get(&serde_yaml::Value::from("BUMP_CHECK_REPEAT"))
        .and_then(|v| v.as_i64())
        .map(|v| v as u32)
        .unwrap_or(10);

    let bump_disable_threshold = host_block.get(&serde_yaml::Value::from("BUMP_DISABLE_THRESHOLD"))
        .and_then(|v| v.as_i64())
        .map(|v| v as i32)
        .unwrap_or(3);

    Ok(OperationsSettings {
        z_up_step,
        z_down_step,
        bump_check_enable,
        bump_check_repeat,
        bump_disable_threshold,
    })
}

// -------------------- GPIO config --------------------

#[derive(Debug, Clone)]
pub struct GpioComponents {
    pub z_touch_pins: Option<Vec<u32>>,
    pub x_home_pin: Option<u32>,
    pub x_away_pin: Option<u32>,
    pub x_limit_pin: Option<u32>,
    pub rotary_encoder_pins: Option<RotaryEncoderPins>,
    pub distance_sensor_pins: Option<DistanceSensorPins>,
}

#[derive(Debug, Clone)]
pub struct RotaryEncoderPins {
    pub a: u32,
    pub b: u32,
}

#[derive(Debug, Clone)]
pub struct DistanceSensorPins {
    pub trig: u32,
    pub echo: u32,
}

#[derive(Debug, Clone)]
pub struct GpioSettings {
    pub enabled: bool,
    pub library: Option<String>,
    pub max_steps: Option<u32>,
    pub components: Option<GpioComponents>,
}

/// Load GPIO configuration for a given hostname from string_driver.yaml.
/// Returns None if GPIO_ENABLED is false or not present.
/// Fails loudly if GPIO_ENABLED is true but required keys are missing.
pub fn load_gpio_settings(hostname: &str) -> Result<Option<GpioSettings>> {
    let yaml_path: PathBuf = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("string_driver.yaml");
    let file = File::open(&yaml_path)
        .map_err(|e| anyhow!("Missing required string_driver.yaml at {:?}: {}", yaml_path, e))?;
    let yaml: serde_yaml::Value = serde_yaml::from_reader(file)?;

    // Search across known OS sections to find a host block matching hostname
    let mut host_block: Option<&serde_yaml::Mapping> = None;
    for os_key in ["RaspberryPi", "Ubuntu", "macOS"].iter() {
        if let Some(os_map) = yaml.get(*os_key).and_then(|v| v.as_mapping()) {
            for (k, v) in os_map.iter() {
                if k.as_str() == Some(hostname) {
                    host_block = v.as_mapping();
                    break;
                }
            }
        }
        if host_block.is_some() { break; }
    }

    let host_block = host_block.ok_or_else(|| anyhow!("No host entry for '{}' in string_driver.yaml", hostname))?;

    // Check if GPIO is enabled
    let gpio_enabled = host_block.get(&serde_yaml::Value::from("GPIO_ENABLED"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    if !gpio_enabled {
        return Ok(None);
    }

    // GPIO is enabled, so we require GPIO_LIBRARY and GPIO_MAX_STEPS
    let library = host_block.get(&serde_yaml::Value::from("GPIO_LIBRARY"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let max_steps = host_block.get(&serde_yaml::Value::from("GPIO_MAX_STEPS"))
        .and_then(|v| v.as_i64())
        .map(|v| v as u32);

    // Parse GPIO_COMPONENTS
    let components = host_block.get(&serde_yaml::Value::from("GPIO_COMPONENTS"))
        .and_then(|v| v.as_mapping())
        .map(|comp_map| {
            let z_touch_pins = comp_map.get(&serde_yaml::Value::from("Z_TOUCH_PINS"))
                .and_then(|v| v.as_sequence())
                .map(|seq| seq.iter().filter_map(|v| v.as_i64().map(|n| n as u32)).collect());

            let x_home_pin = comp_map.get(&serde_yaml::Value::from("X_HOME_PIN"))
                .and_then(|v| v.as_i64())
                .map(|n| n as u32);

            let x_away_pin = comp_map.get(&serde_yaml::Value::from("X_AWAY_PIN"))
                .and_then(|v| v.as_i64())
                .map(|n| n as u32);

            let x_limit_pin = comp_map.get(&serde_yaml::Value::from("X_LIMIT_PIN"))
                .and_then(|v| v.as_i64())
                .map(|n| n as u32);

            let rotary_encoder_pins = comp_map.get(&serde_yaml::Value::from("ROTARY_ENCODER_PINS"))
                .and_then(|v| v.as_mapping())
                .and_then(|m| {
                    let a = m.get(&serde_yaml::Value::from("A"))?.as_i64()? as u32;
                    let b = m.get(&serde_yaml::Value::from("B"))?.as_i64()? as u32;
                    Some(RotaryEncoderPins { a, b })
                });

            let distance_sensor_pins = comp_map.get(&serde_yaml::Value::from("DISTANCE_SENSOR_PINS"))
                .and_then(|v| v.as_mapping())
                .and_then(|m| {
                    let trig = m.get(&serde_yaml::Value::from("TRIG"))?.as_i64()? as u32;
                    let echo = m.get(&serde_yaml::Value::from("ECHO"))?.as_i64()? as u32;
                    Some(DistanceSensorPins { trig, echo })
                });

            GpioComponents {
                z_touch_pins,
                x_home_pin,
                x_away_pin,
                x_limit_pin,
                rotary_encoder_pins,
                distance_sensor_pins,
            }
        });

    // If GPIO is enabled, require GPIO_LIBRARY and GPIO_MAX_STEPS (fail-fast per rules)
    if library.is_none() {
        return Err(anyhow!("GPIO_ENABLED is true but GPIO_LIBRARY is missing for '{}' in string_driver.yaml", hostname));
    }

    if max_steps.is_none() {
        return Err(anyhow!("GPIO_ENABLED is true but GPIO_MAX_STEPS is missing for '{}' in string_driver.yaml", hostname));
    }

    Ok(Some(GpioSettings {
        enabled: true,
        library,
        max_steps,
        components,
    }))
}
