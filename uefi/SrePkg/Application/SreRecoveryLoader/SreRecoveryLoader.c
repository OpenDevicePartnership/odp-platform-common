/** @file
  SreRecoveryLoader - ESP-sourced counterpart to BpRecoveryLoader. Reads
  \ValidationOS.wim from the load volume into DRAM and registers it as
  a RAM disk via EFI_RAM_DISK_PROTOCOL (UEFI 2.5+). The WIM appears as
  a virtual block device on a new handle in the EFI handle database.

  Stages only; does not chainload bootmgfw against the new RAM disk.

  Copyright (c) Microsoft Corporation. All rights reserved.
  SPDX-License-Identifier: BSD-2-Clause-Patent
**/

#include <Uefi.h>
#include <Library/UefiLib.h>
#include <Library/UefiBootServicesTableLib.h>
#include <Library/UefiApplicationEntryPoint.h>
#include <Library/MemoryAllocationLib.h>
#include <Library/BaseMemoryLib.h>
#include <Library/PrintLib.h>
#include <Library/UefiRuntimeServicesTableLib.h>
#include <Protocol/LoadedImage.h>
#include <Protocol/SimpleFileSystem.h>
#include <Protocol/RamDisk.h>
#include <Protocol/DevicePath.h>
#include <Guid/FileInfo.h>
#include <Guid/GlobalVariable.h>

#define WIM_FILE_PATH       L"\\ValidationOS.wim"

//
// NVRAM result schema. Same vendor GUID as NvmeBpResult so a single Windows
// reader can dispatch on Magic. Magic = 'SREL'.
//
#define SRE_LOADER_MAGIC    0x4C455253  // 'SREL'
#define SRE_LOADER_VERSION  0x00010000  // v1.0

STATIC EFI_GUID  gNvmeBpResultGuid = {
  0x7B5A1F3E, 0x2D8C, 0x4A91, { 0xB6, 0xE3, 0xD8, 0xF2, 0xC9, 0xA4, 0xE1, 0x05 }
};

typedef enum {
  SRE_PHASE_INIT          = 0,
  SRE_PHASE_WIM_OPENED    = 1,
  SRE_PHASE_WIM_LOADED    = 2,
  SRE_PHASE_RAMDISK_REG   = 3,
  SRE_PHASE_DONE          = 4,
} SRE_PHASE;

#pragma pack(push, 1)
typedef struct {
  UINT32  Magic;
  UINT32  Version;
  UINT32  PhaseReached;
  UINT32  RamDiskRegistered;   // 1 if EFI_RAM_DISK_PROTOCOL.Register succeeded
  UINT64  WimFileBytes;
  UINT64  RamDiskBaseAddr;
  INT32   LastEfiStatus;       // low 32 of last failing EFI_STATUS
  UINT32  Pad;
} SRE_LOADER_RESULT;
#pragma pack(pop)

STATIC SRE_LOADER_RESULT  gResult;
STATIC EFI_HANDLE         gAppImageHandle = NULL;

STATIC
VOID
EFIAPI
WriteResultToNvRam (
  VOID
  )
{
  UINT32  Attrs = EFI_VARIABLE_NON_VOLATILE | EFI_VARIABLE_BOOTSERVICE_ACCESS | EFI_VARIABLE_RUNTIME_ACCESS;
  gResult.Magic   = SRE_LOADER_MAGIC;
  gResult.Version = SRE_LOADER_VERSION;
  (VOID)gRT->SetVariable (
                L"SreLoaderResult",
                &gNvmeBpResultGuid,
                Attrs,
                sizeof (gResult),
                &gResult
                );
}

