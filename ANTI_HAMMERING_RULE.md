# ANTI-HAMMERING RULE

## CRITICAL: NO HAMMERING WITHOUT PERMISSION

**Definition of "Hammering":**
- Rapid repeated commands to steppers/Arduino without adequate rest periods
- Excessive GPIO polling (checking sensors faster than necessary)
- Tight loops that send commands faster than hardware can safely handle
- Operations that could stress or damage physical hardware

**Rule:**
1. **BEFORE making ANY code change** that involves:
   - Loops with stepper/Arduino commands
   - GPIO sensor polling
   - Repeated operations on hardware
   - Any sequence of commands to physical devices
   
2. **MUST REQUEST PERMISSION** if the change could result in:
   - Commands faster than configured rest periods (z_rest, x_rest, tune_rest)
   - GPIO checks more frequent than once per operation cycle
   - Loops without adequate delays between iterations
   - Multiple rapid commands to same stepper/Arduino

3. **MUST VERIFY** existing code for hammering before modifying:
   - Check rest periods are respected
   - Verify GPIO polling frequency
   - Ensure loops have proper exit conditions and delays
   - Confirm no rapid command sequences

4. **MUST DOCUMENT** any intentional rapid operations with justification

**Current Rest Periods (from config):**
- z_rest: Default 1.0s (configurable)
- x_rest: Default 5.0s (configurable)  
- tune_rest: Default 5.0s (configurable)

**Examples of Hammering:**
- ❌ Calling GPIO press_check() multiple times per loop iteration without rest
- ❌ Moving steppers faster than z_rest allows
- ❌ refresh_positions() called immediately after every command without delay
- ❌ Tight loops that could execute >10 times per second

**Examples of Safe Operations:**
- ✅ Respecting rest_z(), rest_x(), rest_tune() between moves
- ✅ Single GPIO check per operation cycle
- ✅ refresh_positions() only after move completion (with 500ms wait)
- ✅ Loops with MAX_ITERATIONS limits and proper rest periods

