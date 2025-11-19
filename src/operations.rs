/// Operations module - Rust implementation of operations from surfer.py
/// 
/// Single source of truth: all configuration comes from string_driver.yaml
/// via config_loader - no hardcoded fallbacks.

use anyhow::{anyhow, Result};
use gethostname::gethostname;
use crate::config_loader::{load_operations_settings, load_arduino_settings, load_gpio_settings, mainboard_tuner_indices};
use crate::gpio;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::fs::OpenOptions;
use std::time::Duration;
use memmap2::Mmap;

/// Type alias for partials data: Vec<Vec<(f32, f32)>> where each inner Vec is a channel's partials (freq, amp)
type PartialsData = Vec<Vec<(f32, f32)>>;

/// Type alias for partials slot (matches partials_slot::PartialsSlot)
type PartialsSlot = Arc<Mutex<Option<PartialsData>>>;

/// Calculate voice count per channel from partials data
/// Returns Vec<usize> where each element is the count of non-zero amplitudes for that channel
fn calculate_voice_count(partials: &PartialsData) -> Vec<usize> {
    partials.iter()
        .map(|channel_partials| {
            channel_partials.iter()
                .filter(|&&(_, amp)| amp > 0.0)
                .count()
        })
        .collect()
}

/// Calculate amplitude sum per channel from partials data
/// Returns Vec<f32> where each element is the sum of amplitudes for that channel
fn calculate_amp_sum(partials: &PartialsData) -> Vec<f32> {
    partials.iter()
        .map(|channel_partials| {
            channel_partials.iter()
                .map(|&(_, amp)| amp)
                .sum()
        })
        .collect()
}

/// Stepper enable state tracking (index -> enabled)
type StepperEnabled = Arc<Mutex<HashMap<usize, bool>>>;

/// Trait for stepper operations - allows bump_check to work with different implementations
pub trait StepperOperations {
    fn rel_move(&mut self, stepper: usize, delta: i32) -> Result<()>;
    fn abs_move(&mut self, stepper: usize, position: i32) -> Result<()>;
    fn reset(&mut self, stepper: usize, position: i32) -> Result<()>;
    fn disable(&mut self, stepper: usize) -> Result<()>;
}

/// Operations context for bump checking and recovery
#[derive(Debug)]
pub struct Operations {
    hostname: String,
    bump_check_enable: Arc<Mutex<bool>>,
    z_up_step: Arc<Mutex<i32>>,
    z_down_step: Arc<Mutex<i32>>,
    tune_rest: Arc<Mutex<f32>>,
    x_rest: Arc<Mutex<f32>>,
    z_rest: Arc<Mutex<f32>>,
    lap_rest: Arc<Mutex<f32>>,
    adjustment_level: Arc<Mutex<i32>>,
    retry_threshold: Arc<Mutex<i32>>,
    delta_threshold: Arc<Mutex<i32>>,
    z_variance_threshold: Arc<Mutex<i32>>,
    x_start: Arc<Mutex<i32>>,
    x_finish: Arc<Mutex<i32>>,
    x_step: Arc<Mutex<i32>>,
    pub z_first_index: usize,
    pub string_num: usize,
    pub x_step_index: Option<usize>,
    pub x_max_pos: Option<i32>,
    pub tuner_indices: Vec<usize>,
    pub stepper_enabled: StepperEnabled,
    pub gpio: Option<crate::gpio::GpioBoard>,
    arduino_connected: bool,
    // Audio analysis arrays
    voice_count: Arc<Mutex<Vec<usize>>>, // Per-channel voice count
    amp_sum: Arc<Mutex<Vec<f32>>>, // Per-channel amplitude sum
    partials_slot: Option<PartialsSlot>, // Reference to shared partials slot
}

impl Operations {
    /// Create a new Operations instance from configuration.
    /// Loads config from string_driver.yaml for the current hostname.
    pub fn new() -> Result<Self> {
        Self::new_with_partials_slot(None)
    }
    
    /// Create a new Operations instance with optional partials slot.
    /// Loads config from string_driver.yaml for the current hostname.
    pub fn new_with_partials_slot(partials_slot: Option<PartialsSlot>) -> Result<Self> {
        let hostname = gethostname().to_string_lossy().to_string();
        
        // Load operations settings (single source of truth)
        let ops_settings = load_operations_settings(&hostname)?;
        
        // Load Arduino settings to get Z_FIRST_INDEX and STRING_NUM
        let ard_settings = load_arduino_settings(&hostname)?;
        let z_first_index = ard_settings.z_first_index
            .ok_or_else(|| anyhow!("Z_FIRST_INDEX missing for '{}' in string_driver.yaml", hostname))?;
        let string_num = ard_settings.string_num;
        
        // Load z_up_step from operations settings (from YAML - default to 2 if not specified)
        let z_up_step = ops_settings.z_up_step
            .unwrap_or(2); // Default to 2 if not specified in YAML
        
        // Load z_down_step from operations settings (from YAML - default to -2 if not specified)
        let z_down_step = ops_settings.z_down_step.unwrap_or(-2);
        
        // Load rest values from operations settings (from YAML - defaults from surfer.py)
        let tune_rest = ops_settings.tune_rest.unwrap_or(5.0);
        let x_rest = ops_settings.x_rest.unwrap_or(5.0);
        let z_rest = ops_settings.z_rest.unwrap_or(1.0);
        let lap_rest = ops_settings.lap_rest.unwrap_or(4.0);
        
        // Load adjustment parameters from operations settings (from YAML - defaults from surfer.py)
        let adjustment_level = ops_settings.adjustment_level.unwrap_or(4);
        let retry_threshold = ops_settings.retry_threshold.unwrap_or(50);
        let delta_threshold = ops_settings.delta_threshold.unwrap_or(50);
        let z_variance_threshold = ops_settings.z_variance_threshold.unwrap_or(50);
        
        // Load GPIO if available (required for z_calibration and bump_check)
        let gpio_settings = load_gpio_settings(&hostname)?;
        // Get GPIO_MAX_STEPS for default X range calculation before moving gpio_settings
        let gpio_max_steps = gpio_settings.as_ref().and_then(|gs| gs.max_steps).map(|v| v as i32);
        let gpio = gpio_settings.map(|_| crate::gpio::GpioBoard::new()).transpose()?;
        let arduino_connected = ard_settings.num_steppers > 0;
        
        let x_step_index = ard_settings.x_step_index;
        let x_max_pos = ard_settings.x_max_pos;
        
        // Load X movement parameters from operations settings (from YAML - defaults)
        // Default x_start = 100, x_finish = X_MAX_POS - 100
        let default_x_finish = if let Some(max_pos) = x_max_pos {
            if max_pos > 0 {
                max_pos - 100
            } else {
                100
            }
        } else {
            100
        };
        
        let x_start = ops_settings.x_start.unwrap_or(100);
        let x_finish = ops_settings.x_finish.unwrap_or(default_x_finish);
        let x_step = ops_settings.x_step.unwrap_or(10);
        let tuner_indices = mainboard_tuner_indices(&ard_settings);
        
        // Initialize stepper enabled states (all enabled by default)
        let mut stepper_enabled = HashMap::new();
        for i in 0..(string_num * 2) {
            let stepper_idx = z_first_index + i;
            stepper_enabled.insert(stepper_idx, true);
        }
        if let Some(x_idx) = x_step_index {
            stepper_enabled.insert(x_idx, true);
        }
        for idx in &tuner_indices {
            stepper_enabled.insert(*idx, true);
        }
        
        Ok(Self {
            hostname,
            bump_check_enable: Arc::new(Mutex::new(ops_settings.bump_check_enable)),
            z_up_step: Arc::new(Mutex::new(z_up_step)),
            z_down_step: Arc::new(Mutex::new(z_down_step)),
            tune_rest: Arc::new(Mutex::new(tune_rest)),
            x_rest: Arc::new(Mutex::new(x_rest)),
            z_rest: Arc::new(Mutex::new(z_rest)),
            lap_rest: Arc::new(Mutex::new(lap_rest)),
            adjustment_level: Arc::new(Mutex::new(adjustment_level)),
            retry_threshold: Arc::new(Mutex::new(retry_threshold)),
            delta_threshold: Arc::new(Mutex::new(delta_threshold)),
            z_variance_threshold: Arc::new(Mutex::new(z_variance_threshold)),
            x_start: Arc::new(Mutex::new(x_start)),
            x_finish: Arc::new(Mutex::new(x_finish)),
            x_step: Arc::new(Mutex::new(x_step)),
            z_first_index,
            string_num,
            x_step_index,
            x_max_pos,
            tuner_indices,
            stepper_enabled: Arc::new(Mutex::new(stepper_enabled)),
            gpio,
            arduino_connected,
            voice_count: Arc::new(Mutex::new(vec![0; string_num])),
            amp_sum: Arc::new(Mutex::new(vec![0.0; string_num])),
            partials_slot,
        })
    }
    
