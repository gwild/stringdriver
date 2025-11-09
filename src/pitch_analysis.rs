//! Pitch analysis module using the McLeod Pitch Method (MPM).
//!
//! This implementation is based on the algorithm described in the paper
//! "A Smarter Way to Find Pitch" by Philip McLeod and Geoff Wyvill.

use rustfft::{num_complex::Complex, FftPlanner};

/// Configuration for the pitch analysis.
///
/// This struct holds parameters that can be tuned for different audio
/// sources or performance requirements. It's designed to be loaded
/// from a configuration source.
#[derive(Debug, Clone)]
pub struct PitchConfig {
    pub sample_rate: f32,
    pub clarity_threshold: f32,
    pub power_threshold: f32,
}

impl Default for PitchConfig {
    fn default() -> Self {
        Self {
            sample_rate: 44100.0,
            clarity_threshold: 0.90, // slightly lower for stability on bass
            power_threshold: 0.1,
        }
    }
}

fn apply_hann(signal: &[f32]) -> Vec<f32> {
    let n = signal.len().max(1);
    let denom = (n - 1) as f32;
    signal
        .iter()
        .enumerate()
        .map(|(i, &x)| {
            let w = 0.5 * (1.0 - (2.0 * std::f32::consts::PI * (i as f32) / denom).cos());
            x * w as f32
        })
        .collect()
}

/// Finds the fundamental frequency of a monophonic audio signal.
///
/// # Arguments
/// * `signal` - A slice of f32 audio samples, assumed to be mono.
/// * `config` - Configuration for the pitch detection algorithm.
///
/// # Returns
/// * `Option<f32>` - The detected frequency in Hz, or `None` if no clear pitch is found.
pub fn find_pitch(signal: &[f32], config: &PitchConfig) -> Option<f32> {
    if signal.is_empty() {
        return None;
    }

    // DC removal improves NSDF stability; no windowing here
    let mean = signal.iter().copied().sum::<f32>() / (signal.len() as f32);
    let detrended: Vec<f32> = signal.iter().map(|&x| x - mean).collect();

    // 1. NSDF via FFT ACF + proper energy normalization
    let nsdf = normalized_square_difference(&detrended);

    // 2. Constrain search to plausible tau range (bass-oriented 25–1200 Hz)
    let sr = config.sample_rate.max(1.0);
    let tau_min = (sr / 1200.0).max(1.0) as usize; // max freq ~1200 Hz
    let tau_max = (sr / 25.0) as usize;            // min freq ~25 Hz
    if tau_min + 2 >= nsdf.len() { return None; }
    let tau_end = tau_max.min(nsdf.len().saturating_sub(2));
    if tau_min >= tau_end { return None; }

    // 3. Pick best tau by peak NSDF value within range
    let mut best_pos = tau_min;
    let mut best_val = nsdf[tau_min];
    for pos in (tau_min+1)..=tau_end {
        if nsdf[pos] > best_val {
            best_val = nsdf[pos];
            best_pos = pos;
        }
    }

    // 4. Clarity gate
    if best_val < config.clarity_threshold { return None; }

    // 5. Octave correction: prefer smaller periods (higher fundamentals)
    // if their NSDF peaks are close to the best peak. This mitigates
    // common subharmonic errors on bass instruments (e.g., picking 1/3).
    let mut chosen_pos = best_pos;
    let mut chosen_val = best_val;
    let neighborhood = 2usize; // search +/- 2 samples for local max
    let accept_ratio = 0.98;   // be less aggressive: require 98% of best clarity
    for div in 2..=2 { // only consider 1/2 to avoid over-correction to higher harmonics
        let approx = best_pos as f32 / div as f32;
        if approx < tau_min as f32 { break; }
        let center = approx.round() as usize;
        let start = center.saturating_sub(neighborhood).max(tau_min);
        let end = (center + neighborhood).min(tau_end);
        let mut local_pos = start;
        let mut local_val = nsdf[start];
        for p in (start+1)..=end {
            if nsdf[p] > local_val { local_val = nsdf[p]; local_pos = p; }
        }
        if local_val >= accept_ratio * best_val {
            chosen_pos = local_pos;
            chosen_val = local_val;
            break; // prefer the highest harmonic correction first (1/2, then 1/3, ...)
        }
    }

    // 6. Parabolic interpolation and convert to Hz
    let interpolated_period = parabolic_interpolation(&nsdf, chosen_pos);
    if interpolated_period <= 0.0 { return None; }
    Some(sr / interpolated_period)
}

