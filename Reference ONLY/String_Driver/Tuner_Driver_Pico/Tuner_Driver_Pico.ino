#include <AccelStepper.h>
#include "CmdMessenger.h"

/* Define available CmdMessenger commands (must match host expectations) */
enum {
  command,
  positions,
  amove,
  rmove,
  reset_all,
  reset_stepper,
  set_stepper,
  set_accel,
  set_speed,
  set_min,
  set_max,
  t_size,
  check_memory
};

/*
  Raspberry Pi Pico firmware for tuner driver (Pico variant of Tuner_Driver)
  - 6 tuner steppers (one per string)
  - No limit switches in this Pico variant
  - Single global enable for all drivers
  - Optional shared microstep pins (MS1/MS2/MS3); can be left unconnected
  - Protocol matches original Tuner_Driver: positions frame has 6 ints, includes t_size
*/

// ------------------------------
// Configuration
// ------------------------------

// Physical tuner steppers present on Pico
const int NUM_T_STEPPERS = 6;

// Speeds/accels (host can override via set_speed/set_accel)
const long T_MIN_SPEED = 0;
const long T_MAX_SPEED = 250;               // Match original Tuner_Driver default
const long T_DEFAULT_ACCELERATION = 10000;

// Global enable pin for all stepper drivers (active LOW typical for A4988/DRV8825)
// Use the Pico onboard LED pin (GP25) as the single global enable
const int ENABLE_PIN = 25;                   // Pico GP25 (onboard LED)

// Microstep pins not used for tuners on Pico variant
const int MS1_PIN = -1;
const int MS2_PIN = -1;
const int MS3_PIN = -1;

// STEP/DIR pin mapping for each tuner stepper (Pico GPIO numbers)
// Update these to match your wiring. Pairs are {STEP, DIR}.
const int T_STEPPER_PINS[NUM_T_STEPPERS][2] = {
  {2, 3},   // t0
  {4, 5},   // t1
  {6, 7},   // t2
  {8, 9},   // t3
  {10, 11}, // t4
  {12, 13}  // t5
};

// Physical switch pins (two per tuner): {increase, decrease}
// Configured as INPUT_PULLUP; LOW = active
// Uses available GPIOs that do not conflict with STEP/DIR and GP25
const int T_SWITCH_PINS[NUM_T_STEPPERS][2] = {
  {14, 15}, // t0
  {16, 17}, // t1
  {18, 19}, // t2
  {20, 21}, // t3
  {22, 26}, // t4
  {27, 28}  // t5
};

// ------------------------------
// CmdMessenger
// ------------------------------

const uint32_t BAUD_RATE = 115200;
CmdMessenger c = CmdMessenger(Serial, ',', ';', '/');

// ------------------------------
// State
// ------------------------------

AccelStepper TSteppers[NUM_T_STEPPERS];

long tSpeed = T_MAX_SPEED;                   // Max speed used for runToNewPosition
long tAcceleration = T_DEFAULT_ACCELERATION;

// Shared min/max for all tuner steppers (host can change via set_min/set_max)
long tMIN = -100000;
long tMAX = 100000;

// Scratch variables reused by callbacks
int which = 0;
long where = 0;
long amount = 0;

// ------------------------------
// Helpers
// ------------------------------

static inline void enableDrivers() {
  // Active LOW enables drivers (LED off)
  digitalWrite(ENABLE_PIN, LOW);
}

static inline void disableDrivers() {
  // Active HIGH disables drivers (LED on)
  digitalWrite(ENABLE_PIN, HIGH);
}

// Pico/RP2040 doesn't provide AVR-style freeMemory(); return 0 as stub
int freeMemory() {
  return 0;
}

// ------------------------------
// CmdMessenger callbacks
// ------------------------------

void on_command(void) {
  // Legacy passthrough command space; read and ignore to keep parser in sync
  (void)c.readBinArg<int>();
}

void on_positions(void) {
  // Send 6 integers for tuner positions (match original Tuner_Driver)
  c.sendCmdStart(positions);
  for (int i = 0; i < NUM_T_STEPPERS; i++) {
    c.sendCmdBinArg((int)TSteppers[i].currentPosition());
    delay(2);
  }
  c.sendCmdEnd();
}