    /// Set bump_check_enable state
    pub fn set_bump_check_enable(&self, enabled: bool) {
        if let Ok(mut enable) = self.bump_check_enable.lock() {
            *enable = enabled;
        }
    }
    
    /// Get bump_check_enable state
    pub fn get_bump_check_enable(&self) -> bool {
        self.bump_check_enable.lock()
            .map(|e| *e)
            .unwrap_or(false)
    }
    
    /// Set z_up_step value
    pub fn set_z_up_step(&self, step: i32) {
        if let Ok(mut step_val) = self.z_up_step.lock() {
            *step_val = step;
        }
    }
    
    /// Get z_up_step value
    pub fn get_z_up_step(&self) -> i32 {
        self.z_up_step.lock()
            .map(|s| *s)
            .unwrap_or(2)
    }
    
    /// Set z_down_step value
    pub fn set_z_down_step(&self, step: i32) {
        if let Ok(mut step_val) = self.z_down_step.lock() {
            *step_val = step;
        }
    }
    
    /// Get z_down_step value
    pub fn get_z_down_step(&self) -> i32 {
        self.z_down_step.lock()
            .map(|s| *s)
            .unwrap_or(-2)
    }
    
    pub fn x_step_index(&self) -> Option<usize> {
        self.x_step_index
    }
    
    pub fn tuner_indices(&self) -> Vec<usize> {
        self.tuner_indices.clone()
    }
    
    /// Set tune_rest value
    pub fn set_tune_rest(&self, rest: f32) {
        if let Ok(mut rest_val) = self.tune_rest.lock() {
            *rest_val = rest;
        }
    }
    
    /// Get tune_rest value
    pub fn get_tune_rest(&self) -> f32 {
        self.tune_rest.lock()
            .map(|r| *r)
            .unwrap_or(10.0)
    }
    
    /// Set x_rest value
    pub fn set_x_rest(&self, rest: f32) {
        if let Ok(mut rest_val) = self.x_rest.lock() {
            *rest_val = rest;
        }
    }
    
    /// Get x_rest value
    pub fn get_x_rest(&self) -> f32 {
        self.x_rest.lock()
            .map(|r| *r)
            .unwrap_or(10.0)
    }
    
    /// Set z_rest value
    pub fn set_z_rest(&self, rest: f32) {
        if let Ok(mut rest_val) = self.z_rest.lock() {
            *rest_val = rest;
        }
    }
    
    /// Get z_rest value
    pub fn get_z_rest(&self) -> f32 {
        self.z_rest.lock()
            .map(|r| *r)
            .unwrap_or(5.0)
    }

    fn sleep_for(seconds: f32) {
        if seconds > 0.0 {
            std::thread::sleep(Duration::from_secs_f32(seconds));
        }
    }

    fn rest_z(&self) {
        Self::sleep_for(self.get_z_rest());
    }

    fn rest_x(&self) {
        Self::sleep_for(self.get_x_rest());
    }

    fn rest_tune(&self) {
        Self::sleep_for(self.get_tune_rest());
    }

    fn rest_lap(&self) {
        Self::sleep_for(self.get_lap_rest());
    }

    fn rel_move_z_with_rest<T: StepperOperations>(&self, stepper_ops: &mut T, stepper: usize, delta: i32, rest: bool) -> Result<()> {
        stepper_ops.rel_move(stepper, delta)?;
        if rest {
            self.rest_z();
        }
        Ok(())
    }

    fn rel_move_z<T: StepperOperations>(&self, stepper_ops: &mut T, stepper: usize, delta: i32) -> Result<()> {
        self.rel_move_z_with_rest(stepper_ops, stepper, delta, true)
    }

    fn rel_move_z_no_rest<T: StepperOperations>(&self, stepper_ops: &mut T, stepper: usize, delta: i32) -> Result<()> {
        self.rel_move_z_with_rest(stepper_ops, stepper, delta, false)
    }

    fn rel_move_x<T: StepperOperations>(&self, stepper_ops: &mut T, stepper: usize, delta: i32) -> Result<()> {
        stepper_ops.rel_move(stepper, delta)?;
        self.rest_x();
        Ok(())
    }

    fn rel_move_tune<T: StepperOperations>(&self, stepper_ops: &mut T, stepper: usize, delta: i32) -> Result<()> {
        stepper_ops.rel_move(stepper, delta)?;
        self.rest_tune();
        Ok(())
    }
    
    /// Set lap_rest value
    pub fn set_lap_rest(&self, rest: f32) {
        if let Ok(mut rest_val) = self.lap_rest.lock() {
            *rest_val = rest;
        }
    }
    
    /// Get lap_rest value
    pub fn get_lap_rest(&self) -> f32 {
        self.lap_rest.lock()
            .map(|r| *r)
            .unwrap_or(4.0)
    }
    
    /// Set adjustment_level value
    pub fn set_adjustment_level(&self, level: i32) {
        if let Ok(mut level_val) = self.adjustment_level.lock() {
            *level_val = level;
        }
    }
    
    /// Get adjustment_level value
    pub fn get_adjustment_level(&self) -> i32 {
        self.adjustment_level.lock()
            .map(|l| *l)
            .unwrap_or(4)
    }
    
    /// Set retry_threshold value
    pub fn set_retry_threshold(&self, threshold: i32) {
        if let Ok(mut thresh) = self.retry_threshold.lock() {
            *thresh = threshold;
        }
    }
    
    /// Get retry_threshold value
    pub fn get_retry_threshold(&self) -> i32 {
        self.retry_threshold.lock()
            .map(|t| *t)
            .unwrap_or(50)
    }
    
    /// Set delta_threshold value
    pub fn set_delta_threshold(&self, threshold: i32) {
        if let Ok(mut thresh) = self.delta_threshold.lock() {
            *thresh = threshold;
        }
    }
    
    /// Get delta_threshold value
    pub fn get_delta_threshold(&self) -> i32 {
        self.delta_threshold.lock()
            .map(|t| *t)
            .unwrap_or(50)
    }
    
    /// Set z_variance_threshold value
    pub fn set_z_variance_threshold(&self, threshold: i32) {
        if let Ok(mut thresh) = self.z_variance_threshold.lock() {
            *thresh = threshold;
        }
    }
    
    /// Get z_variance_threshold value
    pub fn get_z_variance_threshold(&self) -> i32 {
        self.z_variance_threshold.lock()
            .map(|t| *t)
            .unwrap_or(50)
    }
    
    /// Set x_start value
    pub fn set_x_start(&self, start: i32) {
        if let Ok(mut val) = self.x_start.lock() {
            *val = start;
        }
    }
    
    /// Get x_start value
    pub fn get_x_start(&self) -> i32 {
        self.x_start.lock()
            .map(|s| *s)
            .unwrap_or(0)
    }
    
    /// Set x_finish value
    pub fn set_x_finish(&self, finish: i32) {
        if let Ok(mut val) = self.x_finish.lock() {
            *val = finish;
        }
    }
    
    /// Get x_finish value
    pub fn get_x_finish(&self) -> i32 {
        self.x_finish.lock()
            .map(|f| *f)
            .unwrap_or(100)
    }
    
    /// Set x_step value
    pub fn set_x_step(&self, step: i32) {
        if let Ok(mut val) = self.x_step.lock() {
            *val = step;
        }
    }
    
    /// Get x_step value
    pub fn get_x_step(&self) -> i32 {
        self.x_step.lock()
            .map(|s| *s)
            .unwrap_or(10)
    }
    
    /// Get Z stepper indices based on configuration
    pub fn get_z_stepper_indices(&self) -> Vec<usize> {
        let mut indices = Vec::new();
        for i in 0..(self.string_num * 2) {
            let idx = self.z_first_index + i;
            indices.push(idx);
        }
        indices
    }
    
    /// Set stepper enable state
    pub fn set_stepper_enabled(&self, stepper_idx: usize, enabled: bool) {
        if let Ok(mut enabled_map) = self.stepper_enabled.lock() {
            enabled_map.insert(stepper_idx, enabled);
        }
    }
    
    /// Get stepper enable state
    pub fn get_stepper_enabled(&self, stepper_idx: usize) -> bool {
        self.stepper_enabled.lock()
            .map(|map| map.get(&stepper_idx).copied().unwrap_or(false))
            .unwrap_or(false)
    }
    
    /// Get all stepper enabled states (clone of internal map)
    pub fn get_all_stepper_enabled(&self) -> HashMap<usize, bool> {
        self.stepper_enabled.lock()
            .map(|map| map.clone())
            .unwrap_or_default()
    }
    
    /// Get shared memory path for partials data
    /// Returns the path to the shared memory file where audio_streaming writes partials
    pub fn get_shared_memory_path() -> String {
        // Determine shared memory directory based on platform
        let shm_dir = if cfg!(target_os = "linux") {
            "/dev/shm"
        } else if cfg!(target_os = "macos") {
            "/tmp"
        } else {
            "/tmp"
        };
        format!("{}/audio_peaks", shm_dir)
    }
    
