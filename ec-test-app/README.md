# ec-test-app

## Overview
This repo includes code for testing EC interface and sample ACPI code.
See [ODP Documentation](https://github.com/OpenDevicePartnership/documentation/tree/main/guide_book) for details on EC specification.

## Features and Status
```
exe   - User mode CLI to call and evaluate ACPI functions to test EC interfaces
inc   - Shared header files between test app and kernel mode driver
kmdf  - Kernel mode driver that test app communicates with to evaluate ACPI methods
rust  - Demo application that uses Ratatui to display GUI for demoing EC features
uefi  - Sample EC ACPI tables for secure EC. Includes thermal and notification examples
```

## Environment Setup
Download recent EWDK and mount ISO
```
cd BuildEnv
setupbuildenv.cmd x86_arm64
```

## Compilation Instructions

To compile eclib.lib and eclib.dll from lib folder in cmd with environment setup run
`msbuild /p:Configuration=Release /p:Platform=ARM64`

To compile ectest.sys from kmdf folder in cmd with environment setup run
`msbuild /p:Configuration=Release /p:Platform=ARM64`

To compile ectest.exe from exe folder in cmd with environment setup run
`msbuild /p:Configuration=Release /p:Platform=ARM64`

To compile ec_demo.exe from rust folder in cmd with environment setup run after compiling lib
`cargo build --release --target=aarch64-pc-windows-msvc`

The driver needs ACPI entries to load and execute. Sample ACPI for loading the driver and stubbed implementation of fan is available in acpi folder.
If your ACPI already has fan and battery definitions you can just include ectest and add methods to expose the ACPI functions you want to test.

## Installing the driver and Running ectest.exe
After recompiling ACPI and booting your device you will need to install the driver and run the validation tests.
Copy the following files from output folders to a thumbdrive or location on the target to test:
```
ec-test-app\exe\arm64\Debug\ectest.exe
ec-test-app\kmdf\arm64\Debug\ectest_kmdf\*
<WDKROOT>\Program Files\Windows Kits\10\Tools\10.0.26100.0\arm64\devcon.exe
```

You can install the driver through device manager as well, but easier to use devcon in case you need to automate or you can DISM it into your image as well.
From admin command prompt on your target device cd to location of install files:
```
cd e:\install
devcon remove ACPI\ECTST0001
devcon install ectest.inf ACPI\ECTST0001
```

You will get a pop-up saying that the certificate is not tested and you can choose to install anyways. Otherwise if you install certificate in your certstore under trusted root you won't get this.
To run the test you can simply use the following
```
E:\>ectest -acpi \_SB.ECT0.TFST
Found matching Class GUID: ACPI\ECTST0001\0
\\.\GLOBALROOT\Device\00000016
DevicePath: \\.\GLOBALROOT\Device\00000016
Opened device successfully

Calling DeviceIoControl EVAL_ACPI_METHOD: \_SB.ECT0.TFST
ACPI Method:
  Signature: 0x426f6541
  Length: 0x30
  Count: 0x3
    Argument[0]:
    Integer Value: 0x0
    Argument[1]:
    Integer Value: 0x1
    Argument[2]:
    Integer Value: 0x2
ACPI Raw Output:
 0x41 0x65 0x6f 0x42 0x30 0x0 0x0 0x0 0x3 0x0 0x0 0x0 0x0 0x0 0x8 0x0 0x0 0x0 0x0 0x0 0x0 0x0 0x0 0x0 0x0 0x0 0x8 0x0 0x1 0x0 0x0 0x0 0x0 0x0 0x0 0x0 0x0 0x0 0x8 0x0 0x2 0x0 0x0 0x0 0x0 0x0 0x0 0x0
```

You can add more functions in the ectest.asl file to add more test functions to your ACPI that calls other ACPI methods and just pass in the name of your new test method on the command line.