void on_amove(void) {
  which = c.readBinArg<int>();
  where = c.readBinArg<int>();
  if (which >= 0 && which < NUM_T_STEPPERS) {
    long target = where;
    if (target < tMIN) target = tMIN;
    if (target > tMAX) target = tMAX;
    TSteppers[which].setMaxSpeed(tSpeed);
    TSteppers[which].enableOutputs();
    TSteppers[which].runToNewPosition(target);
    TSteppers[which].disableOutputs();
  }
}

void on_rmove(void) {
  which = c.readBinArg<int>();
  where = c.readBinArg<int>();
  if (which >= 0 && which < NUM_T_STEPPERS) {
    long target = TSteppers[which].currentPosition() + where;
    if (target < tMIN) target = tMIN;
    if (target > tMAX) target = tMAX;
    TSteppers[which].setMaxSpeed(tSpeed);
    TSteppers[which].enableOutputs();
    TSteppers[which].runToNewPosition(target);
    TSteppers[which].disableOutputs();
  }
}

void on_reset_all(void) {
  for (int i = 0; i < NUM_T_STEPPERS; i++) {
    TSteppers[i].setCurrentPosition(0);
  }
}

void on_reset_stepper(void) {
  which = c.readBinArg<int>();
  if (which >= 0 && which < NUM_T_STEPPERS) {
    TSteppers[which].setCurrentPosition(0);
    delay(1);
  }
}

void on_set_stepper(void) {
  which = c.readBinArg<int>();
  where = c.readBinArg<int>();
  if (which >= 0 && which < NUM_T_STEPPERS) {
    TSteppers[which].setCurrentPosition(where);
    delay(1);
  }
}

void on_set_accel(void) {
  which = c.readBinArg<int>();
  amount = c.readBinArg<int>();
  if (which >= 0 && which < NUM_T_STEPPERS) {
    TSteppers[which].setAcceleration(amount);
  }
  // Also update default used by future moves
  tAcceleration = amount;
}

void on_set_speed(void) {
  which = c.readBinArg<int>();
  amount = c.readBinArg<int>();
  if (which >= 0 && which < NUM_T_STEPPERS) {
    TSteppers[which].setSpeed(amount);
  }
  // Max speed used by runToNewPosition
  tSpeed = amount;
}

void on_set_min(void) {
  (void)c.readBinArg<int>(); // which (ignored; shared for all tuners)
  amount = c.readBinArg<int>();
  tMIN = amount;
}

void on_set_max(void) {
  (void)c.readBinArg<int>(); // which (ignored; shared for all tuners)
  amount = c.readBinArg<int>();
  tMAX = amount;
}

void on_t_size(void) {
  // Shared microstep setting for all drivers (if pins are wired)
  int mode = c.readBinArg<int>();
  if (MS1_PIN < 0 || MS2_PIN < 0 || MS3_PIN < 0) {
    return; // Not wired; ignore
  }
  switch (mode) {
    case 0: // Full step
      digitalWrite(MS1_PIN, LOW);
      digitalWrite(MS2_PIN, LOW);
      digitalWrite(MS3_PIN, LOW);
      break;
    case 4: // 1/2 step
      digitalWrite(MS1_PIN, HIGH);
      digitalWrite(MS2_PIN, LOW);
      digitalWrite(MS3_PIN, LOW);
      break;
    case 2: // 1/4 step
      digitalWrite(MS1_PIN, LOW);
      digitalWrite(MS2_PIN, HIGH);
      digitalWrite(MS3_PIN, LOW);
      break;
    case 6: // 1/8 step
      digitalWrite(MS1_PIN, HIGH);
      digitalWrite(MS2_PIN, HIGH);
      digitalWrite(MS3_PIN, LOW);
      break;
    case 7: // 1/16 step
      digitalWrite(MS1_PIN, HIGH);
      digitalWrite(MS2_PIN, HIGH);
      digitalWrite(MS3_PIN, HIGH);
      break;
    default: // Full step
      digitalWrite(MS1_PIN, LOW);
      digitalWrite(MS2_PIN, LOW);
      digitalWrite(MS3_PIN, LOW);
      break;
  }
}

