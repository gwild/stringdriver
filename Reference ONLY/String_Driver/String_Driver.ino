
// --------------------------------------------------------------------------------------------------------------

// variable setup
int p = 0;
int which = 0;
long where = 0;
long amount = 0;
float gs = 0;
byte incoming = 0;          // incoming serial data
byte incomingOld = 0;         // old data
long tuneMin = -25000;
long tuneMax = 25000;
long tuneSpeed = 1000;
long tuneAccel = 10000;
int gantryMin = 0;
long gantryMax = 10000;
long gantrySpeed = 700;
long gantryMaxSpeed = 1000;
long gantryAccel = 10000;
long coilSpeed = 50;
long coilAccel = 10000;
int coilMin = -100;
int coilMax = 100;
String inString = "";    // string to hold inputo
int spots[7]= {0,0,0,0,0,0,0};
char spotty[7];
long minmax[3][2]= {{tuneMin,tuneMax}, {gantryMin,gantryMax}, {coilMin,coilMax}};
long thismin;
long thismax;
long accel[3]= {tuneAccel, gantryAccel, coilAccel};
long speeds[3]= {tuneSpeed, gantrySpeed, coilSpeed};

int freeMemory() {
  extern int __heap_start, *__brkval;
  int v;
  return (int)&v - (__brkval == 0 ? (int)&__heap_start : (int)__brkval);
}

const int buttons[15] = {43, 42, 45, 44, 47, 46, 51, 50, 49, 48, 53, 52, 11, 10, 12};
int buttonState[15] = {1,1,1,1,1,1,1,1,1,1,1,1,1,1,1};
int buttonStateOld[15] = {1,1,1,1,1,1,1,1,1,1,1,1,1,1,1};
const int stepnum = 7;
#include <AccelStepper.h>
// Define a stepper and the pins it will use
AccelStepper stepper0(AccelStepper::FULL4WIRE, 15, 14, 16, 17); // Tuner 1
AccelStepper stepper1(AccelStepper::FULL4WIRE, 18, 19, 20, 21); // Tuner 2
AccelStepper stepper2(AccelStepper::HALF4WIRE, 22, 24, 23, 25); // Gantry
AccelStepper stepper3(AccelStepper::HALF4WIRE, 27, 26, 28, 29); // Input 1
AccelStepper stepper4(AccelStepper::HALF4WIRE, 30, 31, 32, 33); // Output 1
AccelStepper stepper5(AccelStepper::HALF4WIRE, 34, 35, 36, 37); // Input 2
AccelStepper stepper6(AccelStepper::HALF4WIRE, 38, 39, 40, 41); // Output 2
AccelStepper* mySteppers[stepnum] = {&stepper0, &stepper1, &stepper2, &stepper3, &stepper4, &stepper5, &stepper6};

#include "CmdMessenger.h"
/* Define available CmdMessenger commands */
enum {
    command,
    gspeed,
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
    check_memory
};

/* Initialize CmdMessenger -- this should match PyCmdMessenger instance */
const long BAUD_RATE = 115200;
CmdMessenger c = CmdMessenger(Serial,',',';','/');

/* Create callback functions to deal with incoming messages */
void on_command(void){
   
    /* Grab two integers */
    incoming = c.readBinArg<int>();
    /* Send result back */ 
    //c.sendCmd(command,incoming);
}

void on_gspeed(void){
  gs = c.readBinArg<float>();
  stepper2.setMaxSpeed(abs(gantryMaxSpeed*gs));
  if (gs > 0.){
    stepperMove(2, gantryMax);  // gantry right
  }
  else {
    if (gs < 0.){
      stepperMove(2, gantryMin);  // gantry left    
    }
  }
}
 
void on_positions(void){
  sendPositions();
}

void on_amove(void){
  which = c.readBinArg<int>();
  where = c.readBinArg<long>();
  stepperAMove(which,where);
}

void on_rmove(void){
  which = c.readBinArg<int>();
  where = c.readBinArg<long>();
  stepperRMove(which,where);
}

void on_reset_all(void){
  resetPositions();
}

void on_reset_stepper(void){
  which = c.readBinArg<int>();
  resetStepper(which);
}

void on_set_stepper(void){
  which = c.readBinArg<int>();
  where = c.readBinArg<long>();
  setStepper(which,where);
}

void on_set_accel(void){
  which = c.readBinArg<int>();
  amount = c.readBinArg<long>();
  setaccel(which,amount);
}

void on_set_speed(void){
  which = c.readBinArg<int>();
  amount = c.readBinArg<long>();
  setspeed(which,amount);
}

void on_set_min(void){
  which = c.readBinArg<int>();
  amount = c.readBinArg<long>();
  setMin(which,amount);
}

