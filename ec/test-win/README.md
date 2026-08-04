# ec-test-win

## Overview
Windows native components for testing EC interfaces via ACPI. Includes a user-mode library and CLI test application that talk to the `ectest.sys` KMDF driver.

> The `ectest.sys` KMDF driver now lives in the [odp-windows-drivers](https://github.com/OpenDevicePartnership/odp-windows-drivers) repository (`drivers/acpi`). Build and install it from there. The IOCTL/struct contract shared with the driver is kept here in `inc/ectest.h`.

## Structure
```
exe/  - User mode CLI (ectest.exe) to call and evaluate ACPI functions
inc/  - Shared header files (incl. the IOCTL contract used to talk to ectest.sys)
lib/  - User mode library (eclib) bridging user apps to the KMDF driver
dep/  - External dependencies (WIL git submodule)
```

## Environment Setup
Download a recent EWDK and mount the ISO:
```
cd BuildEnv
setupbuildenv.cmd x86_arm64
```

## Compilation
From cmd with EWDK environment setup:

Compile eclib.lib and eclib.dll from `lib/`:
```
msbuild /p:Configuration=Release /p:Platform=ARM64
```

Compile ectest.exe from `exe/`:
```
msbuild /p:Configuration=Release /p:Platform=ARM64
```

The `ectest.sys` KMDF driver is built from the [odp-windows-drivers](https://github.com/OpenDevicePartnership/odp-windows-drivers) repository (`drivers/acpi`).

## Installing the Driver
After recompiling ACPI and booting your device, install the driver and run validation tests. Build the `ectest.sys` driver from the [odp-windows-drivers](https://github.com/OpenDevicePartnership/odp-windows-drivers) repository (`drivers/acpi`).

Copy the following files to a thumbdrive or location on the target:
```
ec\test-win\exe\arm64\Debug\ectest.exe
<odp-windows-drivers build output>\ectest_kmdf\*
<WDKROOT>\Program Files\Windows Kits\10\Tools\10.0.26100.0\arm64\devcon.exe
```

From an admin command prompt on your target device:
```
devcon remove ACPI\ECTST0001
devcon install ectest.inf ACPI\ECTST0001
```

You will get a pop-up saying the certificate is not tested — you can choose to install anyways, or install the certificate in your certstore under trusted root to avoid it.

## Running ectest.exe
```
ectest -acpi \_SB.ECT0.TFST
```

The driver needs ACPI entries to load and execute. Sample ACPI for loading the driver and stubbed implementation of fan is available in the acpi folder. If your ACPI already has fan and battery definitions, you can just include ectest and add methods to expose the ACPI functions you want to test.

You can add more functions in the ectest.asl file to add more test functions to your ACPI that call other ACPI methods, and pass the name of your new test method on the command line.