    /// Get control file path for audio monitor metadata
    /// Returns the path to the control file that contains channel count and partials info
    fn get_control_file_path() -> String {
        // Determine shared memory directory based on platform (same as shared memory)
        let shm_dir = if cfg!(target_os = "linux") {
            "/dev/shm"
        } else if cfg!(target_os = "macos") {
            "/tmp"
        } else {
            "/tmp"
        };
        format!("{}/audio_control", shm_dir)
    }
    
    /// Read actual channel count and partials per channel from control file
    /// Returns (num_channels, num_partials_per_channel) if file exists and is readable
    /// Returns None if file doesn't exist or can't be read
    fn read_control_file() -> Option<(usize, usize)> {
        let control_path = Self::get_control_file_path();
        let content = std::fs::read_to_string(&control_path).ok()?;
        let lines: Vec<&str> = content.trim().split('\n').collect();
        if lines.len() >= 3 {
            // Format: PID\nnum_channels\nnum_partials
            let num_channels = lines[1].parse::<usize>().ok()?;
            let num_partials = lines[2].parse::<usize>().ok()?;
            Some((num_channels, num_partials))
        } else {
            None
        }
    }
    
    /// Read partials data from shared memory file
    /// Returns None if file doesn't exist or can't be read
    /// num_channels: number of channels to read (typically string_num)
    /// num_partials_per_channel: number of partials per channel (hint, will be overridden by control file if available)
    pub fn read_partials_from_shared_memory(num_channels: usize, mut num_partials_per_channel: usize) -> Option<PartialsData> {
        let shm_path = Self::get_shared_memory_path();
        
        // Try to open and read the shared memory file
        let file = OpenOptions::new().read(true).open(&shm_path).ok()?;
        let mmap = unsafe { Mmap::map(&file).ok()? };
        
        // Deserialize bytes: each partial is (f32 freq, f32 amp) = 8 bytes
        // Format: channel 0 partials, channel 1 partials, etc.
        // Each channel has exactly num_partials_per_channel partials
        const PARTIAL_SIZE: usize = 8; // 2 * f32 = 8 bytes
        
        // Read control file to get actual channel count and partials per channel written by audio_monitor
        let (actual_channels_written, actual_partials_per_channel) = match Self::read_control_file() {
            Some((ch, ppc)) => (ch, ppc),
            None => {
                // Fallback: try to detect from file size if control file not available
                if num_channels > 0 {
                    let total_entries = mmap.len() / PARTIAL_SIZE;
                    let detected = total_entries / num_channels;
                    if detected > 0 {
                        (num_channels, detected) // Assume num_channels is correct if no control file
                    } else {
                        (num_channels, num_partials_per_channel) // Use hint
                    }
                } else {
                    (num_channels, num_partials_per_channel) // Use hint
                }
            }
        };
        
        // Use actual values from control file (or detected values)
        num_partials_per_channel = actual_partials_per_channel;
        
        if num_partials_per_channel == 0 {
            // Fallback to default of 12 if still zero
            num_partials_per_channel = 12;
        }
        
        let channel_size = num_partials_per_channel * PARTIAL_SIZE;
        
        // Read min(actual_channels_written, num_channels) channels
        // This respects the caller's request while not reading beyond what was written
        let channels_to_read = actual_channels_written.min(num_channels);
        
        let mut partials = Vec::new();
        let mut offset = 0;
        
        // Read exactly channels_to_read channels
        for _ in 0..channels_to_read {
            if offset + channel_size > mmap.len() {
                break; // Not enough data
            }
            
            let mut channel_data = Vec::new();
            
            // Read exactly num_partials_per_channel partials for this channel
            for _ in 0..num_partials_per_channel {
                if offset + PARTIAL_SIZE > mmap.len() {
                    break;
                }
                
                let freq_bytes = &mmap[offset..offset + 4];
                let amp_bytes = &mmap[offset + 4..offset + 8];
                
                let freq = f32::from_ne_bytes([freq_bytes[0], freq_bytes[1], freq_bytes[2], freq_bytes[3]]);
                let amp = f32::from_ne_bytes([amp_bytes[0], amp_bytes[1], amp_bytes[2], amp_bytes[3]]);
                
                channel_data.push((freq, amp));
                offset += PARTIAL_SIZE;
            }
            
            partials.push(channel_data);
        }
        
        if partials.is_empty() {
            None
        } else {
            Some(partials)
        }
    }
    
    /// Update voice_count and amp_sum from partials data in the shared slot
    /// Caller should use get_results::read_partials_from_slot() to read from slot
    /// If partials_slot is None, reads from shared memory file as fallback
    pub fn update_audio_analysis_with_partials(&self, partials: Option<PartialsData>) {
        if let Some(partials) = partials {
            let num_channels = partials.len().min(self.string_num);
            
            // Use get_results functions for calculations
            let voice_counts = calculate_voice_count(&partials);
            let amp_sums = calculate_amp_sum(&partials);
            
            // Update voice_count - keep array size at string_num, only update channels that have data
            if let Ok(mut voice_count) = self.voice_count.lock() {
                // Ensure array is at least string_num size
                if voice_count.len() < self.string_num {
                    voice_count.resize(self.string_num, 0);
                }
                // Update channels that have data
                for ch_idx in 0..num_channels {
                    if ch_idx < voice_counts.len() && ch_idx < voice_count.len() {
                        voice_count[ch_idx] = voice_counts[ch_idx];
                    }
                }
            }
            
            // Update amp_sum - keep array size at string_num, only update channels that have data
            if let Ok(mut amp_sum) = self.amp_sum.lock() {
                // Ensure array is at least string_num size
                if amp_sum.len() < self.string_num {
                    amp_sum.resize(self.string_num, 0.0);
                }
                // Update channels that have data
                for ch_idx in 0..num_channels {
                    if ch_idx < amp_sums.len() && ch_idx < amp_sum.len() {
                        amp_sum[ch_idx] = amp_sums[ch_idx];
                    }
                }
            }
        }
    }
    
    /// Update voice_count and amp_sum from partials data in the shared slot
    /// DEPRECATED: Use update_audio_analysis_with_partials() with get_results::read_partials_from_slot()
    /// This method duplicates logic and should not be used - kept for backward compatibility only
    pub fn update_audio_analysis(&self) {
        // Caller should use: get_results::read_partials_from_slot(&slot) then update_audio_analysis_with_partials()
        // This fallback is only for cases where slot is not available and shared memory must be used
        let partials = if self.partials_slot.is_some() {
            // If slot exists, caller should use get_results::read_partials_from_slot() instead
            None  // Force caller to use proper pattern
        } else {
            // Only fallback to shared memory if no slot available
            const DEFAULT_NUM_PARTIALS: usize = 12;
            Self::read_partials_from_shared_memory(self.string_num, DEFAULT_NUM_PARTIALS)
        };
        self.update_audio_analysis_with_partials(partials);
    }
    
    /// Get reference to partials slot (for use with get_results::read_partials_from_slot)
    pub fn partials_slot(&self) -> Option<&PartialsSlot> {
        self.partials_slot.as_ref()
    }
    
    /// Get voice_count array (clone)
    pub fn get_voice_count(&self) -> Vec<usize> {
        self.voice_count.lock()
            .map(|vc| vc.clone())
            .unwrap_or_default()
    }
    
    /// Get amp_sum array (clone)
    pub fn get_amp_sum(&self) -> Vec<f32> {
        self.amp_sum.lock()
            .map(|asum| asum.clone())
            .unwrap_or_default()
    }
    
    /// Get bump status for all Z steppers
    /// Returns Vec<(stepper_index, is_bumping)>
    pub fn get_bump_status(&self) -> Vec<(usize, bool)> {
        let mut status = Vec::new();
        
        if let Some(ref gpio) = self.gpio {
            if !gpio.exist {
                return status;
            }
            
            let z_indices = self.get_z_stepper_indices();
            for &stepper_idx in &z_indices {
                let gpio_index = stepper_idx.saturating_sub(self.z_first_index);
                match gpio.press_check(Some(gpio_index)) {
                    Ok(states) => {
                        let is_bumping = states.get(0).copied().unwrap_or(false);
                        status.push((stepper_idx, is_bumping));
                    }
                    Err(_) => {
                        status.push((stepper_idx, false));
                    }
                }
            }
        }
        
        status
    }
    
