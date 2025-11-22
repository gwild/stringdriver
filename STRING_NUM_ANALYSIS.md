# STRING_NUM vs Channels Analysis

## Summary of Confusion

There is significant confusion throughout the repository between:
- **STRING_NUM**: Used in `string_driver.yaml` - intended to represent number of strings/audio channels
- **AUDIO_CHANNELS**: Used in `audio_monitor.yaml` - comma-separated list of physical audio input channel indices
- **num_channels**: Runtime variable - actual number of channels being processed
- **ch_idx / string_idx**: Loop variables - sometimes used interchangeably for channel/string indices

## Current Usage Patterns

### 1. Configuration Files

#### `string_driver.yaml` - STRING_NUM
- **Purpose**: Number of audio channels to monitor (should match length of `AUDIO_CHANNELS` in `audio_monitor.yaml`)
- **Values**:
  - `gregory-MacBookAir`: 2 (matches `AUDIO_CHANNELS: "0,1"` = 2 channels)
  - `stringdriver-3`: 4 (matches `AUDIO_CHANNELS: "2,3,4,5"` = 4 channels)
  - `stringdriver-2`: 6 (matches `AUDIO_CHANNELS: "2,3,4,5,6,7"` = 6 channels)
  - `stringdriver-1`: 2 (matches `AUDIO_CHANNELS: "0,1"` = 2 channels)
  - `gregory-Latitude-5550`: 0 (no audio monitoring)
  - `Gregorys-Air.lan`: 0 (no audio monitoring)
- **Usage**: Controls array sizes for voice_count, amp_sum, and GUI meter displays
- **Derivation**: Should be derived from `AUDIO_CHANNELS` length (comma-separated list length)
- **Problem**: When set to 0, no meters display even if audio is being processed

#### `audio_monitor.yaml` - AUDIO_CHANNELS
- **Purpose**: Physical audio input channel indices to read from device
- **Format**: Comma-separated string (e.g., "0,1" or "2,3,4,5")
- **Values**:
  - `gregory-MacBookAir`: "0,1" (2 channels - indices 0 and 1)
  - `stringdriver-3`: "2,3,4,5" (4 channels - indices 2,3,4,5)
  - `stringdriver-2`: "2,3,4,5,6,7" (6 channels - indices 2,3,4,5,6,7)
  - `stringdriver-1`: "0,1" (2 channels - indices 0 and 1)
- **Usage**: Determines which physical audio channels to read from input device
- **Channel Count**: Number of channels = length of comma-separated list (e.g., "0,1" = 2 channels)
- **Relationship**: `selected_channels.len()` becomes `num_channels` at runtime
- **Key Point**: Channel count can be derived from `AUDIO_CHANNELS` length - no separate field needed

### 2. Runtime Variables

#### `num_channels` (in audmon)
- **Source**: `selected_channels.len()` from `AUDIO_CHANNELS` parsing
- **Usage**: 
  - Controls FFT processing (one FFT per channel)
  - Controls shared memory layout (channels × partials)
  - Written to control file: `{pid}\n{num_channels}\n{num_partials}`
- **Location**: `audmon/src/main.rs` line 998-1005, 1072

#### `string_num` (in stringdriver)
- **Source**: `ard_settings.string_num` from `STRING_NUM` in YAML
- **Usage**:
  - Array sizes: `vec![0; string_num]` for voice_count, amp_sum
  - Stepper calculations: `string_num * 2` for Z steppers (in/out pairs)
  - Loop bounds: `for i in 0..(string_num * 2)`
  - GUI meters: `for (ch_idx, count) in voice_count.iter().enumerate()`
- **Location**: `stringdriver/src/operations.rs` lines 89, 128, 179, 215-216, 478, 642

### 3. Shared Memory Interface

#### Control File (`/dev/shm/audio_control` or `/tmp/audio_control`)
- **Format**: `{pid}\n{num_channels}\n{num_partials}`
- **Written by**: `audmon` (line 1072) - uses `selected_channels.len()`
- **Read by**: `stringdriver` (line 542) - reads `num_channels` from control file
- **Purpose**: Tells stringdriver how many channels audio_monitor actually wrote

#### Shared Memory Data Layout
- **Format**: Channel 0 partials, Channel 1 partials, ... (interleaved)
- **Size**: `num_channels × num_partials × 8 bytes` (each partial = 2×f32)
- **Read logic**: `stringdriver` reads `min(actual_channels_written, num_channels)` where:
  - `actual_channels_written` = from control file (what audmon wrote)
  - `num_channels` = `string_num` parameter (what stringdriver expects)

### 4. Code Patterns Showing Confusion

