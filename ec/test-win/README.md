# ec-test-win

## Overview
Windows native components for testing EC interfaces via ACPI. Includes a user-mode library, CLI test application, and a kernel-mode driver.

## Structure
```
exe/  - User mode CLI (ectest.exe) to call and evaluate ACPI functions
inc/  - Shared header files between user-mode and kernel-mode components
kmdf/ - Kernel mode driver (ectest.sys) that evaluates ACPI methods
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

Compile ectest.sys from `kmdf/`:
```
msbuild /p:Configuration=Release /p:Platform=ARM64
```

Compile ectest.exe from `exe/`:
```
msbuild /p:Configuration=Release /p:Platform=ARM64
```

## Installing the Driver
After recompiling ACPI and booting your device, install the driver and run validation tests.

Copy the following files to a thumbdrive or location on the target:
```
ec\test-win\exe\arm64\Debug\ectest.exe
ec\test-win\kmdf\arm64\Debug\ectest_kmdf\*
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