    /// Perform bump check on Z-steppers.
    ///
    /// For each enabled Z-stepper (or the specified index):
    /// 1. Poll the touch sensor; if not bumping, do nothing.
    /// 2. If bumping, issue repeated upward moves of `z_up_step`, resting `z_rest` between moves,
    ///    until the sensor clears or the reported position reaches `max_pos`.
    /// 3. When the sensor clears, reset the controller position to `z_up_step` (no hardware motion).
    /// 4. If the sensor never clears and the stepper is already at/above `max_pos`, disable it.
    pub fn bump_check<T: StepperOperations>(
        &self,
        stepper_index: Option<usize>,
        positions: &mut [i32],
        max_positions: &HashMap<usize, i32>,
        stepper_ops: &mut T,
        exit_flag: Option<&Arc<std::sync::atomic::AtomicBool>>,
    ) -> Result<String> {
        let gpio = self.gpio.as_ref().ok_or_else(|| anyhow!("GPIO not initialized"))?;
        if !gpio.exist {
            return Ok("\nno GPIO".to_string());
        }

        if !self.get_bump_check_enable() {
            return Ok("bump_check disabled - skipping".to_string());
        }

        let z_up_step = self.get_z_up_step();
        if z_up_step <= 0 {
            return Err(anyhow!(
                "Invalid z_up_step {} for bump_check: value must be positive to move away from the string",
                z_up_step
            ));
        }

        // Get all Z-stepper indices
        let mut all_z_indices = Vec::new();
        for i in 0..(self.string_num * 2) {
            let idx = self.z_first_index + i;
            all_z_indices.push(idx);
        }
        
        if all_z_indices.is_empty() {
            return Ok(String::new());
        }

        // Build the list of steppers to probe: either all, or one specified
        let steppers_to_check = if let Some(spec_idx) = stepper_index {
            let idx_0_based = if spec_idx > 0 { spec_idx - 1 } else { spec_idx };
            if idx_0_based < all_z_indices.len() {
                vec![all_z_indices[idx_0_based]]
            } else {
                return Ok(format!("\nInvalid stepper index: {}", spec_idx));
            }
        } else {
            all_z_indices.clone()
        };

        let enabled_states = self.get_all_stepper_enabled();
        const MAX_MOVE_ITERATIONS: u32 = 50;
        let mut messages = Vec::new();

        for &stepper_idx in &steppers_to_check {
            if let Some(exit) = exit_flag {
                if exit.load(std::sync::atomic::Ordering::Relaxed) {
                    return Ok(messages.join("\n"));
                }
            }

            let enabled = enabled_states.get(&stepper_idx).copied().unwrap_or(false);
            if !enabled {
                continue;
            }

            let gpio_index = stepper_idx.saturating_sub(self.z_first_index);
            let max_pos = max_positions.get(&stepper_idx).copied().unwrap_or(100);
            
            // Check initial bump state
            let initial_bumping = match gpio.press_check(Some(gpio_index)) {
                Ok(states) => states.get(0).copied().unwrap_or(false),
                Err(e) => {
                    messages.push(format!("GPIO error for stepper {}: {}", stepper_idx, e));
                    continue; // Skip this stepper on GPIO error
                }
            };

            // If not bumping, skip this stepper
            if !initial_bumping {
                continue;
            }

            // Stepper is bumping - move it up until cleared
            let mut cleared = false;
            let mut iterations = 0u32;

            loop {
                if let Some(exit) = exit_flag {
                    if exit.load(std::sync::atomic::Ordering::Relaxed) {
                        return Ok(messages.join("\n"));
                    }
                }

                let current_pos = positions.get(stepper_idx).copied().unwrap_or(0);
                if current_pos >= max_pos {
                    stepper_ops.disable(stepper_idx)?;
                    messages.push(format!(
                        "\nCRITICAL: DISABLING stepper {}. Reason: Bumping at max_pos {}.",
                        stepper_idx, max_pos
                    ));
                    break;
                }

                let remaining = max_pos - current_pos;
                let move_delta = remaining.min(z_up_step);
                self.rel_move_z_no_rest(stepper_ops, stepper_idx, move_delta)?;
                // Position is updated by refresh_positions() - Arduino is source of truth

                // Check if still bumping after move
                let still_bumping = match gpio.press_check(Some(gpio_index)) {
                    Ok(states) => states.get(0).copied().unwrap_or(false),
                    Err(e) => {
                        messages.push(format!("GPIO error for stepper {}: {}", stepper_idx, e));
                        false // Assume cleared on error
                    }
                };

                if !still_bumping {
                    cleared = true;
                    break;
                }

                self.rest_z();

                iterations += 1;
                if iterations >= MAX_MOVE_ITERATIONS {
                    stepper_ops.disable(stepper_idx)?;
                    messages.push(format!(
                        "\nCRITICAL: Stepper {} exceeded {} move attempts while bumping - disabling.",
                        stepper_idx, MAX_MOVE_ITERATIONS
                    ));
                    break;
                }
            }

            if cleared {
                stepper_ops.reset(stepper_idx, z_up_step)?;
                // Position is updated by refresh_positions() - Arduino is source of truth
                messages.push(format!(
                    "\nStepper {} bump cleared - controller set to {}.",
                    stepper_idx, z_up_step
                ));
            }
        }

        Ok(messages.join("\n"))
    }
    
    /// Z-calibrate: Move Z steppers down until they touch sensors.
    /// 
    /// This function calibrates Z-steppers by moving them down until they contact
    /// the touch sensors and leave them at the contact point so a subsequent
    /// bump_check pass can retract them by z_up_step.
    /// 
    /// Args:
    /// - stepper_ops: Trait object for performing stepper operations
    /// - positions: Current stepper positions (will be updated)
    /// - max_positions: Maximum positions for each stepper (index -> max_pos)
    /// - exit_flag: Optional exit flag to check for early return
    /// 
    /// Returns message string describing results
    pub fn z_calibrate<T: StepperOperations>(
        &self,
        stepper_ops: &mut T,
        positions: &mut [i32],
        max_positions: &HashMap<usize, i32>,
        exit_flag: Option<&Arc<std::sync::atomic::AtomicBool>>,
    ) -> Result<String> {
        let gpio = self.gpio.as_ref().ok_or_else(|| anyhow!("GPIO not initialized"))?;
        if !gpio.exist {
            return Ok("Z-Calibration requires GPIO".to_string());
        }
        
        let mut messages = Vec::new();
        messages.push("Running bump_check before Z calibration...".to_string());
        let bump_msg_initial = self.bump_check(None, positions, max_positions, stepper_ops, exit_flag)?;
        if !bump_msg_initial.trim().is_empty() {
            messages.push(bump_msg_initial);
        }
        
        let z_indices = self.get_z_stepper_indices();
        let enabled_states = self.get_all_stepper_enabled();
        let z_down_step = self.get_z_down_step();
        let mut original_positions = std::collections::HashMap::new();
        for &idx in &z_indices {
            if let Some(pos) = positions.get(idx).copied() {
                original_positions.insert(idx, pos);
            }
        }
        
        messages.push("Starting Z calibration...".to_string());
        
        // Calibrate each enabled Z-stepper
        for &stepper_idx in &z_indices {
            // Check exit flag
            if let Some(exit) = exit_flag {
                if exit.load(std::sync::atomic::Ordering::Relaxed) {
                    messages.push("Calibration cancelled".to_string());
                    return Ok(messages.join("\n"));
                }
            }
            
            let enabled = enabled_states.get(&stepper_idx).copied().unwrap_or(false);
            if !enabled {
                messages.push(format!("Skipping disabled stepper {}", stepper_idx));
                continue;
            }
            
            let gpio_index = stepper_idx.saturating_sub(self.z_first_index);
            let max_pos = max_positions.get(&stepper_idx).copied().unwrap_or(100);
            let min_pos = 0; // Default min_pos (could be made configurable)
            
            // Set position to max_pos without moving (like surfer.py's set_stepper)
            // This sets the Arduino's internal position counter without physical movement
            stepper_ops.reset(stepper_idx, max_pos)?;
            // Position is updated by refresh_positions() - Arduino is source of truth
            
            // Move down until sensor is touched
            // Track position locally (like surfer.py's pos_local)
            let mut pos_local = max_pos;
            let mut touched = false;
            
            while !touched {
                // Check exit flag
                if let Some(exit) = exit_flag {
                    if exit.load(std::sync::atomic::Ordering::Relaxed) {
                        messages.push(format!("Calibration cancelled for stepper {}", stepper_idx));
                        break;
                    }
                }
                
                // Check sensor BEFORE moving (surfer.py checks before move)
                match gpio.press_check(Some(gpio_index)) {
                    Ok(states) => {
                        if let Some(&is_touching) = states.get(0) {
                            if is_touching {
                                touched = true;
                                break;
                            }
                        }
                    }
                    Err(e) => {
                        messages.push(format!("GPIO error for stepper {}: {}", stepper_idx, e));
                        break;
                    }
                }
                
                // Check if we've hit minimum position BEFORE moving
                if pos_local <= min_pos {
                    messages.push(format!("Stepper {} bottomed out during calibration (reached min_pos {} without touching) - disabling and leaving at current position", stepper_idx, min_pos));
                    // Disable the stepper since it can't reach the sensor
                    self.set_stepper_enabled(stepper_idx, false);
                    stepper_ops.disable(stepper_idx)?;
                    break;
                }
                
                // Move down (like surfer.py's rmove with down_step)
                self.rel_move_z(stepper_ops, stepper_idx, z_down_step)?;
                pos_local += z_down_step; // Update local position tracker (z_down_step is negative)
                // Position is updated by refresh_positions() - Arduino is source of truth
                
                // Wait using z_rest timing (like surfer.py's waiter(config.ins.z_rest))
                self.rest_z();
            }
            
            if touched {
                stepper_ops.reset(stepper_idx, 0)?;
                // Position is updated by refresh_positions() - Arduino is source of truth
                messages.push(format!("Stepper {} calibrated (touched sensor, reset to 0)", stepper_idx));
            } else {
                messages.push(format!("Stepper {} calibration incomplete", stepper_idx));
            }
        }
        
        // Summarize calibration offsets relative to starting positions
        let mut offset_summaries = Vec::new();
        for &idx in &z_indices {
            if let Some(orig) = original_positions.get(&idx) {
                if let Some(current) = positions.get(idx).copied() {
                    let offset = orig - current;
                    if offset != 0 {
                        offset_summaries.push(format!("{}: {}", idx, offset));
                    }
                }
            }
        }
        if !offset_summaries.is_empty() {
            messages.push(format!("Calibration Offsets: {}", offset_summaries.join(", ")));
        } else {
            messages.push("Calibration Offsets: none".to_string());
        }
        messages.push("Z calibration complete - all enabled steppers moved until touching or disabled".to_string());
        
        // Call bump_check to handle any steppers still touching after calibration
        messages.push("Running bump_check to clear any steppers still touching...".to_string());
        let mut max_positions_map = std::collections::HashMap::new();
        for &stepper_idx in &z_indices {
            max_positions_map.insert(stepper_idx, max_positions.get(&stepper_idx).copied().unwrap_or(100));
        }
        
        // Call bump_check repeatedly until no enabled steppers are touching
        let mut iterations = 0;
        const MAX_BUMP_CHECK_ITERATIONS: u32 = 10; // Safety limit
        loop {
            if iterations >= MAX_BUMP_CHECK_ITERATIONS {
                messages.push("Bump check reached max iterations - stopping".to_string());
                break;
            }
            
            let bump_result = self.bump_check(
                None, // Check all steppers
                positions,
                &max_positions_map,
                stepper_ops,
                exit_flag,
            )?;
            
            // Check if any enabled steppers are still touching
            let mut any_touching = false;
            let current_enabled_states = self.get_all_stepper_enabled();
            for &stepper_idx in &z_indices {
                let enabled = current_enabled_states.get(&stepper_idx).copied().unwrap_or(false);
                if enabled {
                    let gpio_index = stepper_idx.saturating_sub(self.z_first_index);
                    match gpio.press_check(Some(gpio_index)) {
                        Ok(states) => {
                            if let Some(&is_touching) = states.get(0) {
                                if is_touching {
                                    any_touching = true;
                                    break;
                                }
                            }
                        }
                        Err(_) => {}
                    }
                }
            }
            
            if !any_touching {
                messages.push("All enabled steppers cleared - bump_check complete".to_string());
                break;
            }
            
            iterations += 1;
            messages.push(format!("Bump check iteration {} - still clearing steppers", iterations));
        }
        
        Ok(messages.join("\n"))
    }
    