//
// Read the WIM from the load volume into a freshly-allocated DRAM buffer
// aligned to 4 KiB (EFI Page allocations are page-aligned). Buffer must be
// 4 KiB-aligned to satisfy EFI_RAM_DISK_PROTOCOL.Register's alignment
// requirement (it expects an EFI_PHYSICAL_ADDRESS that, by spec, refers to
// a block-aligned region).
//
STATIC
EFI_STATUS
EFIAPI
LoadWimIntoPages (
  OUT EFI_PHYSICAL_ADDRESS  *OutPageAddr,
  OUT UINTN                 *OutPages,
  OUT UINT64                *OutFileBytes
  )
{
  EFI_STATUS                       Status;
  EFI_LOADED_IMAGE_PROTOCOL        *LoadedImage;
  EFI_SIMPLE_FILE_SYSTEM_PROTOCOL  *Sfs;
  EFI_FILE_PROTOCOL                *Root;
  EFI_FILE_PROTOCOL                *Wim;
  EFI_FILE_INFO                    *Info;
  UINTN                            InfoSize;
  UINT64                           FileBytes;
  UINTN                            Pages;
  EFI_PHYSICAL_ADDRESS             PageAddr;
  UINTN                            ReadSize;

  *OutPageAddr  = 0;
  *OutPages     = 0;
  *OutFileBytes = 0;

  Status = gBS->HandleProtocol (gAppImageHandle, &gEfiLoadedImageProtocolGuid, (VOID **)&LoadedImage);
  if (EFI_ERROR (Status)) return Status;
  Status = gBS->HandleProtocol (LoadedImage->DeviceHandle, &gEfiSimpleFileSystemProtocolGuid, (VOID **)&Sfs);
  if (EFI_ERROR (Status)) return Status;
  Status = Sfs->OpenVolume (Sfs, &Root);
  if (EFI_ERROR (Status)) return Status;
  Status = Root->Open (Root, &Wim, (CHAR16 *)WIM_FILE_PATH, EFI_FILE_MODE_READ, 0);
  Root->Close (Root);
  if (EFI_ERROR (Status)) {
    Print (L"  [wim] Open(%s): %r\n", WIM_FILE_PATH, Status);
    return Status;
  }

  gResult.PhaseReached = SRE_PHASE_WIM_OPENED;
  WriteResultToNvRam ();

  InfoSize = 0;
  Status = Wim->GetInfo (Wim, &gEfiFileInfoGuid, &InfoSize, NULL);
  if (Status != EFI_BUFFER_TOO_SMALL) {
    Wim->Close (Wim);
    return EFI_ERROR (Status) ? Status : EFI_DEVICE_ERROR;
  }
  Info = AllocateZeroPool (InfoSize);
  if (Info == NULL) {
    Wim->Close (Wim);
    return EFI_OUT_OF_RESOURCES;
  }
  Status = Wim->GetInfo (Wim, &gEfiFileInfoGuid, &InfoSize, Info);
  if (EFI_ERROR (Status)) {
    FreePool (Info);
    Wim->Close (Wim);
    return Status;
  }
  FileBytes = Info->FileSize;
  FreePool (Info);

  Pages = (UINTN)((FileBytes + EFI_PAGE_SIZE - 1) / EFI_PAGE_SIZE);
  Status = gBS->AllocatePages (AllocateAnyPages, EfiBootServicesData, Pages, &PageAddr);
  if (EFI_ERROR (Status)) {
    Print (L"  [wim] AllocatePages(%lu pages = %lu MiB): %r\n",
           (UINT64)Pages, (UINT64)(Pages * EFI_PAGE_SIZE / SIZE_1MB), Status);
    Wim->Close (Wim);
    return Status;
  }
  // Zero the page tail so any read past file end returns deterministic data.
  ZeroMem ((VOID *)(UINTN)PageAddr, Pages * EFI_PAGE_SIZE);

  ReadSize = (UINTN)FileBytes;
  Status = Wim->Read (Wim, &ReadSize, (VOID *)(UINTN)PageAddr);
  Wim->Close (Wim);
  if (EFI_ERROR (Status) || (UINT64)ReadSize != FileBytes) {
    gBS->FreePages (PageAddr, Pages);
    return EFI_ERROR (Status) ? Status : EFI_DEVICE_ERROR;
  }

  *OutPageAddr  = PageAddr;
  *OutPages     = Pages;
  *OutFileBytes = FileBytes;
  return EFI_SUCCESS;
}

