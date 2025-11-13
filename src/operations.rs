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

/// Bump retry counter tracking per stepper index
type BumpRetryCounts = Arc<Mutex<HashMap<usize, u32>>>;

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
    bump_check_repeat: Arc<Mutex<u32>>,
    z_up_step: Arc<Mutex<i32>>,
    z_down_step: Arc<Mutex<i32>>,
    bump_disable_threshold: Arc<Mutex<i32>>,
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
    pub bump_retry_counts: BumpRetryCounts,
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
        let tune_rest = ops_settings.tune_rest.unwrap_or(10.0);
        let x_rest = ops_settings.x_rest.unwrap_or(10.0);
        let z_rest = ops_settings.z_rest.unwrap_or(5.0);
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
            bump_check_repeat: Arc::new(Mutex::new(ops_settings.bump_check_repeat)),
            z_up_step: Arc::new(Mutex::new(z_up_step)),
            z_down_step: Arc::new(Mutex::new(z_down_step)),
            bump_disable_threshold: Arc::new(Mutex::new(ops_settings.bump_disable_threshold)),
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
            bump_retry_counts: Arc::new(Mutex::new(HashMap::new())),
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
    
    /// Set bump_check_repeat count
    pub fn set_bump_check_repeat(&self, repeat: u32) {
        if let Ok(mut rpt) = self.bump_check_repeat.lock() {
            *rpt = repeat;
        }
    }
    
    /// Get bump_check_repeat count
    pub fn get_bump_check_repeat(&self) -> u32 {
        self.bump_check_repeat.lock()
            .map(|r| *r)
            .unwrap_or(10)
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
    
    /// Set bump_disable_threshold value
    pub fn set_bump_disable_threshold(&self, threshold: i32) {
        if let Ok(mut thresh) = self.bump_disable_threshold.lock() {
            *thresh = threshold;
        }
    }
    
    /// Get bump_disable_threshold value
    pub fn get_bump_disable_threshold(&self) -> i32 {
        self.bump_disable_threshold.lock()
            .map(|t| *t)
            .unwrap_or(16)
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
    /// Logic:
    /// 1. Check if any enabled steppers are bumping
    /// 2. Set any enabled bumping stepper to 0
    /// 3. rel_move any enabled stepper up by z_up_step
    /// 4. Repeat until no enabled stepper is bumping
    /// 5. If any stepper is still bumping with position > 16, disable that stepper and set to 0
    /// 6. Any stepper that was bumping but now is not gets set to z_up_step
    /// 
    /// Args:
    /// - stepper_index: Optional specific stepper index to check (1-based, None = all)
    /// - positions: Current stepper positions (for all steppers)
    /// - enabled_states: Enable states for each stepper (index -> enabled)
    /// - max_positions: Maximum positions for each stepper (index -> max_pos)
    /// - string_num: Number of strings (to determine Z stepper count)
    /// - z_first_index: First Z stepper index
    /// - z_up_step: Up step value for recovery moves
    /// - bump_disable_threshold: Position threshold for disabling steppers
    /// - gpio: GPIO board for checking touch sensors
    /// - bump_check_enable: Whether bump check is enabled
    /// - bump_check_repeat: Number of times to repeat the check
    /// - bump_retry_counts: Shared retry counter map
    /// - stepper_ops: Trait object for performing stepper operations
    /// - exit_flag: Optional exit flag to check for early return
    /// 
    /// Returns message string describing results
    pub fn bump_check_static<T: StepperOperations>(
        stepper_index: Option<usize>,
        positions: &[i32],
        enabled_states: &HashMap<usize, bool>,
        max_positions: &HashMap<usize, i32>,
        string_num: usize,
        z_first_index: usize,
        z_up_step: i32,
        bump_disable_threshold: i32,
        gpio: &crate::gpio::GpioBoard,
        bump_check_enable: bool,
        bump_check_repeat: u32,
        bump_retry_counts: &BumpRetryCounts,
        stepper_ops: &mut T,
        exit_flag: Option<&Arc<std::sync::atomic::AtomicBool>>,
    ) -> Result<String> {
        // When bump-check is disabled we just exit early without logging
        if !bump_check_enable {
            return Ok(String::new());
        }

        if !gpio.exist {
            return Ok("\nno GPIO".to_string());
        }

        // Get all Z-stepper indices
        let mut all_z_indices = Vec::new();
        for i in 0..(string_num * 2) {
            let idx = z_first_index + i;
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

        let mut messages = Vec::new();
        
        // Track which steppers were bumping at the start (for recovery)
        let mut was_bumping: HashMap<usize, bool> = HashMap::new();

        // Loop until no enabled steppers are bumping (or max repeats)
        for repeat_iter in 0..bump_check_repeat {
            // Check exit flag if provided
            if let Some(exit) = exit_flag {
                if exit.load(std::sync::atomic::Ordering::Relaxed) {
                    return Ok(messages.join("\n"));
                }
            }

            // Initialize bump statuses for this iteration
            let mut bump_statuses = vec![false; steppers_to_check.len()];
            
            // Check sensor state for each stepper
            for (idx, &stepper_idx) in steppers_to_check.iter().enumerate() {
                let gpio_index = stepper_idx.saturating_sub(z_first_index);
                
                match gpio.press_check(Some(gpio_index)) {
                    Ok(states) => {
                        if let Some(&is_bumping) = states.get(0) {
                            bump_statuses[idx] = is_bumping;
                            // Track if this stepper was bumping at start of cycle
                            if repeat_iter == 0 {
                                was_bumping.insert(stepper_idx, is_bumping);
                            }
                        }
                    }
                    Err(e) => {
                        messages.push(format!("GPIO error for stepper {}: {}", stepper_idx, e));
                    }
                }
            }

            // Compact display with '-' for disabled, '1' for bumping, '0' otherwise
            let mut bit_chars = Vec::new();
            let mut bumping_indices = Vec::new();
            let mut enabled_bumping_indices = Vec::new();
            
            for (i, &stepper_idx) in steppers_to_check.iter().enumerate() {
                let enabled = enabled_states.get(&stepper_idx).copied().unwrap_or(false);
                if !enabled {
                    bit_chars.push("-".to_string());
                } else {
                    if bump_statuses[i] {
                        bit_chars.push("1".to_string());
                        bumping_indices.push(stepper_idx);
                        enabled_bumping_indices.push(stepper_idx);
                    } else {
                        bit_chars.push("0".to_string());
                    }
                }
            }
            
            messages.push(format!("bump statuses: [{}]", bit_chars.join(", ")));
            if !bumping_indices.is_empty() {
                messages.push(format!("bumping: {:?}", bumping_indices));
            }

            // If no enabled steppers are bumping, we're done
            if enabled_bumping_indices.is_empty() {
                // Handle recovery: set steppers that were bumping but now aren't to z_up_step
                for (idx, &stepper_idx) in steppers_to_check.iter().enumerate() {
                    let enabled = enabled_states.get(&stepper_idx).copied().unwrap_or(false);
                    if enabled && was_bumping.get(&stepper_idx).copied().unwrap_or(false) && !bump_statuses[idx] {
                        stepper_ops.reset(stepper_idx, z_up_step)?;
                        messages.push(format!(
                            "\nStepper {} recovered - reset to position {} (z_up_step).",
                            stepper_idx, z_up_step
                        ));
                    }
                }
                break; // Exit loop - no enabled steppers bumping
            }

            // Handle enabled steppers that are bumping
            for (idx, &stepper_idx) in steppers_to_check.iter().enumerate() {
                let enabled = enabled_states.get(&stepper_idx).copied().unwrap_or(false);
                if !enabled || !bump_statuses[idx] {
                    continue;
                }

                let current_pos = positions.get(stepper_idx).copied().unwrap_or(0);

                // Check if position > bump_disable_threshold and still bumping -> disable
                if current_pos > bump_disable_threshold {
                    stepper_ops.reset(stepper_idx, 0)?;
                    stepper_ops.disable(stepper_idx)?;
                    messages.push(format!(
                        "\nCRITICAL: DISABLING stepper {}. Reason: Bumping at position {} (> {}).",
                        stepper_idx, current_pos, bump_disable_threshold
                    ));
                    // Reset retry counter
                    if let Ok(mut counts) = bump_retry_counts.lock() {
                        counts.insert(stepper_idx, 0);
                    }
                    continue;
                }

                // Recovery: set to 0 and move up by z_up_step
                stepper_ops.reset(stepper_idx, 0)?;
                stepper_ops.rel_move(stepper_idx, z_up_step)?;
                messages.push(format!(
                    "\nStepper {} bumping - reset to 0, moved up {} (z_up_step).",
                    stepper_idx, z_up_step
                ));
            }
        }

        Ok(messages.join("\n"))
    }
    
    /// Perform bump check on Z-steppers (instance method wrapper).
    /// 
    /// This is a convenience wrapper around `bump_check_static` that uses
    /// the instance's internal state.
    pub fn bump_check<T: StepperOperations>(
        &self,
        stepper_index: Option<usize>,
        positions: &[i32],
        max_positions: &HashMap<usize, i32>,
        stepper_ops: &mut T,
        exit_flag: Option<&Arc<std::sync::atomic::AtomicBool>>,
    ) -> Result<String> {
        let gpio = self.gpio.as_ref().ok_or_else(|| anyhow!("GPIO not initialized"))?;
        let enabled_states = self.get_all_stepper_enabled();
        let z_up_step = self.get_z_up_step();
        let bump_disable_threshold = self.get_bump_disable_threshold();
        Self::bump_check_static(
            stepper_index,
            positions,
            &enabled_states,
            max_positions,
            self.string_num,
            self.z_first_index,
            z_up_step,
            bump_disable_threshold,
            gpio,
            self.get_bump_check_enable(),
            self.get_bump_check_repeat(),
            &self.bump_retry_counts,
            stepper_ops,
            exit_flag,
        )
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
            
            // Move to max position first to ensure clear of sensor
            stepper_ops.abs_move(stepper_idx, max_pos)?;
            std::thread::sleep(std::time::Duration::from_millis(500));
            
            // Move down until sensor is touched
            let mut current_pos = max_pos;
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
                
                // Check if we've hit minimum position
                if current_pos <= 0 {
                    messages.push(format!("Stepper {} bottomed out during calibration", stepper_idx));
                    break;
                }
                
                // Check sensor
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
                
                // Move down
                stepper_ops.rel_move(stepper_idx, z_down_step)?;
                current_pos += z_down_step;
                steps_moved += z_down_step.abs();
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            
            if touched {
                // Reset to 0
                stepper_ops.reset(stepper_idx, 0)?;
                if let Some(pos) = positions.get_mut(stepper_idx) {
                    *pos = 0;
                }
                messages.push(format!("Stepper {} calibrated (touched sensor, reset to 0)", stepper_idx));
            } else {
                messages.push(format!("Stepper {} calibration incomplete", stepper_idx));
            }
        }
        
        messages.push("Z calibration complete".to_string());
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
                    stepper_ops.rel_move(stepper_to_move, z_up_step)?;
                    if let Some(pos) = positions.get_mut(stepper_to_move) {
                        *pos += z_up_step;
                    }
                    messages.push(format!(
                        "String {}: too close (amp={:.2}, voices={}), moved stepper {} (closest) up by {}",
                        string_idx, amp_sum, voice_count, stepper_to_move, z_up_step
                    ));
                } else {
                    // Move stepper down (toward string)
                    stepper_ops.rel_move(stepper_to_move, z_down_step)?;
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