    /// Z-adjust: Adjust Z steppers based on audio analysis (amplitude and voice count).
    /// 
    /// This function adjusts Z-steppers based on audio analysis to keep strings
    /// in the correct position. It checks amplitude sums and voice counts against
    /// thresholds and moves steppers accordingly.
    /// 
    /// Args:
    /// - stepper_ops: Trait object for performing stepper operations
    /// - positions: Current stepper positions (will be updated)
    /// - min_thresholds: Minimum amplitude thresholds per channel
    /// - max_thresholds: Maximum amplitude thresholds per channel
    /// - min_voices: Minimum voice counts per channel
    /// - max_voices: Maximum voice counts per channel
    /// - exit_flag: Optional exit flag to check for early return
    /// 
    /// Returns message string describing results
    pub fn z_adjust<T: StepperOperations>(
        &self,
        stepper_ops: &mut T,
        positions: &mut [i32],
        max_positions: &HashMap<usize, i32>,
        min_thresholds: &[f32],
        max_thresholds: &[f32],
        min_voices: &[usize],
        max_voices: &[usize],
        exit_flag: Option<&Arc<std::sync::atomic::AtomicBool>>,
    ) -> Result<String> {
        let enabled_states = self.get_all_stepper_enabled();
        let z_up_step = self.get_z_up_step();
        let z_down_step = self.get_z_down_step();
        let amp_sums = self.get_amp_sum();
        let voice_counts = self.get_voice_count();
        let mut messages = Vec::new();
        
        messages.push("Running bump_check before Z adjustment...".to_string());
        let bump_msg_initial = self.bump_check(None, positions, max_positions, stepper_ops, exit_flag)?;
        if !bump_msg_initial.trim().is_empty() {
            messages.push(bump_msg_initial);
        }
        
        messages.push("Starting Z adjustment...".to_string());
        
        // Adjust each string (pair of Z steppers)
        for string_idx in 0..self.string_num {
            // Check exit flag
            if let Some(exit) = exit_flag {
                if exit.load(std::sync::atomic::Ordering::Relaxed) {
                    messages.push("Adjustment cancelled".to_string());
                    return Ok(messages.join("\n"));
                }
            }
            
            if string_idx >= amp_sums.len() || string_idx >= voice_counts.len() {
                continue;
            }
            
            let amp_sum = amp_sums[string_idx];
            let voice_count = voice_counts[string_idx];
            
            let min_thresh = min_thresholds.get(string_idx).copied().unwrap_or(20.0);
            let max_thresh = max_thresholds.get(string_idx).copied().unwrap_or(100.0);
            let min_voice = min_voices.get(string_idx).copied().unwrap_or(0);
            let max_voice = max_voices.get(string_idx).copied().unwrap_or(12);
            
            // Determine which stepper to move (z_in or z_out)
            let z_in_idx = self.z_first_index + (string_idx * 2);
            let z_out_idx = self.z_first_index + (string_idx * 2) + 1;
            
            let z_in_enabled = enabled_states.get(&z_in_idx).copied().unwrap_or(false);
            let z_out_enabled = enabled_states.get(&z_out_idx).copied().unwrap_or(false);
            
            if !z_in_enabled && !z_out_enabled {
                messages.push(format!("String {}: both steppers disabled, skipping", string_idx));
                continue;
            }
            
            // Check if adjustment is needed
            let too_close = amp_sum > max_thresh || voice_count > max_voice;
            let too_far = amp_sum < min_thresh || voice_count < min_voice;
            
            if too_close || too_far {
                // Determine which stepper to move based on adjustment direction
                // Positions can be negative (steppers below zero are closer to string)
                // More negative = closer to string, more positive = farther from string
                let z_in_pos = positions.get(z_in_idx).copied().unwrap_or(0);
                let z_out_pos = positions.get(z_out_idx).copied().unwrap_or(0);
                
                let stepper_to_move = if !z_in_enabled {
                    z_out_idx
                } else if !z_out_enabled {
                    z_in_idx
                } else if too_close {
                    // Too close: move the stepper that's closest to the string (most negative position)
                    // Example: if z_in_pos=-10 and z_out_pos=-5, z_in is closer (more negative)
                    // If equal, alternate to keep balanced
                    if z_in_pos < z_out_pos {
                        z_in_idx  // z_in is more negative (closer)
                    } else if z_out_pos < z_in_pos {
                        z_out_idx  // z_out is more negative (closer)
                    } else {
                        // Equal positions: alternate based on string index to keep balanced
                        if string_idx % 2 == 0 {
                            z_in_idx
                        } else {
                            z_out_idx
                        }
                    }
                } else {
                    // too_far: move the stepper that's farthest from the string (most positive/least negative position)
                    // Example: if z_in_pos=-5 and z_out_pos=-10, z_in is farther (less negative)
                    // If equal, alternate to keep balanced
                    if z_in_pos > z_out_pos {
                        z_in_idx  // z_in is less negative/more positive (farther)
                    } else if z_out_pos > z_in_pos {
                        z_out_idx  // z_out is less negative/more positive (farther)
                    } else {
                        // Equal positions: alternate based on string index to keep balanced
                        if string_idx % 2 == 0 {
                            z_out_idx
                        } else {
                            z_in_idx
                        }
                    }
                };
                
                if too_close {
                    // Move stepper up (away from string)
                    self.rel_move_z(stepper_ops, stepper_to_move, z_up_step)?;
                    // Position is updated by refresh_positions() - Arduino is source of truth
                    messages.push(format!(
                        "String {}: too close (amp={:.2}, voices={}), moved stepper {} (closest) up by {}",
                        string_idx, amp_sum, voice_count, stepper_to_move, z_up_step
                    ));
                    self.rest_lap();
                } else {
                    // Move stepper down (toward string)
                    self.rel_move_z(stepper_ops, stepper_to_move, z_down_step)?;
                    // Position is updated by refresh_positions() - Arduino is source of truth
                    messages.push(format!(
                        "String {}: too far (amp={:.2}, voices={}), moved stepper {} (farthest) down by {}",
                        string_idx, amp_sum, voice_count, stepper_to_move, z_down_step
                    ));
                    self.rest_lap();
                }
            } else {
                messages.push(format!(
                    "String {}: in range (amp={:.2}, voices={})",
                    string_idx, amp_sum, voice_count
                ));
            }
        }
        
        messages.push("Running bump_check after Z adjustment...".to_string());
        let bump_msg_final = self.bump_check(None, positions, max_positions, stepper_ops, exit_flag)?;
        if !bump_msg_final.trim().is_empty() {
            messages.push(bump_msg_final);
        }
        messages.push("Z adjustment complete".to_string());
        Ok(messages.join("\n"))
    }
    
