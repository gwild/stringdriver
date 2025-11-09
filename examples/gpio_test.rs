/// Standalone GPIO test binary - matches GPIO_SD.py test loop (lines 433-457)
/// 
/// Run with: cargo run --example gpio_test --features gpiod

#[path = "../src/config_loader.rs"]
mod config_loader;
#[path = "../src/gpio.rs"]
mod gpio;

use anyhow::Result;
use std::time::Duration;
use std::thread;

fn main() -> Result<()> {
    println!("INFO: Loading host-specific GPIO configuration...");
    let mut gpio = gpio::GpioBoard::new()?;
    
    if gpio.exist {
        println!("INFO: GPIO initialized successfully.");
        println!("Using library: {}", gpio.library.as_deref().unwrap_or("unknown"));
        
        loop {
            println!("--- GPIO Test Loop ---");
            
            if let Some(ref z_pins) = gpio.z_touch_lines {
                match gpio.press_check(None) {
                    Ok(states) => println!("Z-Touch state: {:?}", states),
                    Err(e) => println!("Z-Touch error: {}", e),
                }
            }
            
            if gpio.x_home_line.is_some() || gpio.x_limit_button.is_some() {
                match gpio.x_home_check() {
                    Ok(state) => println!("X-Home state: {}", state),
                    Err(e) => println!("X-Home error: {}", e),
                }
            }
            
            if gpio.x_away_line.is_some() {
                match gpio.x_away_check() {
                    Ok(state) => println!("X-Away state: {}", state),
                    Err(e) => println!("X-Away error: {}", e),
                }
            }
            
            // Encoder (software tracking)
            println!("Encoder Steps: {}", gpio.get_encoder_steps());
            
            // Distance sensor
            if gpio.distance_sensor_enabled {
                match gpio.get_distance() {
                    Ok(dist) => println!("Distance: {}", dist),
                    Err(e) => println!("Distance error: {}", e),
                }
            }
            
            println!("{}", "-".repeat(22));
            println!();
            thread::sleep(Duration::from_secs(1));
        }
    } else {
        println!("GPIO not available or failed to initialize. Exiting test.");
        Ok(())
    }
}