#### Pattern 1: Variable Naming Inconsistency
```rust
// operations.rs line 642
let num_channels = partials.len().min(self.string_num);

// operations.rs line 655, 669
for ch_idx in 0..num_channels {  // Uses "ch_idx" (channel index)

// operations.rs line 1158
for string_idx in 0..self.string_num {  // Uses "string_idx" (string index)
    // But then uses string_idx to index into amp_sums/voice_counts arrays
    // which are indexed by channel, not string!
```

#### Pattern 2: Mixed Terminology in Comments
```rust
// operations.rs line 553
/// num_channels: number of channels to read (typically string_num)

// operations.rs line 16
/// Type alias for partials data: Vec<Vec<(f32, f32)>> where each inner Vec is a channel's partials

// operations.rs line 97-98
voice_count: Arc<Mutex<Vec<usize>>>, // Per-channel voice count
amp_sum: Arc<Mutex<Vec<f32>>>, // Per-channel amplitude sum
```

#### Pattern 3: Array Sizing Logic
```rust
// operations.rs line 642
let num_channels = partials.len().min(self.string_num);
// This means: use whichever is smaller - actual channels from audio OR configured string_num
// Problem: If string_num < actual channels, some channels are ignored
// Problem: If string_num > actual channels, arrays have unused slots
```

### 5. Key Issues Identified

1. **Terminology Mismatch**:
   - Code uses "channel" for audio processing (correct)
   - Code uses "string" for stepper operations (correct)
   - But `string_num` is used for BOTH audio channels AND string count
   - This causes confusion when they don't match

2. **Array Size Mismatch**:
   - `voice_count` and `amp_sum` arrays sized by `string_num`
   - But audio data has `num_channels` (from `AUDIO_CHANNELS.len()`)
   - Code uses `min(partials.len(), string_num)` to reconcile
   - Result: If `AUDIO_CHANNELS` has 4 channels but `STRING_NUM=2`, only 2 channels processed

3. **Stepper Calculation Confusion**:
   - `string_num * 2` used to calculate Z stepper count (in/out pairs)
   - This assumes each string has 2 steppers (Z in, Z out)
   - But `string_num` might be set for audio channels, not actual strings
   - Example: `gregory-MacBookAir` has `STRING_NUM: 4` but no steppers

4. **GUI Display Issues**:
   - Meters created with `vec![value; string_num]`
   - If `string_num = 0`, no meters display
   - But audio might still be processing (if `AUDIO_CHANNELS` is set)

5. **Control File Mismatch**:
   - `audmon` writes `selected_channels.len()` to control file
   - `stringdriver` reads this as `actual_channels_written`
   - But `stringdriver` limits reads to `min(actual_channels_written, string_num)`
   - If `string_num < actual_channels_written`, channels are dropped

## Recommended Clarification

### Proposed Terminology
- **AUDIO_CHANNELS**: Physical audio input channel indices (device-specific, comma-separated string)
- **Channel Count**: Derived from `AUDIO_CHANNELS` length (e.g., "0,1" = 2 channels) - NO SEPARATE FIELD NEEDED
- **STRING_NUM**: Number of audio channels to monitor (should match `AUDIO_CHANNELS` length)
- **NUM_STRINGS**: Number of physical strings (when hardware present, typically equals `STRING_NUM`)
- **NUM_STEPPERS_PER_STRING**: Typically 2 (Z in, Z out)

### When They Should Match
- **STRING_NUM should always match the number of channels in AUDIO_CHANNELS**
- Example: `AUDIO_CHANNELS: "0,1"` → `STRING_NUM: 2`
- Example: `AUDIO_CHANNELS: "2,3,4,5"` → `STRING_NUM: 4`
- In normal operation with hardware: Each audio channel corresponds to one string
- Each string has 2 Z steppers (in/out), so `string_num * 2` Z steppers total

### When They Can Differ
- Testing without hardware: `STRING_NUM` matches `AUDIO_CHANNELS` length (for meters), but `ARD_NUM_STEPPERS = null` (no steppers)
- When borrowing hardware: `STRING_NUM` can be temporarily increased to match borrowed hardware's channel count

## Files Needing Review
1. `string_driver.yaml` - STRING_NUM definitions and comments
2. `audio_monitor.yaml` - AUDIO_CHANNELS definitions
3. `stringdriver/src/operations.rs` - Mixed use of string_num, num_channels, ch_idx, string_idx
4. `stringdriver/src/gui/operations_gui.rs` - Meter display logic
5. `audmon/src/main.rs` - Channel selection and control file writing
6. `stringdriver/src/operations.rs` - Shared memory reading logic