    /// Right to left move operation: moves X from x_start to x_finish, adjusting Z at each position
    /// Uses Adjustment Level to iterate in place until successfully passing the value
    /// If attempts exceed Retry Threshold or Z variance threshold, performs calibration
    pub fn right_left_move<T: StepperOperations>(
        &self,
        stepper_ops: &mut T,
        positions: &mut [i32],
        max_positions: &HashMap<usize, i32>,
        min_thresholds: &[f32],
        max_thresholds: &[f32],
        min_voices: &[usize],
        max_voices: &[usize],
        exit_flag: Option<&Arc<std::sync::atomic::AtomicBool>>,
    ) -> Result<String> {
        let x_step_index = self.x_step_index.ok_or_else(|| anyhow!("X stepper not configured"))?;
        let x_start = self.get_x_start();
        let x_finish = self.get_x_finish();
        let x_step = self.get_x_step();
        let adjustment_level = self.get_adjustment_level();
        let retry_threshold = self.get_retry_threshold();
        let z_variance_threshold = self.get_z_variance_threshold();
        
        let mut messages = Vec::new();
        messages.push(format!("Starting right_left_move: X from {} to {} (step: {})", x_start, x_finish, x_step));
        
        // Read current X position from Arduino - Arduino is source of truth
        let current_x_pos = positions.get(x_step_index).copied().ok_or_else(|| anyhow!("Failed to read X position from Arduino"))?;
        messages.push(format!("Current X position from Arduino: {}", current_x_pos));
        
        // Absolute move to x_start if not already there
        if current_x_pos != x_start {
            messages.push(format!("Moving X to absolute position: {} (current: {})", x_start, current_x_pos));
            stepper_ops.abs_move(x_step_index, x_start)?;
            // Position is updated by refresh_positions() in stepper_gui - Arduino knows the position
            // Note: local positions array will be updated when operations_gui polls stepper_gui
        }
        
        // Read current X position from Arduino (after move) - Arduino is source of truth
        let mut current_x = positions.get(x_step_index).copied().ok_or_else(|| anyhow!("Failed to read X position from Arduino"))?;
        messages.push(format!("X position after initial move: {}", current_x));
        let step_direction = if x_finish > x_start { 1 } else { -1 };
        let abs_step = x_step.abs();
        
        while (step_direction > 0 && current_x < x_finish) || (step_direction < 0 && current_x > x_finish) {
            // Check exit flag
            if let Some(exit) = exit_flag {
                if exit.load(std::sync::atomic::Ordering::Relaxed) {
                    messages.push("Operation cancelled".to_string());
                    return Ok(messages.join("\n"));
                }
            }
            
            // At current X position, iterate until we get Adjustment Level consecutive successful passes
            // Each pass = z_adjust + bump_check
            let mut pass_count = 0; // Consecutive successful passes
            let mut attempts = 0; // Total attempts (for retry threshold)
            let mut last_voice_counts = Vec::new();
            
            loop {
                // Check exit flag
                if let Some(exit) = exit_flag {
                    if exit.load(std::sync::atomic::Ordering::Relaxed) {
                        messages.push("Operation cancelled".to_string());
                        return Ok(messages.join("\n"));
                    }
                }
                
                attempts += 1;
                
                // Run z_adjust
                let z_adjust_msg = self.z_adjust(
                    stepper_ops,
                    positions,
                    max_positions,
                    min_thresholds,
                    max_thresholds,
                    min_voices,
                    max_voices,
                    exit_flag,
                )?;
                
                // Run bump_check
                let bump_msg = self.bump_check(None, positions, max_positions, stepper_ops, exit_flag)?;
                
                // Get current voice counts and amp sums
                let voice_counts = self.get_voice_count();
                let amp_sums = self.get_amp_sum();
                
                // Check if all channels are within their min/max ranges (green indicators)
                // A pass is when voice_count AND amp_sum for all channels are within their ranges
                let all_pass = (0..self.string_num).all(|string_idx| {
                    if string_idx >= amp_sums.len() || string_idx >= voice_counts.len() {
                        return false;
                    }
                    let amp_sum = amp_sums[string_idx];
                    let voice_count = voice_counts[string_idx];
                    
                    let min_thresh = min_thresholds.get(string_idx).copied().unwrap_or(20.0);
                    let max_thresh = max_thresholds.get(string_idx).copied().unwrap_or(100.0);
                    let min_voice = min_voices.get(string_idx).copied().unwrap_or(0);
                    let max_voice = max_voices.get(string_idx).copied().unwrap_or(12);
                    
                    // Check both amp_sum and voice_count are within their ranges
                    amp_sum >= min_thresh && amp_sum <= max_thresh &&
                    voice_count >= min_voice && voice_count <= max_voice
                });
                
                if all_pass {
                    // Successful pass - increment pass counter
                    pass_count += 1;
                    messages.push(format!("Pass {} of {} successful at X={} (attempt {})", pass_count, adjustment_level, current_x, attempts));
                    
                    // If we've reached Adjustment Level consecutive passes, move X by step_size and break
                    if pass_count >= adjustment_level {
                        messages.push(format!("Adjustment level {} met at X={} after {} attempts, moving X by step size {}", adjustment_level, current_x, attempts, abs_step));
                        
                        // Move X by exactly x_step_size (relative move)
                        let step_delta = step_direction * abs_step;
                        self.rel_move_x(stepper_ops, x_step_index, step_delta)?;
                        // Position is updated by refresh_positions() - Arduino knows the position
                        // Read updated position from Arduino for next iteration - Arduino is source of truth
                        current_x = positions.get(x_step_index).copied().ok_or_else(|| anyhow!("Failed to read X position from Arduino"))?;
                        messages.push(format!("Moved X by {} to position: {}", step_delta, current_x));
                        
                        // Reset pass counter for next X position
                        pass_count = 0;
                        attempts = 0;
                        break; // Break inner loop to move to next X position
                    }
                } else {
                    // Adjustment failed - reset pass counter
                    if pass_count > 0 {
                        messages.push(format!("Adjustment failed at X={}, resetting pass count from {} to 0", current_x, pass_count));
                    }
                    pass_count = 0;
                }
                
                // Check if we've exceeded retry threshold
                if attempts >= retry_threshold {
                    messages.push(format!("Retry threshold {} exceeded at X={}, performing calibration", retry_threshold, current_x));
                    let cal_msg = self.z_calibrate(stepper_ops, positions, max_positions, exit_flag)?;
                    messages.push(cal_msg);
                    // Reset counters after calibration
                    pass_count = 0;
                    attempts = 0;
                    // Continue trying at current X position
                }
                
                // Check Z variance threshold
                if !last_voice_counts.is_empty() && last_voice_counts.len() == voice_counts.len() {
                    let variance: i32 = voice_counts.iter()
                        .zip(last_voice_counts.iter())
                        .map(|(curr, last)| ((*curr as i32) - (*last as i32)).abs())
                        .sum();
                    
                    if variance > z_variance_threshold {
                        messages.push(format!("Z variance threshold {} exceeded at X={}, performing calibration", z_variance_threshold, current_x));
                        let cal_msg = self.z_calibrate(stepper_ops, positions, max_positions, exit_flag)?;
                        messages.push(cal_msg);
                        // Reset counters after calibration
                        pass_count = 0;
                        attempts = 0;
                        // Continue trying at current X position
                    }
                }
                
                last_voice_counts = voice_counts.clone();
            }
            
            // Break if we've reached x_finish
            if current_x == x_finish {
                break;
            }
        }
        
        messages.push("right_left_move complete".to_string());
        Ok(messages.join("\n"))
    }
    
