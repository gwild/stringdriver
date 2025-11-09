# String Driver GUI Binaries

Standalone Rust GUI applications for the String Driver system.

## Binaries

- `stepper_gui` - Stepper motor control GUI
- `operations_gui` - Operations control GUI
- `launcher` - Launcher for all GUI applications
- `pitch_detector` - Standalone pitch detection tool
- `gpio_test` - GPIO testing tool
- `gstreamer_test` - GStreamer pipeline testing tool
- `ard_rust` - Arduino communication test tool

## Building

```bash
cargo build --release
```

## Running

```bash
cargo run --bin stepper_gui --release
cargo run --bin operations_gui --release
cargo run --bin launcher --release
```

## Configuration

Configuration is loaded from `string_driver.yaml` in the project root.

