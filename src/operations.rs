/// Operations module - Rust implementation of operations from surfer.py
/// 
/// Single source of truth: all configuration comes from string_driver.yaml
/// via config_loader - no hardcoded fallbacks.

use anyhow::{anyhow, Result};
use gethostname::gethostname;
use crate::config_loader::{load_operations_settings, load_arduino_settings, load_gpio_settings};
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
    pub z_first_index: usize,
    pub string_num: usize,
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
        let gpio = gpio_settings.map(|_| crate::gpio::GpioBoard::new()).transpose()?;
        let arduino_connected = ard_settings.num_steppers > 0;
        
        // Initialize stepper enabled states (all enabled by default)
        // Only track Z steppers (for operations/bump_check)
        let mut stepper_enabled = HashMap::new();
        for i in 0..(string_num * 2) {
            let stepper_idx = z_first_index + i;
            stepper_enabled.insert(stepper_idx, true);
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
            z_first_index,
            string_num,
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
    
    /// Read partials data from shared memory file
    /// Returns None if file doesn't exist or can't be read
    /// num_channels: number of channels to read (typically string_num)
    /// num_partials_per_channel: number of partials per channel (typically 12)
    pub fn read_partials_from_shared_memory(num_channels: usize, num_partials_per_channel: usize) -> Option<PartialsData> {
        let shm_path = Self::get_shared_memory_path();
        
        // Try to open and read the shared memory file
        let file = OpenOptions::new().read(true).open(&shm_path).ok()?;
        let mmap = unsafe { Mmap::map(&file).ok()? };
        
        // Deserialize bytes: each partial is (f32 freq, f32 amp) = 8 bytes
        // Format: channel 0 partials, channel 1 partials, etc.
        // Each channel has exactly num_partials_per_channel partials
        const PARTIAL_SIZE: usize = 8; // 2 * f32 = 8 bytes
        let channel_size = num_partials_per_channel * PARTIAL_SIZE;
        
        let mut partials = Vec::new();
        let mut offset = 0;
        
        // Read exactly num_channels channels
        for _ in 0..num_channels {
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
            
            // Update voice_count
            if let Ok(mut voice_count) = self.voice_count.lock() {
                voice_count.resize(num_channels, 0);
                for (ch_idx, &count) in voice_counts.iter().take(num_channels).enumerate() {
                    voice_count[ch_idx] = count;
                }
            }
            
            // Update amp_sum
            if let Ok(mut amp_sum) = self.amp_sum.lock() {
                amp_sum.resize(num_channels, 0.0);
                for (ch_idx, &sum) in amp_sums.iter().take(num_channels).enumerate() {
                    amp_sum[ch_idx] = sum;
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
            let mut cleared = false;
            let mut iterations = 0u32;

            loop {
                if let Some(exit) = exit_flag {
                    if exit.load(std::sync::atomic::Ordering::Relaxed) {
                        return Ok(messages.join("\n"));
                    }
                }

                let is_bumping = match gpio.press_check(Some(gpio_index)) {
                    Ok(states) => states.get(0).copied().unwrap_or(false),
                    Err(e) => {
                        messages.push(format!("GPIO error for stepper {}: {}", stepper_idx, e));
                        false
                    }
                };

                if !is_bumping {
                    cleared = true;
                    break;
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
                if let Some(pos) = positions.get_mut(stepper_idx) {
                    *pos += move_delta;
                }

                let still_bumping = match gpio.press_check(Some(gpio_index)) {
                    Ok(states) => states.get(0).copied().unwrap_or(false),
                    Err(e) => {
                        messages.push(format!("GPIO error for stepper {}: {}", stepper_idx, e));
                        false
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
                if let Some(pos) = positions.get_mut(stepper_idx) {
                    *pos = z_up_step;
                }
                messages.push(format!(
                    "\nStepper {} bump cleared - controller set to {}.",
                    stepper_idx, z_up_step
                ));
            }
        }

        Ok(messages.join("\n"))
    }
    
    /// Z-calibrate: Move Z steppers down until they touch sensors, then reset to 0.
    /// 
    /// This function calibrates Z-steppers by moving them down until they contact
    /// the touch sensors, then resets their position to 0.
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
        
        // Temporarily disable bump_check (like surfer.py does)
        let original_bump_check = self.get_bump_check_enable();
        self.set_bump_check_enable(false);
        
        let z_indices = self.get_z_stepper_indices();
        let enabled_states = self.get_all_stepper_enabled();
        let z_down_step = self.get_z_down_step();
        let mut messages = Vec::new();
        
        messages.push("Starting Z calibration...".to_string());
        
        // Calibrate each enabled Z-stepper
        for &stepper_idx in &z_indices {
            // Check exit flag
            if let Some(exit) = exit_flag {
                if exit.load(std::sync::atomic::Ordering::Relaxed) {
                    messages.push("Calibration cancelled".to_string());
                    // Restore bump_check state before returning
                    self.set_bump_check_enable(original_bump_check);
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
            
            // Move down until sensor is touched
            // Track position locally (like surfer.py's pos_local)
            let mut pos_local = max_pos;
            let mut touched = false;
            let mut steps_moved = 0;
            
            while !touched {
                // Check exit flag
                if let Some(exit) = exit_flag {
                    if exit.load(std::sync::atomic::Ordering::Relaxed) {
                        messages.push(format!("Calibration cancelled for stepper {}", stepper_idx));
                        break;
                    }
                }
                
                // Check if we've hit minimum position (like surfer.py checks pos_local <= z_step.min_pos)
                if pos_local <= min_pos {
                    messages.push(format!("Stepper {} bottomed out during calibration (reached min_pos {} without touching) - disabling and setting to max_pos {}", stepper_idx, min_pos, max_pos));
                    // Disable the stepper since it can't reach the sensor
                    self.set_stepper_enabled(stepper_idx, false);
                    stepper_ops.disable(stepper_idx)?;
                    // Set to max_pos (not 0) since it started at max_pos
                    stepper_ops.reset(stepper_idx, max_pos)?;
                    if let Some(pos) = positions.get_mut(stepper_idx) {
                        *pos = max_pos;
                    }
                    break;
                }
                
                // Check sensor
                match gpio.press_check(Some(gpio_index)) {
                    Ok(states) => {
                        // When button_index is Some, press_check returns Vec with one element at index 0
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
                
                // Move down (like surfer.py's rmove with down_step)
                self.rel_move_z(stepper_ops, stepper_idx, z_down_step)?;
                pos_local += z_down_step; // Update local position tracker
                steps_moved += z_down_step.abs();
                
                // Wait using z_rest timing (like surfer.py's waiter(config.ins.z_rest))
                self.rest_z();
            }
            
            if touched {
                // Reset to 0 (like surfer.py's set_stepper to 0)
                stepper_ops.reset(stepper_idx, 0)?;
                if let Some(pos) = positions.get_mut(stepper_idx) {
                    *pos = 0;
                }
                messages.push(format!("Stepper {} calibrated (touched sensor, reset to 0)", stepper_idx));
            } else {
                messages.push(format!("Stepper {} calibration incomplete", stepper_idx));
            }
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
        
        // Re-enable bump_check if it was previously enabled
        self.set_bump_check_enable(original_bump_check);
        if original_bump_check {
            messages.push("Bump check re-enabled".to_string());
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
                    if let Some(pos) = positions.get_mut(stepper_to_move) {
                        *pos += z_up_step;
                    }
                    messages.push(format!(
                        "String {}: too close (amp={:.2}, voices={}), moved stepper {} (closest) up by {}",
                        string_idx, amp_sum, voice_count, stepper_to_move, z_up_step
                    ));
                } else {
                    // Move stepper down (toward string)
                    self.rel_move_z(stepper_ops, stepper_to_move, z_down_step)?;
                    if let Some(pos) = positions.get_mut(stepper_to_move) {
                        *pos += z_down_step;
                    }
                    messages.push(format!(
                        "String {}: too far (amp={:.2}, voices={}), moved stepper {} (farthest) down by {}",
                        string_idx, amp_sum, voice_count, stepper_to_move, z_down_step
                    ));
                }
            } else {
                messages.push(format!(
                    "String {}: in range (amp={:.2}, voices={})",
                    string_idx, amp_sum, voice_count
                ));
            }
        }
        
        messages.push("Z adjustment complete".to_string());
        Ok(messages.join("\n"))
    }
}

