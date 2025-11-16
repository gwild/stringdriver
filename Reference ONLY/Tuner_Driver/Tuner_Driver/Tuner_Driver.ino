#include <AccelStepper.h>
#include "CmdMessenger.h"
/* Define available CmdMessenger commands */
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

// Constants
const int NUM_STEPPERS = 6; //6 tuners for 6 strings!
const long tMIN_SPEED = 0;
const long tMAX_SPEED = 250;
const int OutputStartPin = 2;
const int InputStartPin = OutputStartPin + (NUM_STEPPERS)*2;

/* Initialize CmdMessenger -- this should match PyCmdMessenger instance */
const uint32_t BAUD_RATE = 115200;
CmdMessenger c = CmdMessenger(Serial, ',', ';', '/');

// Pin assignments
const int SPEED_CONTROL_PIN = A0;
const int tEnablePin = A1;
const int MS1Pin = A3;
const int MS2Pin = A4;
const int MS3Pin = A5;


// Variables
int StepperPins[NUM_STEPPERS][2];
int SwitchPins[NUM_STEPPERS][2];
int SwitchStates[NUM_STEPPERS] = {0};
long tSpeed = tMAX_SPEED;
volatile boolean interrupt[NUM_STEPPERS] = {false};
AccelStepper Steppers[NUM_STEPPERS];
long tMIN = -100000;
long tMAX = 100000;
long tACCELERATION = 10000;
byte incoming = 0;          // incoming serial data
byte incomingOld = 0;         // old data
long minmax[1][2]= {{tMIN,tMAX}};
int p = 0;
int which = 0;
long where = 0;
long amount = 0;
int freeMemory() {
  extern int __heap_start, *__brkval;
  int v;
  return (int)&v - (__brkval == 0 ? (int)&__heap_start : (int)__brkval);
}


/* Create callback functions to deal with incoming messages */
void on_command(void) {

  /* Grab two integers */
  incoming = c.readBinArg<int>();
  /* Send result back */
//  c.sendCmd(command,incoming);
}

void on_positions(void) {
  sendPositions();
}

void on_amove(void) {
  which = c.readBinArg<int>();
  where = c.readBinArg<int>();
  stepperAMove(which, where);
}

void on_rmove(void) {
  which = c.readBinArg<int>();
  where = c.readBinArg<int>();
  stepperRMove(which, where);
//  c.sendCmd(rmove,which,where);
}

void on_reset_all(void) {
  resetPositions();
}

void on_reset_stepper(void) {
  which = c.readBinArg<int>();
  resetStepper(which);
}

void on_set_stepper(void) {
  which = c.readBinArg<int>();
  where = c.readBinArg<int>();
  setStepper(which, where);
}

void on_set_accel(void) {
  which = c.readBinArg<int>();
  amount = c.readBinArg<int>();
  setaccel(which, amount);
}

void on_set_speed(void) {
  which = c.readBinArg<int>();
  amount = c.readBinArg<int>();
  setspeed(which, amount);
}

void on_set_min(void) {
  which = c.readBinArg<int>();
  amount = c.readBinArg<int>();
  setMin(which, amount);
}

void on_set_max(void) {
  which = c.readBinArg<int>();
  amount = c.readBinArg<int>();
  setMax(which, amount);
}

void on_t_size(void) {
  which = c.readBinArg<int>();
  setTSize(which);
//  c.sendCmd(z_size,which);
}

void on_check_memory(void) {
  int freeMem = freeMemory();
  // Send back the free memory to the host
  c.sendCmdStart(check_memory);
  c.sendCmdBinArg(freeMem);
  c.sendCmdEnd();
}

/* Attach callbacks for CmdMessenger commands */
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

