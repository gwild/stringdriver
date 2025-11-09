use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::File;
use serde_yaml;
use anyhow::{anyhow, Result};
use dotenvy::dotenv;
use std::collections::BTreeMap;
use std::env;
use std::path::PathBuf;

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct HostConfig {
    pub CROSSTALK_MATRIX: Option<Vec<Vec<f32>>>,
    pub DEFAULT_TUNING_HZ: Option<Vec<f32>>,
    // Audio-related keys (match config.yaml names exactly)
    pub AUDIO_INPUT_DEVICE: Option<usize>,
    pub AUDIO_OUTPUT_DEVICE: Option<usize>,
    pub AUDIO_CHANNELS: Option<String>,
    pub AUDIO_INPUT_RATE: Option<f64>,
    pub AUDIO_OUTPUT_RATE: Option<f64>,
    // Presence of this key indicates JACK is used on this host
    pub QJACKCTL_CMD: Option<String>,
    // Explicit backend override ("ALSA" or "JACK")
    pub AUDIO_BACKEND: Option<String>,
    // Explicit ChucK ALSA DAC index (required when AUDIO_BACKEND=ALSA)
    pub CHUCK_DAC_INDEX: Option<usize>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct RaspberryPiConfig {
    #[serde(rename = "stringdriver-1")]
    pub stringdriver_1: Option<HostConfig>,
    #[serde(rename = "stringdriver-2")]
    pub stringdriver_2: Option<HostConfig>,
    #[serde(rename = "stringdriver-3")]
    pub stringdriver_3: Option<HostConfig>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Config {
    pub RaspberryPi: Option<RaspberryPiConfig>,
    #[serde(rename = "Ubuntu")]
    pub ubuntu: Option<HashMap<String, HostConfig>>, // hostname -> HostConfig
    #[serde(rename = "macOS")]
    pub macos: Option<HashMap<String, HostConfig>>,  // hostname -> HostConfig
}

#[derive(Debug, Clone)]
pub struct DbSettings {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub password: String,
    pub database: String,
}

impl DbSettings {
    pub fn from_env() -> Result<Self, anyhow::Error> {
        use gethostname::gethostname;
        // Ensure .env is loaded once here so all env-based config is centralized
        let _ = dotenv();
        
        let hostname = gethostname().to_string_lossy().to_string();
        
        // All hosts default to LAN (192.168.1.84)
        let host = env::var("PG_HOST")
            .or_else(|_| env::var("DB_HOST"))
            .unwrap_or_else(|_| "192.168.1.84".to_string());
            
        let port = env::var("PG_PORT")
            .or_else(|_| env::var("DB_PORT"))
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(5432);
            
        let user = env::var("PG_USER")
            .or_else(|_| env::var("DB_USER"))
            .unwrap_or_else(|_| "GJW".to_string());
            
        let password = env::var("PG_PASSWORD")
            .or_else(|_| env::var("DB_PASSWORD"))?;
            
        let database = env::var("PG_DATABASE")
            .or_else(|_| env::var("DB_NAME"))
            .unwrap_or_else(|_| "String_Driver".to_string());
        
        eprintln!("âââ DB CONFIG LOADED âââ");
        eprintln!("  Hostname: {}", hostname);
        eprintln!("  Connection type: LAN");
        eprintln!("  Target: {}:{}/{}", host, port, database);
        eprintln!("ââââââââââââââââââââââââ");
            
        log::info!(target: "config_loader", "DbSettings: host={}, port={}, user={}, db={} (hostname={})", 
                   host, port, user, database, hostname);
        
        Ok(Self { host, port, user, password, database })
    }
}

pub fn load_config() -> Result<Config, anyhow::Error> {
    // Single source of truth: only rust_driver/audio_streaming.yaml
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("audio_streaming.yaml");
    let file = File::open(&path)
        .map_err(|e| anyhow!("Missing required audio_streaming.yaml at {:?}: {}", path, e))?;
            let config: Config = serde_yaml::from_reader(file)?;
    Ok(config)
}

/// Load full HostConfig for the current hostname from audio_streaming.yaml variants
pub fn host_config_for(hostname: &str) -> Result<HostConfig> {
    let cfg = load_config().map_err(|e| anyhow!("Failed to load audio_streaming.yaml: {}", e))?;
    let mut host_cfg: Option<HostConfig> = None;
    if let Some(rpi) = cfg.RaspberryPi {
        host_cfg = match hostname {
            "stringdriver-1" => rpi.stringdriver_1,
            "stringdriver-2" => rpi.stringdriver_2,
            "stringdriver-3" => rpi.stringdriver_3,
            _ => None,
        };
    }
    if host_cfg.is_none() {
        if let Some(m) = cfg.ubuntu {
            host_cfg = m.get(hostname).cloned();
        }
    }
    if host_cfg.is_none() {
        if let Some(m) = cfg.macos {
            host_cfg = m.get(hostname).cloned();
        }
    }
    host_cfg.ok_or_else(|| anyhow!("No host config found for '{}' in audio_streaming.yaml", hostname))
}

// -------------------- GStreamer/Icecast config --------------------

#[derive(Debug, Deserialize, Serialize, Clone, Default)]
pub struct GstHostConfig {
    pub GSTREAMER_PATH: Option<String>,
    pub ICECAST_MOUNT: Option<String>,
    pub GSTREAMER_AUDIO_SRC: Option<String>,
    pub GSTREAMER_CONVERT: Option<String>,
    pub GSTREAMER_ENCODER: Option<String>,
    pub GSTREAMER_SINK: Option<String>,
    pub GSTREAMER_STREAM_NAME: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone, Default)]
pub struct GstCommonConfig {
    pub ICECAST_BITRATE: Option<i32>,
    pub ICECAST_QUALITY: Option<i32>,
    pub ICECAST_CONTENT_TYPE: Option<String>,
    pub ICECAST_PUBLIC: Option<i32>,
    pub GST_AUDIO_FORMAT: Option<String>,
    pub GST_RESAMPLE_QUALITY: Option<i32>,
    pub GST_QUEUE_BUFFERS: Option<i32>,
    pub GST_QUEUE_TIME: Option<i32>,
    pub GST_QUEUE_BYTES: Option<i32>,
    pub GST_SECOND_QUEUE_ENABLE: Option<bool>,
    pub GST_SECOND_QUEUE_BUFFERS: Option<i32>,
    pub GST_SECOND_QUEUE_TIME: Option<i32>,
    pub GST_SECOND_QUEUE_BYTES: Option<i32>,
    pub SHOUT2SEND_SYNC: Option<bool>,
    pub LAME_TARGET: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct GstConfig {
    #[serde(rename = "macOS")]
    pub macos: Option<BTreeMap<String, GstHostConfig>>, // hostname -> config
    #[serde(rename = "Ubuntu")]
    pub ubuntu: Option<BTreeMap<String, GstHostConfig>>, // hostname -> config
    #[serde(rename = "RaspberryPi")]
    pub rpi: Option<BTreeMap<String, GstHostConfig>>, // hostname -> config
    pub common: Option<GstCommonConfig>,
}

pub fn load_gstreamer_yaml() -> Result<GstConfig> {
    // Single source of truth: only rust_driver/gstreamer.yaml
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("gstreamer.yaml");
    let file = File::open(&path)
        .map_err(|e| anyhow!("Missing required gstreamer.yaml at {:?}: {}", path, e))?;
    let cfg: GstConfig = serde_yaml::from_reader(file)?;
    Ok(cfg)
}

pub fn gstreamer_env_for(hostname: &str) -> Result<BTreeMap<String, String>> {
    let _ = dotenv(); // ensure env loaded for ICECAST_* vars
    let cfg = load_gstreamer_yaml()?;
    let mut host: Option<GstHostConfig> = None;
    if let Some(map) = cfg.ubuntu.as_ref() {
        if let Some(h) = map.get(hostname) { host = Some(h.clone()); }
    }
    if host.is_none() {
        if let Some(map) = cfg.macos.as_ref() {
            if let Some(h) = map.get(hostname) { host = Some(h.clone()); }
        }
    }
    if host.is_none() {
        if let Some(map) = cfg.rpi.as_ref() {
            if let Some(h) = map.get(hostname) { host = Some(h.clone()); }
        }
    }
    let host = host.ok_or_else(|| anyhow!("No host entry for '{}' in gstreamer.yaml", hostname))?;
    let common = cfg.common.unwrap_or_default();

    let mut envs = BTreeMap::new();
    // Host entries
    if let Some(v) = host.GSTREAMER_PATH { envs.insert("GSTREAMER_PATH".into(), v); }
    if let Some(v) = host.ICECAST_MOUNT { envs.insert("ICECAST_MOUNT".into(), v); }
    if let Some(v) = host.GSTREAMER_AUDIO_SRC { envs.insert("GSTREAMER_AUDIO_SRC".into(), v); }
    if let Some(v) = host.GSTREAMER_CONVERT { envs.insert("GSTREAMER_CONVERT".into(), v); }
    if let Some(v) = host.GSTREAMER_ENCODER { envs.insert("GSTREAMER_ENCODER".into(), v); }
    if let Some(v) = host.GSTREAMER_SINK { envs.insert("GSTREAMER_SINK".into(), v); }
    if let Some(v) = host.GSTREAMER_STREAM_NAME { envs.insert("GSTREAMER_STREAM_NAME".into(), v); }

    // Common entries
    if let Some(v) = common.ICECAST_BITRATE { envs.insert("ICECAST_BITRATE".into(), v.to_string()); }
    if let Some(v) = common.ICECAST_QUALITY { envs.insert("ICECAST_QUALITY".into(), v.to_string()); }
    if let Some(v) = common.ICECAST_CONTENT_TYPE { envs.insert("ICECAST_CONTENT_TYPE".into(), v); }
    if let Some(v) = common.ICECAST_PUBLIC { envs.insert("ICECAST_PUBLIC".into(), v.to_string()); }
    if let Some(v) = common.GST_AUDIO_FORMAT { envs.insert("GST_AUDIO_FORMAT".into(), v); }
    if let Some(v) = common.GST_RESAMPLE_QUALITY { envs.insert("GST_RESAMPLE_QUALITY".into(), v.to_string()); }
    if let Some(v) = common.GST_QUEUE_BUFFERS { envs.insert("GST_QUEUE_BUFFERS".into(), v.to_string()); }
    if let Some(v) = common.GST_QUEUE_TIME { envs.insert("GST_QUEUE_TIME".into(), v.to_string()); }
    if let Some(v) = common.GST_QUEUE_BYTES { envs.insert("GST_QUEUE_BYTES".into(), v.to_string()); }
    if let Some(v) = common.GST_SECOND_QUEUE_ENABLE { envs.insert("GST_SECOND_QUEUE_ENABLE".into(), v.to_string()); }
    if let Some(v) = common.GST_SECOND_QUEUE_BUFFERS { envs.insert("GST_SECOND_QUEUE_BUFFERS".into(), v.to_string()); }
    if let Some(v) = common.GST_SECOND_QUEUE_TIME { envs.insert("GST_SECOND_QUEUE_TIME".into(), v.to_string()); }
    if let Some(v) = common.GST_SECOND_QUEUE_BYTES { envs.insert("GST_SECOND_QUEUE_BYTES".into(), v.to_string()); }
    if let Some(v) = common.SHOUT2SEND_SYNC { envs.insert("SHOUT2SEND_SYNC".into(), v.to_string()); }
    if let Some(v) = common.LAME_TARGET { envs.insert("LAME_TARGET".into(), v); }

    // ICECAST creds from environment (fail-fast if missing as per rules)
    let ice_host = env::var("ICECAST_HOST").map_err(|_| anyhow!("Missing ICECAST_HOST in environment"))?;
    let ice_port = env::var("ICECAST_PORT").map_err(|_| anyhow!("Missing ICECAST_PORT in environment"))?;
    let ice_pass = env::var("ICECAST_PASSWORD").map_err(|_| anyhow!("Missing ICECAST_PASSWORD in environment"))?;
    envs.insert("ICECAST_HOST".into(), ice_host);
    envs.insert("ICECAST_PORT".into(), ice_port);
    envs.insert("ICECAST_PASSWORD".into(), ice_pass);

    Ok(envs)
}

pub fn load_host_config(yaml_path: &str, hostname: &str) -> Option<HostConfig> {
    let file = File::open(yaml_path).ok()?;
    let config: Config = serde_yaml::from_reader(file).ok()?;
    if let Some(rpi) = config.RaspberryPi {
        if let Some(cfg) = match hostname {
            "stringdriver-1" => rpi.stringdriver_1,
            "stringdriver-2" => rpi.stringdriver_2,
            "stringdriver-3" => rpi.stringdriver_3,
            _ => None,
        } {
            return Some(cfg);
        }
    }
    if let Some(ubuntu) = config.ubuntu {
        if let Some(cfg) = ubuntu.get(hostname) {
            return Some(cfg.clone());
        }
    }
    if let Some(macos) = config.macos {
        if let Some(cfg) = macos.get(hostname) {
            return Some(cfg.clone());
        }
    }
    None
}

// -------------------- Presets config --------------------

/// Load presets YAML (single source of truth)
pub fn load_presets_yaml() -> Result<BTreeMap<String, serde_yaml::Value>> {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("presets.yaml");
    let file = File::open(&path)
        .map_err(|e| anyhow!("Missing required presets.yaml at {:?}: {}", path, e))?;
    let presets: BTreeMap<String, serde_yaml::Value> = serde_yaml::from_reader(file)?;
    Ok(presets)
}

/// Save presets YAML (single source of truth)
pub fn save_presets_yaml(presets: &BTreeMap<String, serde_yaml::Value>) -> Result<()> {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("presets.yaml");
    let yaml_str = serde_yaml::to_string(presets)?;
    std::fs::write(&path, yaml_str)?;
    Ok(())
}

// -------------------- Crosstalk config --------------------

/// Load crosstalk matrix for a hostname (single source of truth)
pub fn load_crosstalk_matrix(hostname: &str) -> Option<Vec<Vec<f32>>> {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("crosstalk_training_matrices.yaml");
    let file = File::open(&path).ok()?;
    let yaml: serde_yaml::Value = serde_yaml::from_reader(file).ok()?;
    let host_map = yaml.as_mapping()?.get(&serde_yaml::Value::from(hostname.to_string()))?.as_mapping()?;
    let matrix = host_map.get(&serde_yaml::Value::from("CROSSTALK_MATRIX"))?;
    serde_yaml::from_value::<Vec<Vec<f32>>>(matrix.clone()).ok()
}

/// Save crosstalk data for a hostname (single source of truth)
pub fn save_crosstalk_data(hostname: &str, matrix: &[Vec<f32>], signatures: &serde_yaml::Value) -> Result<()> {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("crosstalk_training_matrices.yaml");
    let mut yaml = if let Ok(file) = File::open(&path) {
        serde_yaml::from_reader(file)?
    } else {
        serde_yaml::Value::Mapping(serde_yaml::Mapping::new())
    };
    
    let host_entry = yaml.as_mapping_mut()
        .ok_or_else(|| anyhow!("Invalid YAML structure"))?
        .entry(serde_yaml::Value::from(hostname.to_string()))
        .or_insert_with(|| serde_yaml::Value::from(serde_yaml::Mapping::new()));
    
    let host_map = host_entry.as_mapping_mut()
        .ok_or_else(|| anyhow!("Invalid host entry structure"))?;
    
    let matrix_yaml = serde_yaml::to_value(matrix)?;
    host_map.insert(serde_yaml::Value::from("CROSSTALK_MATRIX"), matrix_yaml);
    host_map.insert(serde_yaml::Value::from("STRING_SIGNATURES"), signatures.clone());
    
    let out = serde_yaml::to_string(&yaml)?;
    std::fs::write(&path, out)?;
    Ok(())
}

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
    // YAML lives in the rust_driver directory (same as Cargo.toml)
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