EFI_STATUS
EFIAPI
EntryPoint (
  IN EFI_HANDLE        ImageHandle,
  IN EFI_SYSTEM_TABLE  *SystemTable
  )
{
  EFI_STATUS  Status;

  gAppImageHandle = ImageHandle;
  ZeroMem (&gResult, sizeof (gResult));
  Print (L"SreRecoveryLoader v1.0 - load \\ValidationOS.wim from ESP, register as RAM disk\n\n");

  // Clear BootNext; BootOrder selects the next boot target. Below: drop
  // this app's 0x4243 entry from BootOrder so it doesn't re-dispatch itself.
  UINT32  Attrs = EFI_VARIABLE_NON_VOLATILE | EFI_VARIABLE_BOOTSERVICE_ACCESS | EFI_VARIABLE_RUNTIME_ACCESS;
  (VOID)gRT->SetVariable (L"BootNext", &gEfiGlobalVariableGuid, 0, 0, NULL);
  Print (L"[safety] BootNext cleared (BootOrder will select next boot)\n");

  UINTN  BoSize = 0;
  EFI_STATUS  BoStatus = gRT->GetVariable (L"BootOrder", &gEfiGlobalVariableGuid, NULL, &BoSize, NULL);
  if (BoStatus == EFI_BUFFER_TOO_SMALL && BoSize >= 2) {
    UINT8  *BoBuf = AllocateZeroPool (BoSize);
    UINT8  *NewBo = AllocateZeroPool (BoSize);
    if (BoBuf != NULL && NewBo != NULL) {
      BoStatus = gRT->GetVariable (L"BootOrder", &gEfiGlobalVariableGuid, NULL, &BoSize, BoBuf);
      if (!EFI_ERROR (BoStatus)) {
        UINTN  NewSize = 0;
        for (UINTN I = 0; I + 1 < BoSize; I += 2) {
          UINT16  Idx = *(UINT16 *)(BoBuf + I);
          if (Idx != 0x4243) {
            *(UINT16 *)(NewBo + NewSize) = Idx;
            NewSize += 2;
          }
        }
        if (NewSize > 0 && NewSize != BoSize) {
          (VOID)gRT->SetVariable (L"BootOrder", &gEfiGlobalVariableGuid, Attrs, NewSize, NewBo);
          Print (L"[safety] BootOrder cleaned (removed 0x4243)\n");
        }
      }
    }
    if (BoBuf != NULL) FreePool (BoBuf);
    if (NewBo != NULL) FreePool (NewBo);
  }
  gBS->SetWatchdogTimer (0, 0, 0, NULL);
  Print (L"[safety] BDS watchdog disabled\n\n");

  gResult.PhaseReached = SRE_PHASE_INIT;
  WriteResultToNvRam ();

  // Step 1: load the WIM into DRAM.
  EFI_PHYSICAL_ADDRESS  WimAddr  = 0;
  UINTN                 WimPages = 0;
  UINT64                WimBytes = 0;
  Print (L"[step 1] Loading %s from ESP...\n", WIM_FILE_PATH);
  Status = LoadWimIntoPages (&WimAddr, &WimPages, &WimBytes);
  if (EFI_ERROR (Status)) {
    Print (L"  FAILED: %r\n", Status);
    gResult.LastEfiStatus = (INT32)Status;
    WriteResultToNvRam ();
    goto Reboot;
  }
  Print (L"  loaded %lu bytes (%lu MiB) at phys 0x%lx (%lu pages)\n",
         WimBytes, WimBytes / SIZE_1MB, (UINT64)WimAddr, (UINT64)WimPages);
  gResult.PhaseReached  = SRE_PHASE_WIM_LOADED;
  gResult.WimFileBytes  = WimBytes;
  gResult.RamDiskBaseAddr = (UINT64)WimAddr;
  WriteResultToNvRam ();

  // Step 2: register as RAM disk via EFI_RAM_DISK_PROTOCOL (UEFI 2.5+,
  // produced by RamDiskDxe).
  Print (L"\n[step 2] Locating EFI_RAM_DISK_PROTOCOL...\n");
  EFI_RAM_DISK_PROTOCOL  *RamDisk = NULL;
  Status = gBS->LocateProtocol (&gEfiRamDiskProtocolGuid, NULL, (VOID **)&RamDisk);
  if (EFI_ERROR (Status)) {
    Print (L"  LocateProtocol(RamDisk): %r (platform missing RamDiskDxe?)\n", Status);
    gResult.LastEfiStatus = (INT32)Status;
    WriteResultToNvRam ();
    goto Reboot;
  }
  Print (L"  found at %p\n", RamDisk);

  // Register the WIM as a RAM disk. The "Virtual" disk type GUID
  // (EFI_VIRTUAL_DISK_GUID) creates a generic RAMDISK that the firmware will
  // expose as a block device via EFI_BLOCK_IO_PROTOCOL on a new handle.
  EFI_DEVICE_PATH_PROTOCOL  *RamDp = NULL;
  Print (L"[step 3] Registering WIM as RAM disk (Virtual disk type GUID)...\n");
  Status = RamDisk->Register (
                      (UINT64)WimAddr,
                      WimBytes,
                      &gEfiVirtualDiskGuid,
                      NULL,        // ParentDevicePath; NULL = root of EFI handle tree
                      &RamDp
                      );
  if (EFI_ERROR (Status)) {
    Print (L"  Register: %r\n", Status);
    gResult.LastEfiStatus = (INT32)Status;
    WriteResultToNvRam ();
    goto Reboot;
  }
  Print (L"  OK - RAM disk registered. New device path at %p\n", RamDp);
  gResult.RamDiskRegistered = 1;
  gResult.PhaseReached      = SRE_PHASE_RAMDISK_REG;
  WriteResultToNvRam ();

  Print (L"\n*** RAM disk staged. ***\n");
  gResult.PhaseReached = SRE_PHASE_DONE;
  WriteResultToNvRam ();

Reboot:
  // Clear BootNext; BootOrder selects the next boot target.
  (VOID)gRT->SetVariable (L"BootNext", &gEfiGlobalVariableGuid, 0, 0, NULL);
  Print (L"\n[exit] resetting...\n");
  gRT->ResetSystem (EfiResetCold, EFI_SUCCESS, 0, NULL);
  return EFI_SUCCESS;
}