void setup() {
  Serial.begin(BAUD_RATE);
  attach_callbacks(); 
   
  // Set the speed control pin as input
  pinMode(SPEED_CONTROL_PIN, INPUT);

  // Set the enable pins as output
  pinMode(tEnablePin, OUTPUT);

  // Set the step size pins as output
  pinMode(MS1Pin, OUTPUT);
  pinMode(MS2Pin, OUTPUT);
  pinMode(MS3Pin, OUTPUT);

// disable steppers
//  digitalWrite(tEnablePin, HIGH);

// set z step size
  digitalWrite(MS1Pin, LOW);
  digitalWrite(MS2Pin, LOW);
  digitalWrite(MS3Pin, LOW);

  // Assign pins for steppers and switches starting with Digital pin 2
  for (int i = 0; i < NUM_STEPPERS; i++) {
    StepperPins[i][0] = OutputStartPin + 2*i;
    StepperPins[i][1] = OutputStartPin + 2*i + 1;
    SwitchPins[i][0] = InputStartPin + 2*i;
    SwitchPins[i][1] = InputStartPin + 2*i + 1;
  }

  for (int i = 0; i < NUM_STEPPERS; i++) {
    // Set switch pins as input and enable interrupts
    pinMode(SwitchPins[i][0], INPUT_PULLUP);
    pinMode(SwitchPins[i][1], INPUT_PULLUP);

  // Set the stepper pins as outputs and create the AccelStepper objects for each stepper
    pinMode(StepperPins[i][0], OUTPUT);
    pinMode(StepperPins[i][1], OUTPUT);

    Steppers[i] = AccelStepper(AccelStepper::DRIVER, StepperPins[i][0], StepperPins[i][1]);  //step, direction
    Steppers[i].setMaxSpeed(tMAX_SPEED);
    Steppers[i].setAcceleration(tACCELERATION);
    Steppers[i].setEnablePin(tEnablePin);
    Steppers[i].setPinsInverted(true,false,true);
  }
  stepperSet();
  resetPositions();
//  Serial.println("StepperPins");
//  for (int i = 0; i < NUM_STEPPERS; i++) {
//    for (int j = 0; j < 2; j++) {
//      Serial.print(StepperPins[i][j]);
//      Serial.print(", ");
//    }
//  Serial.println();
//  }
//  Serial.println("SwitchPins");
//  for (int i = 0; i < NUM_STEPPERS; i++) {
//    for (int j = 0; j < 2; j++) {
//      Serial.print(SwitchPins[i][j]);
//      Serial.print(", ");
//    }
//    Serial.println();
//  }
}

boolean checkPins() {
  volatile boolean switched = false;
  for (int i = 0; i < NUM_STEPPERS; i++) {
    if (digitalRead(SwitchPins[i][0]) == LOW) {
      Steppers [i].moveTo(tMAX);
      switched = true;
    }
    else if (digitalRead(SwitchPins[i][1]) == LOW) {
      Steppers[i].moveTo(tMIN);
      switched = true;
    }
    
    else {
//      Steppers[i].moveTo(Steppers[i].currentPosition());
      Steppers[i].stop();
//      Steppers[i].disableOutputs();
//      Serial.println(Steppers[i].currentPosition());
    }
  }
  return switched;
}

void loop() {
  c.feedinSerialData();
  if (checkPins() == true) {
//    int speedValue = analogRead(SPEED_CONTROL_PIN);
//    tSpeed = map(speedValue, 0, 1023, tMIN_SPEED, tMAX_SPEED);
    for (int i = 0; i < NUM_STEPPERS; i++) {
//      Steppers[i].setMaxSpeed(tSpeed);
//      digitalWrite(tEnablePin, LOW);
      Steppers[i].enableOutputs();
      Steppers[i].run();
     }
  }
//  else {
//    for (int i = 0; i < NUM_STEPPERS; i++) {
//      Steppers[i].disableOutputs();
//    }
//  }
//    digitalWrite(xEnablePin, HIGH);
//    digitalWrite(zEnablePin, HIGH);
//  }
//  printPositions();
  delay(2); //controls overall speed but gets weird to z-steps at less than 5 or so
  for (int i = 0; i < NUM_STEPPERS; i++) {
      Steppers[i].disableOutputs();
  }
//  digitalWrite(tEnablePin, HIGH);
//  sendPositions();
}

