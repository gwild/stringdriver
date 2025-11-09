# String Driver GUI Applications

GUI applications for the String Driver system. These applications read partials results from shared memory to control steppers attached to an Arduino.

## Structure

This repository contains:
- **GUI Applications** (`src/gui/`) - Main user-facing GUI applications that control steppers based on partials from shared memory
- **Support Modules** (`src/`) - Shared code for configuration, operations, GPIO, etc. used by the GUIs
- **Example/Test Tools** (`examples/`) - Debugging and testing utilities

## GUI Applications

- `stepper_gui` - Stepper motor control GUI for Arduino-based stepper control
- `operations_gui` - Operations control GUI for bump checking and stepper management
- `launcher` - Launcher that starts all GUI applications

## Building

```bash
cargo build --release
```

## Running GUI Applications

```bash
# Individual GUIs
cargo run --bin stepper_gui --release
cargo run --bin operations_gui --release

# Or use the launcher to start all GUIs
cargo run --bin launcher --release
```

## Example/Test Tools

Test and debugging tools are available as examples:

```bash
# Arduino communication test
cargo run --example ard_rust

# GPIO testing (requires gpiod feature)
cargo run --example gpio_test --features gpiod
```

## Configuration

Configuration is loaded from `string_driver.yaml` in the project root. The applications read partials data from shared memory (`/dev/shm/audio_peaks` on Linux) to control steppers.