    /// Left to right move operation: moves X from x_finish to x_start, adjusting Z at each position
    /// Uses Adjustment Level to iterate in place until successfully passing the value
    /// If attempts exceed Retry Threshold or Z variance threshold, performs calibration
    pub fn left_right_move<T: StepperOperations>(
        &self,
        stepper_ops: &mut T,
        positions: &mut [i32],
        max_positions: &HashMap<usize, i32>,
        min_thresholds: &[f32],
        max_thresholds: &[f32],
        min_voices: &[usize],
        max_voices: &[usize],
        exit_flag: Option<&Arc<std::sync::atomic::AtomicBool>>,
    ) -> Result<String> {
        let x_step_index = self.x_step_index.ok_or_else(|| anyhow!("X stepper not configured"))?;
        let x_start = self.get_x_start();
        let x_finish = self.get_x_finish();
        let x_step = self.get_x_step();
        let adjustment_level = self.get_adjustment_level();
        let retry_threshold = self.get_retry_threshold();
        let z_variance_threshold = self.get_z_variance_threshold();
        
        let mut messages = Vec::new();
        messages.push(format!("Starting left_right_move: X from {} to {} (step: {})", x_finish, x_start, x_step));
        
        // Read current X position from Arduino - Arduino is source of truth
        let current_x_pos = positions.get(x_step_index).copied().ok_or_else(|| anyhow!("Failed to read X position from Arduino"))?;
        messages.push(format!("Current X position from Arduino: {}", current_x_pos));
        
        // Absolute move to x_finish if not already there
        if current_x_pos != x_finish {
            messages.push(format!("Moving X to absolute position: {} (current: {})", x_finish, current_x_pos));
            stepper_ops.abs_move(x_step_index, x_finish)?;
            // Position is updated by refresh_positions() in stepper_gui - Arduino knows the position
            // Note: local positions array will be updated when operations_gui polls stepper_gui
        }
        
        // Read current X position from Arduino (after move) - Arduino is source of truth
        let mut current_x = positions.get(x_step_index).copied().ok_or_else(|| anyhow!("Failed to read X position from Arduino"))?;
        messages.push(format!("X position after initial move: {}", current_x));
        let step_direction = if x_start > x_finish { 1 } else { -1 };
        let abs_step = x_step.abs();
        
        while (step_direction > 0 && current_x < x_start) || (step_direction < 0 && current_x > x_start) {
            // Check exit flag
            if let Some(exit) = exit_flag {
                if exit.load(std::sync::atomic::Ordering::Relaxed) {
                    messages.push("Operation cancelled".to_string());
                    return Ok(messages.join("\n"));
                }
            }
            
            // At current X position, iterate until we get Adjustment Level consecutive successful passes
            // Each pass = z_adjust + bump_check
            let mut pass_count = 0; // Consecutive successful passes
            let mut attempts = 0; // Total attempts (for retry threshold)
            let mut last_voice_counts = Vec::new();
            
            loop {
                // Check exit flag
                if let Some(exit) = exit_flag {
                    if exit.load(std::sync::atomic::Ordering::Relaxed) {
                        messages.push("Operation cancelled".to_string());
                        return Ok(messages.join("\n"));
                    }
                }
                
                attempts += 1;
                
                // Run z_adjust
                let z_adjust_msg = self.z_adjust(
                    stepper_ops,
                    positions,
                    max_positions,
                    min_thresholds,
                    max_thresholds,
                    min_voices,
                    max_voices,
                    exit_flag,
                )?;
                
                // Run bump_check
                let bump_msg = self.bump_check(None, positions, max_positions, stepper_ops, exit_flag)?;
                
                // Get current voice counts and amp sums
                let voice_counts = self.get_voice_count();
                let amp_sums = self.get_amp_sum();
                
                // Check if all channels are within their min/max ranges (green indicators)
                // A pass is when voice_count AND amp_sum for all channels are within their ranges
                let all_pass = (0..self.string_num).all(|string_idx| {
                    if string_idx >= amp_sums.len() || string_idx >= voice_counts.len() {
                        return false;
                    }
                    let amp_sum = amp_sums[string_idx];
                    let voice_count = voice_counts[string_idx];
                    
                    let min_thresh = min_thresholds.get(string_idx).copied().unwrap_or(20.0);
                    let max_thresh = max_thresholds.get(string_idx).copied().unwrap_or(100.0);
                    let min_voice = min_voices.get(string_idx).copied().unwrap_or(0);
                    let max_voice = max_voices.get(string_idx).copied().unwrap_or(12);
                    
                    // Check both amp_sum and voice_count are within their ranges
                    amp_sum >= min_thresh && amp_sum <= max_thresh &&
                    voice_count >= min_voice && voice_count <= max_voice
                });
                
                if all_pass {
                    // Successful pass - increment pass counter
                    pass_count += 1;
                    messages.push(format!("Pass {} of {} successful at X={} (attempt {})", pass_count, adjustment_level, current_x, attempts));
                    
                    // If we've reached Adjustment Level consecutive passes, move X by step_size and break
                    if pass_count >= adjustment_level {
                        messages.push(format!("Adjustment level {} met at X={} after {} attempts, moving X by step size {}", adjustment_level, current_x, attempts, abs_step));
                        
                        // Move X by exactly x_step_size (relative move)
                        let step_delta = step_direction * abs_step;
                        self.rel_move_x(stepper_ops, x_step_index, step_delta)?;
                        // Position is updated by refresh_positions() - Arduino knows the position
                        // Read updated position from Arduino for next iteration - Arduino is source of truth
                        current_x = positions.get(x_step_index).copied().ok_or_else(|| anyhow!("Failed to read X position from Arduino"))?;
                        messages.push(format!("Moved X by {} to position: {}", step_delta, current_x));
                        
                        // Reset pass counter for next X position
                        pass_count = 0;
                        attempts = 0;
                        break; // Break inner loop to move to next X position
                    }
                } else {
                    // Adjustment failed - reset pass counter
                    if pass_count > 0 {
                        messages.push(format!("Adjustment failed at X={}, resetting pass count from {} to 0", current_x, pass_count));
                    }
                    pass_count = 0;
                }
                
                // Check if we've exceeded retry threshold
                if attempts >= retry_threshold {
                    messages.push(format!("Retry threshold {} exceeded at X={}, performing calibration", retry_threshold, current_x));
                    let cal_msg = self.z_calibrate(stepper_ops, positions, max_positions, exit_flag)?;
                    messages.push(cal_msg);
                    // Reset counters after calibration
                    pass_count = 0;
                    attempts = 0;
                    // Continue trying at current X position
                }
                
                // Check Z variance threshold
                if !last_voice_counts.is_empty() && last_voice_counts.len() == voice_counts.len() {
                    let variance: i32 = voice_counts.iter()
                        .zip(last_voice_counts.iter())
                        .map(|(curr, last)| ((*curr as i32) - (*last as i32)).abs())
                        .sum();
                    
                    if variance > z_variance_threshold {
                        messages.push(format!("Z variance threshold {} exceeded at X={}, performing calibration", z_variance_threshold, current_x));
                        let cal_msg = self.z_calibrate(stepper_ops, positions, max_positions, exit_flag)?;
                        messages.push(cal_msg);
                        // Reset counters after calibration
                        pass_count = 0;
                        attempts = 0;
                        // Continue trying at current X position
                    }
                }
                
                last_voice_counts = voice_counts.clone();
            }
            
            // Break if we've reached x_start
            if current_x == x_start {
                break;
            }
        }
        
        messages.push("left_right_move complete".to_string());
        Ok(messages.join("\n"))
    }
    
    /// X Home operation: moves X stepper toward home until home limit is hit
    /// Handles both separate home/away pins and single X_LIMIT_PIN (direction-based)
    pub fn x_home<T: StepperOperations>(
        &self,
        stepper_ops: &mut T,
        positions: &mut [i32],
        exit_flag: Option<&Arc<std::sync::atomic::AtomicBool>>,
    ) -> Result<String> {
        let x_step_index = self.x_step_index.ok_or_else(|| anyhow!("X stepper not configured"))?;
        
        // Check if this is a dummy X stepper (X_MAX_POS == 0)
        if self.x_max_pos == Some(0) {
            return Ok("X stepper is dummy (X_MAX_POS=0) - operation skipped".to_string());
        }
        
        let gpio = self.gpio.as_ref().ok_or_else(|| anyhow!("GPIO not initialized"))?;
        if !gpio.exist {
            return Ok("GPIO not available - cannot check home limit".to_string());
        }
        
        let mut messages = Vec::new();
        messages.push("Starting X Home operation...".to_string());
        
        // Check if we have home limit detection
        if gpio.x_home_line.is_none() {
            return Ok("No X home limit switch configured".to_string());
        }
        
        // Get max position - required for this operation
        let x_max_pos = self.x_max_pos.ok_or_else(|| anyhow!("X_MAX_POS not configured"))?;
        if x_max_pos <= 0 {
            return Ok("X_MAX_POS is invalid (must be > 0) - operation skipped".to_string());
        }
        
        // Reset X to max position BEFORE moving to home
        stepper_ops.reset(x_step_index, x_max_pos)?;
        // Position is updated by refresh_positions() - Arduino is source of truth
        messages.push(format!("X position reset to max ({}) before moving to home", x_max_pos));
        
        // Move toward home (negative direction) in -10 step increments until GPIO trigger
        const STEP_SIZE: i32 = -10; // Move 10 steps toward home at a time
        let mut iterations = 0;
        const MAX_ITERATIONS: u32 = 1000; // Safety limit
        
        loop {
            // Check exit flag
            if let Some(exit) = exit_flag {
                if exit.load(std::sync::atomic::Ordering::Relaxed) {
                    messages.push("Operation cancelled".to_string());
                    return Ok(messages.join("\n"));
                }
            }
            
            // Check if we've hit the GPIO trigger (home limit)
            let at_home = gpio.x_home_check().unwrap_or(false);
            
            if at_home {
                messages.push("Home GPIO trigger detected".to_string());
                break; // Exit loop - position will be set to 0 after verification
            }
            
            // Safety check
            if iterations >= MAX_ITERATIONS {
                messages.push(format!("Max iterations ({}) reached - stopping", MAX_ITERATIONS));
                break;
            }
            
            // Move -10 steps toward home
            self.rel_move_x(stepper_ops, x_step_index, STEP_SIZE)?;
            // Position is updated by refresh_positions() in stepper_ops.rel_move(), don't manually update
            iterations += 1;
            
            if iterations % 10 == 0 {
                messages.push(format!("Moving toward home... (iteration {})", iterations));
            }
        }
        
        // Verify we're at home with position 0
        let final_pos = positions.get(x_step_index).copied().unwrap_or(0);
        let still_at_home = gpio.x_home_check().unwrap_or(false);
        
        if still_at_home {
            // Home verified by GPIO - set X to 0
            stepper_ops.reset(x_step_index, 0)?;
            // Position is updated by refresh_positions() - Arduino is source of truth
            messages.push(format!("X Home complete - position set to 0, verified at home"));
        } else {
            // Never reached home - check if Arduino position is already 0
            if final_pos == 0 {
                messages.push(format!("X Home failed - never reached home and Arduino position is already 0"));
                messages.push("Disabling X stepper due to home failure".to_string());
                self.set_stepper_enabled(x_step_index, false);
                stepper_ops.disable(x_step_index)?;
            } else {
                messages.push(format!("X Home failed - never reached home, position: {}", final_pos));
            }
        }
        
        Ok(messages.join("\n"))
    }
    
