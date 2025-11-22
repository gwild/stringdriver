/// Configuration loader for string_driver.yaml
/// 
/// Single source of truth: all configuration comes from string_driver.yaml
/// This module loads Arduino, Operations, and GPIO settings for GUI applications.

use serde_yaml;
use anyhow::{anyhow, Result};
use std::fs::File;
use std::path::PathBuf;
use std::env;
use dotenvy::dotenv;
use gethostname::gethostname;

// -------------------- Arduino (carriage) config --------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArduinoFirmware {
    StringDriverV1,
    StringDriverV2,
}

impl ArduinoFirmware {
    fn from_value(value: Option<&str>) -> Result<Self> {
        match value.unwrap_or("string_driver_v2") {
            "string_driver_v1" => Ok(ArduinoFirmware::StringDriverV1),
            "string_driver_v2" => Ok(ArduinoFirmware::StringDriverV2),
            other => Err(anyhow!("Unknown ARDUINO_FIRMWARE value '{}'", other)),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ArduinoSettings {
    pub port: Option<String>, // None means no Arduino connected
    pub num_steppers: Option<usize>, // None means no Arduino connected
    pub string_num: usize,
    pub x_step_index: Option<usize>, // None means no X stepper
    pub x_max_pos: Option<i32>, // X_MAX_POS from YAML
    pub z_first_index: Option<usize>, // None means no Z steppers
    pub tuner_first_index: Option<usize>, // None means no tuners
    pub ard_t_port: Option<String>, // None means tuners on main board or no tuners
    pub ard_t_num_steppers: Option<usize>, // Number of tuner steppers
    pub firmware: ArduinoFirmware,
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
        .and_then(|v| {
            if v.is_null() {
                None
            } else {
                v.as_str().map(|s| s.to_string())
            }
        });

    let num = host_block.get(&serde_yaml::Value::from("ARD_NUM_STEPPERS"))
        .and_then(|v| {
            if v.is_null() {
                None
            } else {
                v.as_i64().map(|v| v as usize)
            }
        });

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

    let firmware = ArduinoFirmware::from_value(
        host_block
            .get(&serde_yaml::Value::from("ARDUINO_FIRMWARE"))
            .and_then(|v| v.as_str()),
    )?;

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
        firmware,
    })
}

pub fn mainboard_tuner_indices(settings: &ArduinoSettings) -> Vec<usize> {
    if settings.ard_t_port.is_some() {
        return Vec::new();
    }
    let tuner_first = match settings.tuner_first_index {
        Some(idx) => idx,
        None => return Vec::new(),
    };
    let z_first = match settings.z_first_index {
        Some(idx) => idx,
        None => return Vec::new(),
    };
    let mut limit = z_first;
    if let Some(x_idx) = settings.x_step_index {
        if x_idx > tuner_first && x_idx < limit {
            limit = x_idx;
        }
    }
    if limit <= tuner_first {
        return Vec::new();
    }
    (tuner_first..limit).collect()
}

// -------------------- Operations config --------------------

#[derive(Debug, Clone)]
pub struct OperationsSettings {
    pub z_up_step: Option<i32>,
    pub z_down_step: Option<i32>,
    pub bump_check_enable: bool,
    pub tune_rest: Option<f32>,
    pub x_rest: Option<f32>,
    pub z_rest: Option<f32>,
    pub lap_rest: Option<f32>,
    pub adjustment_level: Option<i32>,
    pub retry_threshold: Option<i32>,
    pub delta_threshold: Option<i32>,
    pub z_variance_threshold: Option<i32>,
    pub x_start: Option<i32>,
    pub x_finish: Option<i32>,
    pub x_step: Option<i32>,
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
        .unwrap_or(true);

    let tune_rest = host_block.get(&serde_yaml::Value::from("TUNE_REST"))
        .and_then(|v| v.as_f64())
        .map(|v| v as f32);

    let x_rest = host_block.get(&serde_yaml::Value::from("X_REST"))
        .and_then(|v| v.as_f64())
        .map(|v| v as f32);

    let z_rest = host_block.get(&serde_yaml::Value::from("Z_REST"))
        .and_then(|v| v.as_f64())
        .map(|v| v as f32);

    let lap_rest = host_block.get(&serde_yaml::Value::from("LAP_REST"))
        .and_then(|v| v.as_f64())
        .map(|v| v as f32);

    let adjustment_level = host_block.get(&serde_yaml::Value::from("ADJUSTMENT_LEVEL"))
        .and_then(|v| v.as_i64())
        .map(|v| v as i32);

    let retry_threshold = host_block.get(&serde_yaml::Value::from("RETRY_THRESHOLD"))
        .and_then(|v| v.as_i64())
        .map(|v| v as i32);

    let delta_threshold = host_block.get(&serde_yaml::Value::from("DELTA_THRESHOLD"))
        .and_then(|v| v.as_i64())
        .map(|v| v as i32);

    let z_variance_threshold = host_block.get(&serde_yaml::Value::from("Z_VARIANCE_THRESHOLD"))
        .and_then(|v| v.as_i64())
        .map(|v| v as i32);

    let x_start = host_block.get(&serde_yaml::Value::from("X_START"))
        .and_then(|v| v.as_i64())
        .map(|v| v as i32);

    let x_finish = host_block.get(&serde_yaml::Value::from("X_FINISH"))
        .and_then(|v| v.as_i64())
        .map(|v| v as i32);

    let x_step = host_block.get(&serde_yaml::Value::from("X_STEP"))
        .and_then(|v| v.as_i64())
        .map(|v| v as i32);

    Ok(OperationsSettings {
        z_up_step,
        z_down_step,
        bump_check_enable,
        tune_rest,
        x_rest,
        z_rest,
        lap_rest,
        adjustment_level,
        retry_threshold,
        delta_threshold,
        z_variance_threshold,
        x_start,
        x_finish,
        x_step,
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

// -------------------- Database config --------------------

#[derive(Debug, Clone)]
pub struct DbSettings {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub password: String,
    pub database: String,
}

impl DbSettings {
    pub fn from_env() -> Result<Self> {
        let _ = dotenv();
        let hostname = gethostname().to_string_lossy().to_string();
        let host = env::var("PG_HOST").or_else(|_| env::var("DB_HOST")).unwrap_or_else(|_| "192.168.1.84".to_string());
        let port = env::var("PG_PORT").or_else(|_| env::var("DB_PORT")).ok().and_then(|s| s.parse().ok()).unwrap_or(5432);
        let user = env::var("PG_USER").or_else(|_| env::var("DB_USER")).unwrap_or_else(|_| "GJW".to_string());
        let password = env::var("PG_PASSWORD").or_else(|_| env::var("DB_PASSWORD")).map_err(|_| anyhow!("PG_PASSWORD or DB_PASSWORD environment variable required"))?;
        let database = env::var("PG_DATABASE").or_else(|_| env::var("DB_NAME")).unwrap_or_else(|_| "String_Driver".to_string());
        Ok(Self { host, port, user, password, database })
    }
}