void on_set_max(void){
  which = c.readBinArg<int>();
  amount = c.readBinArg<long>();
  setMax(which,amount);
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
  
    c.attach(command,on_command);
    c.attach(positions,on_positions);
    c.attach(gspeed,on_gspeed);
    c.attach(amove,on_amove);
    c.attach(rmove,on_rmove);
    c.attach(reset_all,on_reset_all);
    c.attach(reset_stepper,on_reset_stepper);
    c.attach(set_stepper,on_set_stepper);
    c.attach(set_accel,on_set_accel);
    c.attach(set_speed,on_set_speed);
    c.attach(set_min,on_set_min);
    c.attach(set_max,on_set_max);
    c.attach(check_memory, on_check_memory);
}

void setup() {
  Serial.begin(BAUD_RATE);
  attach_callbacks(); 
  stepperSet();
  for (int i = 0; i <= 14; i++) {
    pinMode(buttons[i], INPUT_PULLUP); //set up button inputs
  }
}  // end of setup

// the loop part of the sketch
void loop() {
  c.feedinSerialData();
  for (int i = 0; i <= 14; i++) { //read button inputs
    buttonState[i] = digitalRead(buttons[i]);
  }

  for (int i = 0; i <= 12; i = i + 2) { //check to see if any buttons de/activated
    if (buttonStateOld[i] != buttonState[i] or buttonStateOld[i + 1] != buttonState[i + 1]) {
      if (buttonState[i] == HIGH && buttonState[i + 1] == HIGH) {
        incoming = 2 * i;  // stop
      }
      else {
        if (buttonState[i] == LOW) {
          incoming = 2 * i + 1; // move minus
        }
        else {
          incoming = 2 * i + 3; // move plus
        }
      }
    }
  }
  if (buttonState[14] != buttonStateOld[14]) {
    incoming = 28 + buttonState[14];
  }
  for (int i = 0; i <= 14; i++) { //copy buttonState to buttonStateOld
    buttonStateOld[i] = buttonState[i];
  }
  
  if (incomingOld != incoming){
//    Serial.println(incoming);
  switch (incoming) {
    case 0:
      stepperStop(0); //tuner 1 stop
      break;
    case 1:
      stepperMove(0, tuneMin); // tuner 1 down
      break;
    case 2:
      stepperStop(0); //tuner 1 stop
      break;
    case 3:
      stepperMove(0, tuneMax);  // tuner 1 up
      break;

    case 4:
      stepperStop(1);  // tune 2 stop
      break;
    case 5:
      stepperMove(1, tuneMin);  // tuner 2 down
      break;
    case 6:
      stepperStop(1);  // tuner 2 stop
      break;
    case 7:
      stepperMove(1, tuneMax);  // tuner 2 up
      break;

    case 8:
      stepperStop(2);  //gantry stop
      break;
    case 9:
      stepperMove(2, gantryMin);  // gantry left
      break;
    case 10:
      stepperStop(2);    // gantry stop
      break;
    case 11:
      stepperMove(2, gantryMax);  // gantry right
      break;

    case 12:
      stepperStop(3);    // input 1 stop
      break;
    case 13:
      stepperMove(3, coilMin);  // input 1 down
      break;
    case 14:
      stepperStop(3);    // input 1 stop
      break;
    case 15:
      stepperMove(3, coilMax);  // input 1 up
      break;

    case 16:
      stepperStop(4);    // output 1 stop
      break;
    case 17:
      stepperMove(4, coilMin);  // output 1 down
      break;
    case 18:
      stepperStop(4);    // output 1 stop
      break;
    case 19:
      stepperMove(4, coilMax);  // output 1 up
      break;

    case 20:
      stepperStop(5);    // input 2 stop
      break;
    case 21:
      stepperMove(5, coilMin);  // input 2 down
      break;
    case 22:
      stepperStop(5);    // input 2 stop
      break;
    case 23:
      stepperMove(5, coilMax);  // input 2 up
      break;
      
    case 24:
      stepperStop(6);    // output 2 stop
      break;
    case 25:
      stepperMove(6, coilMin);  // output 2 down
      break;
    case 26:
      stepperStop(6);    // output 2 stop
      break;
    case 27:
      stepperMove(6, coilMax);  // output 2 up
      break;
      
    case 28: //move slow
      tuneSpeed = 50;
      tuneAccel = 1000;
      gantrySpeed = 500;
      gantryAccel = 10000;
      coilSpeed = 2;
      coilAccel = 1000;
      stepperSet();
      break;
      
    case 29: //move fast
      tuneSpeed = 1000;
      tuneAccel = 10000;
      gantrySpeed = 700;
      gantryAccel = 10000;
      coilSpeed = 50;
      coilAccel = 10000;
      stepperSet();
      break;
      
    case 30: //report positions
      sendPositions();
      break;
      
    case 31: //pass
      break;

  } //end of switch
  
  incomingOld=incoming;
    } //end of if (incomingOld != incoming)
    
  for (int i = 0; i < stepnum; i++) {
    mySteppers[i]->run();
  }
} // end of loop()