void stepperSet() {
  digitalWrite(tEnablePin, HIGH);
  for (int i = 0; i < NUM_STEPPERS; i++) {
    Steppers[i].setMaxSpeed(tMAX_SPEED);
    Steppers[i].setAcceleration(tACCELERATION);
    Steppers[i].disableOutputs();
  }
}

void stepperStop(int j) {
  //Serial.println();
  //Serial.print("stopping stepnum: ");
  //Serial.println(j);
  Steppers[j].stop();
  Steppers[j].move(0);
//  Steppers[j].disableOutputs();
  //  sendPositions();
}

void stepperRMove(int j, int k) {
  int i = 0; //(j != 0);
  p = Steppers[j].currentPosition() + k;
  if (p >= minmax[i][0] && p <= minmax[i][1]) {
//    Steppers[j].setMaxSpeed(tSpeed);
//    digitalWrite(tEnablePin, LOW);
    Steppers[j].enableOutputs();
    Steppers[j].runToNewPosition(p);
    stepperStop(j);
  }
}

void stepperAMove(int j, int k) {
  int i = 0; //(j != 0);
  if (k >= minmax[i][0] && k <= minmax[i][1]) {
//    Steppers[j].setMaxSpeed(tSpeed);
//    digitalWrite(tEnablePin, LOW);
    Steppers[j].enableOutputs();
    Steppers[j].runToNewPosition(k);
    stepperStop(j);
  }
}

void sendPositions() {
  c.sendCmdStart(positions);
  for (int i = 0; i < NUM_STEPPERS; i++) {
    c.sendCmdBinArg(int(Steppers[i].currentPosition()));
    delay(2);
  }
  c.sendCmdEnd();
}

void printPositions() {
  long pos;
  Serial.println("currentPositions");
  for (int i = 0; i < NUM_STEPPERS; i++) {
    pos = int(Steppers[i].currentPosition());
    Serial.print(pos);
    Serial.print(", ");
  }
  Serial.println();
}

void resetPositions() {
  for (int i = 0; i < NUM_STEPPERS; i++) {
    Steppers[i].setCurrentPosition(0);
  }
}

void resetStepper(int j) {
  Steppers[j].setCurrentPosition(0);
  delay(1);
}

void setStepper(int j, int k) {
  Steppers[j].setCurrentPosition(k);
  delay(1);
}

void setaccel(int j, int k) {
  Steppers[j].setAcceleration(k);
}

void setspeed(int j, int k) {
  Steppers[j].setSpeed(k);
}

void setMaxspeed(int j, int k) {
  Steppers[j].setMaxSpeed(k);
}

void setMin(int j, int k) {
  minmax[j][0] = k;
}

void setMax(int j, int k) {
  minmax[j][1] = k;
}

void setTSize(int var) {
  if (var == 0) { //Full step
    digitalWrite(MS1Pin, LOW);
    digitalWrite(MS2Pin, LOW);
    digitalWrite(MS3Pin, LOW);
  }
  else if (var == 4) { //1/2 step
    digitalWrite(MS1Pin, HIGH);
    digitalWrite(MS2Pin, LOW);
    digitalWrite(MS3Pin, LOW);
  }
  else if (var == 2) { //1/4 step
    digitalWrite(MS1Pin, LOW);
    digitalWrite(MS2Pin, HIGH);
    digitalWrite(MS3Pin, LOW);
  }
  else if (var == 6) { //1/8 step
    digitalWrite(MS1Pin, HIGH);
    digitalWrite(MS2Pin, HIGH);
    digitalWrite(MS3Pin, LOW);
  }
  else if (var == 7) { //1/16 step
    digitalWrite(MS1Pin, HIGH);
    digitalWrite(MS2Pin, HIGH);
    digitalWrite(MS3Pin, HIGH);
  }
  else { //Full step
    digitalWrite(MS1Pin, LOW);
    digitalWrite(MS2Pin, LOW);
    digitalWrite(MS3Pin, LOW);
  }
}