/// Computes the normalized square difference function (NSDF) of a signal.
/// NSDF(τ) = 2 * sum x[i] x[i+τ] / (sum x[i]^2 + sum x[i+τ]^2)
fn normalized_square_difference(signal: &[f32]) -> Vec<f32> {
    let n = signal.len();
    if n == 0 { return vec![]; }

    // Compute autocorrelation via FFT
    let fft_len = 1usize.next_power_of_two().max((2 * n).next_power_of_two());
    let mut planner = FftPlanner::new();
    let fft = planner.plan_fft_forward(fft_len);
    let ifft = planner.plan_fft_inverse(fft_len);

    let mut buffer: Vec<Complex<f32>> = Vec::with_capacity(fft_len);
    buffer.extend(signal.iter().map(|&x| Complex::new(x, 0.0)));
    buffer.resize(fft_len, Complex::new(0.0, 0.0));

    fft.process(&mut buffer);
    for v in buffer.iter_mut() {
        *v = *v * v.conj();
    }
    ifft.process(&mut buffer);

    // r[tau] for tau in [0..n-1]
    let mut r: Vec<f32> = buffer.iter().take(n).map(|c| c.re / (fft_len as f32)).collect();

    // Precompute cumulative sum of squares for energy terms
    let mut cumsum_sq = vec![0.0f64; n];
    let mut acc = 0.0f64;
    for (i, &x) in signal.iter().enumerate() {
        acc += (x as f64) * (x as f64);
        cumsum_sq[i] = acc;
    }

    // Build NSDF
    let mut nsdf = vec![0.0f32; n];
    nsdf[0] = 1.0; // by definition
    for tau in 1..n {
        let left_energy = cumsum_sq.get(n - tau - 1).copied().unwrap_or(0.0);
        let right_energy = cumsum_sq[n - 1] - cumsum_sq[tau - 1];
        let denom = (left_energy + right_energy) as f32;
        if denom > 1e-12 {
            nsdf[tau] = (2.0 * r[tau]) / denom;
        } else {
            nsdf[tau] = 0.0;
        }
    }
    nsdf
}


/// Find peaks in the NSDF that are "key maximums".
/// A key maximum is a peak that is higher than all previous peaks within a certain threshold.
fn find_key_maximums(nsdf: &[f32], threshold: f32) -> Vec<usize> {
    if nsdf.len() < 3 {
        return Vec::new();
    }
    let mut max_positions = Vec::new();
    let mut pos = 1; // start at 1 to safely access pos-1
    let mut cur_max_pos = 1;

    let limit = (nsdf.len() - 1) / 2; // scan first half as before
    while pos < limit {
        let prev = nsdf[pos - 1];
        let curr = nsdf[pos];
        let next = nsdf[pos + 1];
        if curr > prev && curr >= next {
            if curr > nsdf[cur_max_pos] {
                cur_max_pos = pos;
            }
        }
        pos += 1;
    }

    while cur_max_pos > 0 {
        if nsdf[cur_max_pos] > threshold {
            max_positions.push(cur_max_pos);
        }
        cur_max_pos = find_prev_max(nsdf, cur_max_pos, threshold);
    }
    max_positions
}

fn find_prev_max(nsdf: &[f32], pos: usize, threshold: f32) -> usize {
    let mut new_max_pos = 0;
    let mut search_pos = pos - 1;
    while search_pos > 0 {
        if nsdf[search_pos] > nsdf[new_max_pos] {
            new_max_pos = search_pos;
        }
        search_pos -= 1;
    }

    if nsdf[new_max_pos] > threshold {
        new_max_pos
    } else {
        0
    }
}


/// From the list of key maximums, find the one that corresponds to the fundamental period.
fn get_fundamental_period(max_positions: &[usize], nsdf: &[f32]) -> f32 {
    if max_positions.is_empty() {
        return 0.0;
    }

    // The best choice is often the largest period (lowest frequency) that has
    // a high clarity (NSDF value). We can just take the first one from the list
    // as it represents the highest peak found.
    let mut period = 0.0;
    let mut highest_clarity = 0.0;

    for &pos in max_positions {
        if nsdf[pos] > highest_clarity {
            highest_clarity = nsdf[pos];
            period = pos as f32;
        }
    }
    period
}

/// Improves the accuracy of the period estimation using parabolic interpolation.
fn parabolic_interpolation(array: &[f32], x: usize) -> f32 {
    if x == 0 || x >= array.len() - 1 {
        return x as f32;
    }

    let y_minus_1 = array[x - 1];
    let y = array[x];
    let y_plus_1 = array[x + 1];

    let numerator = y_minus_1 - y_plus_1;
    let denominator = 2.0 * (y_minus_1 - 2.0 * y + y_plus_1);

    if denominator.abs() > 1e-6 {
        x as f32 + numerator / denominator
    } else {
        x as f32
    }
}