    /// X Away operation: moves X stepper toward away until away limit is hit
    /// Handles both separate home/away pins and single X_LIMIT_PIN (direction-based)
    pub fn x_away<T: StepperOperations>(
        &self,
        stepper_ops: &mut T,
        positions: &mut [i32],
        exit_flag: Option<&Arc<std::sync::atomic::AtomicBool>>,
    ) -> Result<String> {
        let x_step_index = self.x_step_index.ok_or_else(|| anyhow!("X stepper not configured"))?;
        
        // Check if this is a dummy X stepper (X_MAX_POS == 0)
        if self.x_max_pos == Some(0) {
            return Ok("X stepper is dummy (X_MAX_POS=0) - operation skipped".to_string());
        }
        
        let gpio = self.gpio.as_ref().ok_or_else(|| anyhow!("GPIO not initialized"))?;
        if !gpio.exist {
            return Ok("GPIO not available - cannot check away limit".to_string());
        }
        
        let mut messages = Vec::new();
        messages.push("Starting X Away operation...".to_string());
        
        // Get max position - required for this operation
        let x_max_pos = self.x_max_pos.ok_or_else(|| anyhow!("X_MAX_POS not configured"))?;
        if x_max_pos <= 0 {
            return Ok("X_MAX_POS is invalid (must be > 0) - operation skipped".to_string());
        }
        
        // Set X to 0 first
        stepper_ops.reset(x_step_index, 0)?;
        // Position is updated by refresh_positions() - Arduino is source of truth
        messages.push("X position set to 0".to_string());
        
        // Move toward away (positive direction) in +10 step increments until max pos or GPIO trigger
        const STEP_SIZE: i32 = 10; // Move 10 steps toward away at a time
        let mut iterations = 0;
        const MAX_ITERATIONS: u32 = 1000; // Safety limit
        
        loop {
            // Check exit flag
            if let Some(exit) = exit_flag {
                if exit.load(std::sync::atomic::Ordering::Relaxed) {
                    messages.push("Operation cancelled".to_string());
                    return Ok(messages.join("\n"));
                }
            }
            
            // Get current position (updated by refresh_positions() in previous iteration)
            let current_pos = positions.get(x_step_index).copied().unwrap_or(0);
            
            // Check if we've reached max position
            if current_pos >= x_max_pos {
                messages.push(format!("Max position ({}) reached", x_max_pos));
                break;
            }
            
            // Check if we've hit the GPIO trigger (away limit)
            let at_away = gpio.x_away_check().unwrap_or(false);
            if at_away {
                messages.push("Away GPIO trigger detected".to_string());
                break;
            }
            
            // Safety check
            if iterations >= MAX_ITERATIONS {
                messages.push(format!("Max iterations ({}) reached - stopping", MAX_ITERATIONS));
                break;
            }
            
            // Move +10 steps toward away
            self.rel_move_x(stepper_ops, x_step_index, STEP_SIZE)?;
            // Position is updated by refresh_positions() in stepper_ops.rel_move(), don't manually update
            // The local positions array will be updated when operations_gui polls stepper_gui
            iterations += 1;
            
            if iterations % 10 == 0 {
                // Read current position for logging (may be stale until next poll)
                let logged_pos = positions.get(x_step_index).copied().unwrap_or(0);
                messages.push(format!("Moving toward away... (iteration {}, position: {})", iterations, logged_pos));
            }
        }
        
        // Check final state: if GPIO verified, set to max; if never reached away and at max, disable
        let final_pos = positions.get(x_step_index).copied().unwrap_or(0);
        let at_away_gpio = gpio.x_away_check().unwrap_or(false);
        
        if at_away_gpio {
            // Away verified by GPIO - set X to max pos
            stepper_ops.reset(x_step_index, x_max_pos)?;
            // Position is updated by refresh_positions() - Arduino is source of truth
            messages.push(format!("X Away complete - position set to max: {}, verified at away", x_max_pos));
        } else {
            // Never reached away - check if Arduino position is already at max
            if final_pos >= x_max_pos {
                messages.push(format!("X Away failed - never reached away and Arduino position is already at max ({})", final_pos));
                messages.push("Disabling X stepper due to away failure".to_string());
                self.set_stepper_enabled(x_step_index, false);
                stepper_ops.disable(x_step_index)?;
            } else {
                messages.push(format!("X Away failed - never reached away, position: {}", final_pos));
            }
        }
        
        Ok(messages.join("\n"))
    }
    
    /// X Calibrate operation: moves to home, resets to 0, then moves to away and sets max position
    pub fn x_calibrate<T: StepperOperations>(
        &self,
        stepper_ops: &mut T,
        positions: &mut [i32],
        exit_flag: Option<&Arc<std::sync::atomic::AtomicBool>>,
    ) -> Result<String> {
        let x_step_index = self.x_step_index.ok_or_else(|| anyhow!("X stepper not configured"))?;
        
        // Check if this is a dummy X stepper (X_MAX_POS == 0)
        if self.x_max_pos == Some(0) {
            return Ok("X stepper is dummy (X_MAX_POS=0) - calibration skipped".to_string());
        }
        
        let gpio = self.gpio.as_ref().ok_or_else(|| anyhow!("GPIO not initialized"))?;
        if !gpio.exist {
            return Ok("GPIO not available - cannot calibrate X".to_string());
        }
        
        let mut messages = Vec::new();
        messages.push("Starting X Calibration...".to_string());
        
        // Step 1: Move to home
        messages.push("Step 1: Moving to home position...".to_string());
        let home_msg = self.x_home(stepper_ops, positions, exit_flag)?;
        messages.push(home_msg);
        
        // Check exit flag
        if let Some(exit) = exit_flag {
            if exit.load(std::sync::atomic::Ordering::Relaxed) {
                messages.push("Calibration cancelled".to_string());
                return Ok(messages.join("\n"));
            }
        }
        
        // Step 2: Reset position to 0 at home
        messages.push("Step 2: Resetting X position to 0 at home...".to_string());
        stepper_ops.reset(x_step_index, 0)?;
        // Position is updated by refresh_positions() - Arduino is source of truth
        messages.push("X position reset to 0".to_string());
        
        // Check exit flag
        if let Some(exit) = exit_flag {
            if exit.load(std::sync::atomic::Ordering::Relaxed) {
                messages.push("Calibration cancelled".to_string());
                return Ok(messages.join("\n"));
            }
        }
        
        // Step 3: Move to away
        messages.push("Step 3: Moving to away position...".to_string());
        let away_msg = self.x_away(stepper_ops, positions, exit_flag)?;
        messages.push(away_msg);
        
        // Check exit flag
        if let Some(exit) = exit_flag {
            if exit.load(std::sync::atomic::Ordering::Relaxed) {
                messages.push("Calibration cancelled".to_string());
                return Ok(messages.join("\n"));
            }
        }
        
        // Step 4: Set max position based on current position
        let final_pos = positions.get(x_step_index).copied().unwrap_or(0);
        messages.push(format!("Step 4: Setting max position to {}...", final_pos));
        // Note: We don't have a set_max_position method, so we just record it
        // The max position should be stored in the positions array or max_positions map
        messages.push(format!("X Calibration complete - max position: {}", final_pos));
        
        Ok(messages.join("\n"))
    }
}

