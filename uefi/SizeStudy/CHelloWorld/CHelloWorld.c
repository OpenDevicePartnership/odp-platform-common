////
// SizeStudy baseline DXE driver that prints a debug message and exits, written in C.
//
// Copyright (c) Microsoft Corporation. All rights reserved.
// SPDX-License-Identifier: MIT
//
#include <Uefi.h>
#include <Library/DebugLib.h>

EFI_STATUS
EFIAPI
CHelloWorldEntryPoint (
  IN EFI_HANDLE        ImageHandle,
  IN EFI_SYSTEM_TABLE  *SystemTable
  )
{
  (VOID)ImageHandle;
  (VOID)SystemTable;

  DEBUG ((DEBUG_INFO, "[XXXXXXXX] Test C Driver\n"));
  return EFI_SUCCESS;
}