void on_check_memory(void) {
  int freeMem = freeMemory();
  c.sendCmdStart(check_memory);
  c.sendCmdBinArg(freeMem);
  c.sendCmdEnd();
}

// ------------------------------
// Attach callbacks
// ------------------------------

void attach_callbacks(void) {
  c.attach(command, on_command);
  c.attach(positions, on_positions);
  c.attach(amove, on_amove);
  c.attach(rmove, on_rmove);
  c.attach(reset_all, on_reset_all);
  c.attach(reset_stepper, on_reset_stepper);
  c.attach(set_stepper, on_set_stepper);
  c.attach(set_accel, on_set_accel);
  c.attach(set_speed, on_set_speed);
  c.attach(set_min, on_set_min);
  c.attach(set_max, on_set_max);
  c.attach(t_size, on_t_size);
  c.attach(check_memory, on_check_memory);
}

// ------------------------------
// Physical switch handling
// ------------------------------

// Returns true if any switch is active; sets targets accordingly
boolean checkPins() {
  boolean switched = false;
  for (int i = 0; i < NUM_T_STEPPERS; i++) {
    if (digitalRead(T_SWITCH_PINS[i][0]) == LOW) {
      // Increase direction → move toward tMAX
      TSteppers[i].moveTo(tMAX);
      switched = true;
    }
    else if (digitalRead(T_SWITCH_PINS[i][1]) == LOW) {
      // Decrease direction → move toward tMIN
      TSteppers[i].moveTo(tMIN);
      switched = true;
    }
    else {
      // No switch pressed for this stepper → stop/decelerate
      TSteppers[i].stop();
    }
  }
  return switched;
}

// ------------------------------
// Arduino setup/loop
// ------------------------------

void setup() {
  Serial.begin(BAUD_RATE);
  // This line is CRITICAL for the Pico. It waits for the serial port
  // to be opened by the computer before continuing. Without this,
  // the sketch crashes on startup.
  while (!Serial);
  delay(100); // Brief delay for stability

  attach_callbacks();

  // Global enable pin
  pinMode(ENABLE_PIN, OUTPUT);
  disableDrivers();

  // Microstep pins unused on Pico tuner variant

  // Configure stepper pins and AccelStepper instances
  for (int i = 0; i < NUM_T_STEPPERS; i++) {
    pinMode(T_STEPPER_PINS[i][0], OUTPUT); // STEP
    pinMode(T_STEPPER_PINS[i][1], OUTPUT); // DIR
    TSteppers[i] = AccelStepper(AccelStepper::DRIVER, T_STEPPER_PINS[i][0], T_STEPPER_PINS[i][1]);
    TSteppers[i].setPinsInverted(true, false, true); // Match original Tuner_Driver polarity
    TSteppers[i].setMaxSpeed(T_MAX_SPEED);
    TSteppers[i].setAcceleration(T_DEFAULT_ACCELERATION);
    TSteppers[i].setEnablePin(ENABLE_PIN);
  }

  // Configure switch pins as inputs with pullups
  for (int i = 0; i < NUM_T_STEPPERS; i++) {
    pinMode(T_SWITCH_PINS[i][0], INPUT_PULLUP);
    pinMode(T_SWITCH_PINS[i][1], INPUT_PULLUP);
  }

  // Zero positions on startup
  for (int i = 0; i < NUM_T_STEPPERS; i++) {
    TSteppers[i].setCurrentPosition(0);
  }
}

void loop() {
  // Process incoming command bytes from the host
  c.feedinSerialData();

  // Determine targets from physical switches
  checkPins();

  // Enable, run, then disable every loop to avoid overheating when idle
  for (int i = 0; i < NUM_T_STEPPERS; i++) {
    TSteppers[i].enableOutputs();
  }
  for (int i = 0; i < NUM_T_STEPPERS; i++) {
    TSteppers[i].run();
  }
  for (int i = 0; i < NUM_T_STEPPERS; i++) {
    TSteppers[i].disableOutputs();
  }

  delay(2);
}