void stepperSet() {
  //stepper0.setCurrentPosition(0);
  stepper0.setMaxSpeed(tuneSpeed);
  stepper0.setAcceleration(tuneAccel);
  stepper0.setEnablePin(6);
  stepper0.disableOutputs();

  //stepper1.setCurrentPosition(0);
  stepper1.setMaxSpeed(tuneSpeed);
  stepper1.setAcceleration(tuneAccel);
  stepper1.setEnablePin(7);
  stepper1.disableOutputs();

  //stepper2.setCurrentPosition(0);
  stepper2.setMaxSpeed(gantrySpeed);
  stepper2.setAcceleration(gantryAccel);
  stepper2.setEnablePin(8);
  stepper2.disableOutputs();

  //stepper3.setCurrentPosition(0);
  stepper3.setMaxSpeed(coilSpeed);
  stepper3.setAcceleration(coilAccel);
  stepper3.setEnablePin(2);
  stepper3.disableOutputs();

  //stepper4.setCurrentPosition(0);
  stepper4.setMaxSpeed(coilSpeed);
  stepper4.setAcceleration(coilAccel);
  stepper4.setEnablePin(3);
  stepper4.disableOutputs();

  //stepper5.setCurrentPosition(0);
  stepper5.setMaxSpeed(coilSpeed);
  stepper5.setAcceleration(coilAccel);
  stepper5.setEnablePin(4);
  stepper5.disableOutputs();

  //stepper6.setCurrentPosition(0);
  stepper6.setMaxSpeed(coilSpeed);
  stepper6.setAcceleration(coilAccel);
  stepper6.setEnablePin(5);
  stepper6.disableOutputs();
}

void stepperStop(int j) {
  mySteppers[j]->stop();
  mySteppers[j]->move(0);
  mySteppers[j]->disableOutputs();
}

void stepperMove(int j, long k) {
  mySteppers[j]->enableOutputs();
  mySteppers[j]->moveTo(k);
}

void stepperRMove(int j, int k) {
  int i;
  switch (j) {
  case 0: //tuner 1
    i = 0;
    break;
  case 1: //tuner 2
    i = 0;
    break;
  case 2: //gantry
    i = 1;
    break;
  default: //coils
    i = 2;
    break;
  }
  

  if (j == 2) {
    p = mySteppers[j]->currentPosition() + 2*k;
    thismin = minmax[i][0] - minmax[i][1];
    thismax = 2*minmax[i][1];
  }
  else {
    p = mySteppers[j]->currentPosition() + k;
    thismin = minmax[i][0];
    thismax = minmax[i][1];
  }

  if (p >= thismin && p <= thismax) {
    mySteppers[j]->enableOutputs();
    //mySteppers[j]->move(k);
    mySteppers[j]->runToNewPosition(p);
    stepperStop(j);
    //delay(1);
  }
}

void stepperAMove(int j, long k) {
    int i;
    switch (j) {
    case 0: //tuner 1
      i = 0;
      break;
    case 1: //tuner 2
      i = 0;
      break;
    case 2: //gantry
      i = 1;
      break;
    default: //coils
      i = 2;
      break;
  }

//  if (j == 2) {
//    k = k*2;
//  }
  
  if (k >= minmax[i][0] && k <= minmax[i][1]) {
    mySteppers[j]->enableOutputs();
    //mySteppers[j]->moveTo(k);
    mySteppers[j]->runToNewPosition(k);
    stepperStop(j);
    //delay(1);
  }
}

void sendPositions() {
  c.sendCmdStart(positions);
  for (int i = 0; i < stepnum; i++) {
    c.sendCmdBinArg(int(mySteppers[i]->currentPosition()));
  }
  c.sendCmdEnd();
  delay(1); //might need to be inside loop? 
}

void resetPositions() {
  for (int i = 0; i < stepnum; i++) {
    mySteppers[i]->setCurrentPosition(0);
  }
  delay(1);
}
  
void resetStepper(int j) {
  mySteppers[j]->setCurrentPosition(0);
  delay(1);
}

void setStepper(int j, long k) {
  mySteppers[j]->setCurrentPosition(k);
  delay(1);
}

void setaccel(int j, long k) {
  mySteppers[j]->setAcceleration(k);
}

void setspeed(int j, long k) {
    mySteppers[j]->setMaxSpeed(k);
}

void setMin(int j, long k) {
  minmax[j][0] = k;
}

void setMax(int j, long k) {
  minmax[j][1] = k;
}
