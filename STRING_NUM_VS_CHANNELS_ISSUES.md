# STRING_NUM vs Channels - Code Issues Analysis

## Key Principle
**Do NOT assume channels = strings**
- **Channels**: Audio input channels (from `AUDIO_CHANNELS` in `audio_monitor.yaml`)
- **Strings**: Physical strings with steppers (from hardware configuration)
- These can be different!

## Issues Found

### 1. Audio Analysis Arrays Sized by `string_num` (WRONG)

**Location**: `src/operations.rs` lines 215-216
```rust
voice_count: Arc<new(Mutex::new(vec![0; string_num])),
amp_sum: Arc::new(Mutex::new(vec![0.0; string_num])),
```

**Problem**: These arrays hold per-channel audio data, but are sized by `string_num` (number of strings).
- If `string_num = null` or 0, arrays are empty → no meters display
- If `string_num < actual_channels`, some channels are ignored
- If `string_num > actual_channels`, arrays have unused slots

**Should be**: Sized by actual number of channels from audio data (from control file or partials.len())

---

### 2. Limiting Channels to `string_num` (WRONG)

**Location**: `src/operations.rs` line 642
```rust
let num_channels = partials.len().min(self.string_num);
```

**Problem**: This limits the number of channels processed to `string_num`, even if more channels are available.
- If audio has 4 channels but `string_num = 2`, only 2 channels are processed
- Channels are dropped based on string count, not audio availability

**Should be**: Use `partials.len()` directly (actual number of channels from audio)

---

### 3. Resizing Arrays to `string_num` (WRONG)

**Location**: `src/operations.rs` lines 651-652, 665-666
```rust
if voice_count.len() < self.string_num {
    voice_count.resize(self.string_num, 0);
}
```

**Problem**: Arrays are resized to `string_num` size, but they should be resized to actual channel count.

**Should be**: Resize to `num_channels` (from partials.len())

---

### 4. Using `string_idx` to Index Channel Arrays (WRONG)

**Location**: `src/operations.rs` line 1158
```rust
for string_idx in 0..self.string_num {
    // ...
    if string_idx >= amp_sums.len() || string_idx >= voice_counts.len() {
        continue;
    }
    let amp_sum = amp_sums[string_idx];
    let voice_count = voice_counts[string_idx];
```

**Problem**: 
- Loop uses `string_idx` (string index)
- But indexes into `amp_sums` and `voice_counts` which are channel-indexed arrays
- Assumes string_idx == channel_idx, which may not be true

**Should be**: 
- If this is for stepper operations, it should iterate over actual strings (not channels)
- If this is for audio analysis, it should iterate over channels (not strings)
- Need to clarify: are we adjusting strings or processing channels?

---

### 5. GUI Arrays Sized by `string_num` (WRONG)

**Location**: `src/gui/operations_gui.rs` lines 333-336
```rust
let voice_count_min = vec![voice_count_min_default; string_num];
let voice_count_max = vec![voice_count_cap; string_num];
let amp_sum_min = vec![20; string_num];
let amp_sum_max = vec![250; string_num];
```

**Problem**: GUI threshold arrays are sized by `string_num`, but they control per-channel thresholds.
- If `string_num = null` or 0, no thresholds → no meters
- Should be sized by actual channel count

**Should be**: Sized by actual number of channels (from audio data or control file)

---

### 6. Stepper Calculations Using `string_num` (CORRECT, but needs clarification)

**Location**: `src/operations.rs` lines 179, 478
```rust
for i in 0..(string_num * 2) {
    let stepper_idx = z_first_index + i;
```

**Status**: This is CORRECT - steppers are per-string (2 per string: in/out)
- But only valid when `string_num` actually represents number of strings
- Problem: `string_num` might be null or represent channels, not strings

**Should be**: Only use when `string_num` actually represents physical strings (when hardware present)

---

### 7. Reading Shared Memory with `string_num` Limit (WRONG)

**Location**: `src/operations.rs` line 598 (in `read_partials_from_shared_memory`)
```rust
let channels_to_read = actual_channels_written.min(num_channels);
```

**Context**: `num_channels` parameter comes from caller, which often passes `string_num`

**Problem**: If caller passes `string_num` as `num_channels`, it limits reading to string count instead of actual channels

**Should be**: Read all available channels, don't limit by string_num

---

### 8. Comment Says "typically string_num" (MISLEADING)

**Location**: `src/operations.rs` line 553
```rust
/// num_channels: number of channels to read (typically string_num)
```

**Problem**: Comment suggests channels = strings, which is incorrect assumption

**Should be**: Remove or correct comment to clarify channels ≠ strings

---

## Summary of Required Changes

### Audio Analysis (should use channel count, not string_num):
1. `voice_count` array size - use actual channel count
2. `amp_sum` array size - use actual channel count  
3. `update_audio_analysis_with_partials()` - use `partials.len()`, not `min(partials.len(), string_num)`
4. Array resizing - resize to channel count, not string_num
5. GUI threshold arrays - size by channel count
6. Shared memory reading - don't limit by string_num

### Stepper Operations (should use string_num, but only when it represents strings):
1. Z stepper calculations (`string_num * 2`) - OK, but only when string_num = actual string count
2. `z_adjust()` loop - needs clarification: is it iterating strings or channels?

### Key Questions to Resolve:
1. In `z_adjust()`, what does `string_idx` represent?
   - If it's string index → should iterate over actual strings (may differ from channels)
   - If it's channel index → should iterate over channels (may differ from strings)
2. How to determine actual channel count?
   - From control file: `actual_channels_written`
   - From partials data: `partials.len()`
   - Should NOT use `string_num` for this
3. How to determine actual string count?
   - From hardware configuration (when present)
   - May be different from channel count
   - Should NOT assume it equals channel count

