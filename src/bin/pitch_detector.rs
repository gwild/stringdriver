//! Standalone, real-time, multi-channel pitch detector.

#[path = "../pitch_analysis.rs"]
mod pitch_analysis;
#[path = "../config_loader.rs"]
mod config_loader;
#[path = "../crosstalk.rs"]
mod crosstalk;

use crate::pitch_analysis::{find_pitch, PitchConfig};
use anyhow::{anyhow, Result};
use clap::Parser;
use portaudio as pa;
use rayon::prelude::*;
use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use gethostname::gethostname;

/// Standalone Pitch Detector (strict config; no prompts/fallbacks)
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Allow manual startup inputs instead of YAML (interactive). If not set, YAML load is mandatory.
    #[arg(long = "noyaml", default_value_t = false)]
    noyaml: bool,
    /// Optional override for input device index
    #[arg(long)]
    input_device: Option<usize>,
    /// Optional override for input sample rate
    #[arg(long)]
    sample_rate: Option<f64>,
    /// Optional override for channels (e.g., "0,1")
    #[arg(long)]
    channels: Option<String>,
}

fn parse_channels_list(ch: &str) -> Vec<usize> {
    ch.split(',').filter_map(|s| s.trim().parse().ok()).collect()
}

fn rms(buf: &[f32]) -> f32 {
    if buf.is_empty() { return 0.0; }
    let sumsq: f64 = buf.iter().map(|&x| (x as f64) * (x as f64)).sum();
    (sumsq / buf.len() as f64).sqrt() as f32
}

fn list_supported_input_rates(pa: &pa::PortAudio, device_index: pa::DeviceIndex, num_channels: i32) -> Vec<f64> {
    let standard = [
        192000.0, 176400.0, 96000.0, 88200.0, 48000.0, 44100.0,
        32000.0, 22050.0, 16000.0, 11025.0, 8000.0
    ];
    let mut out = Vec::new();
    for &rate in &standard {
        let params = pa::StreamParameters::<f32>::new(device_index, num_channels, true, 0.1);
        if pa.is_input_format_supported(params, rate).is_ok() { out.push(rate); }
    }
    out.sort_by(|a, b| b.partial_cmp(a).unwrap());
    out
}

fn main() -> Result<()> {
    env_logger::Builder::from_default_env()
        .filter_level(log::LevelFilter::Info)
        .init();

    let args = Args::parse();

    // Use config_loader as single source of truth
    let hostname = gethostname().to_string_lossy().to_string();
    let host_cfg = config_loader::host_config_for(&hostname)?;

    let selected_channels: Vec<usize> = host_cfg.AUDIO_CHANNELS
        .ok_or_else(|| anyhow!("AUDIO_CHANNELS missing"))?
        .split(',')
        .filter_map(|s| s.trim().parse().ok())
        .collect();

    let selected_input_device = host_cfg.AUDIO_INPUT_DEVICE
        .ok_or_else(|| anyhow!("AUDIO_INPUT_DEVICE missing"))?;
    let selected_input_sample_rate = host_cfg.AUDIO_INPUT_RATE
        .ok_or_else(|| anyhow!("AUDIO_INPUT_RATE missing"))?;

    // Initialize PortAudio
    let pa = pa::PortAudio::new()?;

    // Validate the chosen device and channels
    let device_index = pa::DeviceIndex(selected_input_device as u32);
    let device_info = pa.device_info(device_index)
        .map_err(|e| anyhow!("Invalid AUDIO_INPUT_DEVICE {}: {}", selected_input_device, e))?;

    // Validate channel bounds strictly
    for &ch in &selected_channels {
        if ch >= device_info.max_input_channels as usize {
            return Err(anyhow!(
                "Channel {} out of range for device '{}' ({} input channels)",
                ch, device_info.name, device_info.max_input_channels
            ));
        }
    }

    // Load crosstalk matrix strictly and build filter to demix before pitch
    let matrix = crosstalk::load_crosstalk()
        .ok_or_else(|| anyhow!("Missing or invalid crosstalk matrix for host {}; expected in crosstalk_training_matrices.yaml", hostname))?;
    let rows = matrix.len();
    let cols = if rows > 0 { matrix[0].len() } else { 0 };
    if rows != selected_channels.len() || cols != selected_channels.len() {
        return Err(anyhow!(
            "CROSSTALK_MATRIX dims {}x{} do not match selected channel count {}",
            rows, cols, selected_channels.len()
        ));
    }
    let filter = Arc::new(crosstalk::CrosstalkFilter::new(matrix));

    // Print one startup line
    let source = hostname.clone();
    println!(
        "Using device={} rate={} channels={:?} source={}",
        selected_input_device,
        selected_input_sample_rate as u32,
        selected_channels,
        source
    );

    let params = pa::StreamParameters::<f32>::new(
        device_index,
        device_info.max_input_channels,
        true,
        device_info.default_low_input_latency,
    );
    let settings = pa::InputStreamSettings::new(params, selected_input_sample_rate, 4096);

    let pitch_config = PitchConfig { sample_rate: selected_input_sample_rate as f32, clarity_threshold: 0.96, ..Default::default() };
    let running = Arc::new(AtomicBool::new(true));
    {
        let r = running.clone();
        ctrlc::set_handler(move || { r.store(false, Ordering::SeqCst); }).ok();
    }

    // Strict per-channel analysis using config-selected channels only
    let callback_selected = selected_channels.clone();
    let num_input_channels = device_info.max_input_channels as usize;
    let filter_cb = Arc::clone(&filter);

    let callback = move |pa::stream::InputCallbackArgs { buffer, .. }| {
        // Build per-selected-channel buffers
        let frames = buffer.len() / num_input_channels;
        let mut channel_buffers: Vec<Vec<f32>> = callback_selected
            .iter()
            .map(|_| Vec::with_capacity(frames))
            .collect();

        for f in 0..frames {
            let base = f * num_input_channels;
            for (i, &sel_ch) in callback_selected.iter().enumerate() {
                channel_buffers[i].push(buffer[base + sel_ch]);
            }
        }

        // Apply crosstalk demix before pitch detection
        let demixed = filter_cb.filter(&channel_buffers);

        // Run pitch detection in parallel on demixed channels
        let pitches: Vec<Option<f32>> = demixed
            .par_iter()
            .map(|ch_buf| find_pitch(ch_buf, &pitch_config))
            .collect();

        let line = pitches.iter()
            .map(|p| match p { Some(freq) => format!("{:8.2}", freq), None => "    --  ".to_string() })
            .collect::<Vec<_>>()
            .join(" | ");
        print!("\rPitches (Hz): [ {} ] ", line);
        let _ = io::stdout().flush();

        if running.load(Ordering::SeqCst) { pa::Continue } else { pa::Complete }
    };

    let mut stream = pa.open_non_blocking_stream(settings, callback)?;
    stream.start()?;

    while stream.is_active()? { thread::sleep(std::time::Duration::from_millis(100)); }
    let _ = stream.stop();
    println!("\nStream stopped.");

    Ok(())
}
