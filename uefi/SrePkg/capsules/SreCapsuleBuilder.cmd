@echo off
py "%~dp0SreCapsuleBuilder.py" ^
  --wim-path "%~dp0..\ValidationFat32Partition.img" ^
  --signer-path "%~dp0..\..\..\..\Common\SURFACE_INTERNAL\Plugin\AutoSignConsoleHelper\Devices.Tool.AutoSignConsole_extdep\content\any\AutosignConsole.exe" ^
  --capsule-version 0x20002122 ^
  --lsv 0 ^
  --monotonic-count 1 ^
  --esrt-guid 9777ff5d-3c53-4316-9af3-b99fb9c60fb5
