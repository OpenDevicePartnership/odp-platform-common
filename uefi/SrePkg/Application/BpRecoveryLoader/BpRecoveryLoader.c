/** @file
  BpRecoveryLoader — reads the recovery WIM from NVMe Boot Partition 1
  via Get Log Page LID=0x15 (NVMe Base Spec 2.3 §5.14.1.16) into DRAM,
  registers it as a RAM disk via EFI_RAM_DISK_PROTOCOL (UEFI 2.5+), and
  chainloads \EFI\Boot\bootx64.efi from the resulting FAT volume.

  Assumes the WIM has already been committed into BP1: vendor unlock
  (FID 0x85, BPxWPS=001b) + Firmware Image Download with the Kioxia
  CDW12=1 BP route hint + Firmware Commit CA=110b BPID=1.

  Two controller-specific behaviors are accommodated:
   - The LID 0x15 response prepends a 16-byte header before the BP
     image bytes (4B LID echo + 4B BPSZ + 8B reserved). Reads issue at
     LPOL = 16 + bp_offset to skip the header.
   - BPSIZE on this part is 1 GiB (= BPINFO.BPSZ * 128 KiB). The full
     1 GiB is allocated and read; trailing zero padding past the WIM
     image is harmless (Windows Boot Manager parses the WIM header and
     ignores everything past the declared image size).

  Copyright (c) Microsoft Corporation. All rights reserved.
  SPDX-License-Identifier: BSD-2-Clause-Patent
**/

#include <Uefi.h>
#include <Library/UefiBootServicesTableLib.h>
#include <Library/MemoryAllocationLib.h>
#include <Library/BaseMemoryLib.h>
#include <Library/DebugLib.h>
#include <Library/UefiRuntimeServicesTableLib.h>
#include <Library/DevicePathLib.h>
#include <Protocol/NvmExpressPassthru.h>
#include <Protocol/RamDisk.h>
#include <Protocol/DevicePath.h>
#include <Protocol/SimpleFileSystem.h>

#define TARGET_BPID                  1
#define BPSIZE_BYTES                 (1024ULL * SIZE_1MB)  // 1 GiB on this part; could probe BPINFO if generalized
#define LID_BP_HEADER_BYTES          16                    // controller prepends this many bytes to the LID 0x15 response

#define NVME_ADMIN_GET_LOG_PAGE      0x02
#define NVME_ADMIN_IDENTIFY          0x06
#define LID_BOOT_PARTITION           0x15

// LID 0x15 chunk size bounds. Actual chunk picked at runtime by
// ProbeMaxTransfer based on the controller's MDTS. Floor and ceiling
// clamp the probe result.
#define READ_CHUNK_MIN               (64   * 1024)
#define READ_CHUNK_MAX               (512  * 1024)
#define READ_CHUNK_DEFAULT           (64   * 1024)  // used if MDTS probe fails

// Emit a progress log line every N bytes read from BP.
#define READ_PROGRESS_INTERVAL       (16ULL * SIZE_1MB)

// Recovery payload boot file — UEFI fallback removable-media convention.
// The customer's bootable layout (Windows bootmgfw+BCD+WIM, systemd-boot,
// custom EFI loader) ships inside a FAT32 image stored in BP; we chainload
// whatever sits at this path on that volume.
#define CHAINLOAD_FILE_PATH          L"\\EFI\\Boot\\bootx64.efi"

// NVRAM result schema. Same vendor GUID as NvmeBpResult / SreLoaderResult
// so a single Windows reader can dispatch on Magic. Magic = 'BPLX'.
#define BPL_RESULT_MAGIC             0x584C5042  // 'BPLX' (little-endian)
#define BPL_RESULT_VERSION           0x00000001  // v0.1

STATIC EFI_GUID  gResultGuid = {
  0x7B5A1F3E, 0x2D8C, 0x4A91, { 0xB6, 0xE3, 0xD8, 0xF2, 0xC9, 0xA4, 0xE1, 0x05 }
};

typedef enum {
  BPL_PHASE_INIT                = 0,
  BPL_PHASE_PASSTHRU_LOCATED    = 1,
  BPL_PHASE_BP_LOADED           = 2,
  BPL_PHASE_RAMDISK_REGISTERED  = 3,
  BPL_PHASE_DONE                = 4,
} BPL_PHASE;

#pragma pack(push, 1)
typedef struct {
  UINT32  Magic;                // BPL_RESULT_MAGIC
  UINT32  Version;              // BPL_RESULT_VERSION
  UINT32  PhaseReached;         // BPL_PHASE_*
  UINT32  RamDiskRegistered;    // 1 if EFI_RAM_DISK_PROTOCOL.Register succeeded
  UINT32  ChainloadAttempted;   // 1 if we tried to locate FAT + LoadImage
  UINT32  ChainloadImageLoaded; // 1 if LoadImage succeeded (so StartImage was called and then returned)
  UINT64  BpBytesRead;          // total bytes read from BP into DRAM
  UINT64  RamDiskBaseAddr;      // physical address of the DRAM-backed RAM disk
  INT32   LastEfiStatus;        // low 32 of last failing EFI_STATUS
  UINT32  LastNvmeStatus;       // (SCT << 8) | SC of last failing NVMe completion
  UINT8   First32Bytes[32];     // first 32 bytes of BP1 content (forensics)
} BPL_RESULT;
#pragma pack(pop)

STATIC BPL_RESULT  gResult;

STATIC
VOID
EFIAPI
WriteResultToNvRam (
  VOID
  )
{
  UINT32  Attrs = EFI_VARIABLE_NON_VOLATILE | EFI_VARIABLE_BOOTSERVICE_ACCESS | EFI_VARIABLE_RUNTIME_ACCESS;
  gResult.Magic   = BPL_RESULT_MAGIC;
  gResult.Version = BPL_RESULT_VERSION;
  (VOID)gRT->SetVariable (
                L"BpLoaderResult",
                &gResultGuid,
                Attrs,
                sizeof (gResult),
                &gResult
                );
}

//
// Probe controller MDTS via Identify Controller (CNS=0x01) and derive the
// largest LID 0x15 chunk size we can use. MDTS lives at byte offset 77 of
// the Identify Controller response (NVMe Base Spec §5.17.2.1):
//   value 0 = no limit; otherwise max transfer = 2^MDTS * MPSMIN.
// We assume MPSMIN = 4 KiB (the NVMe minimum, the typical default, and what
// EDK2's NvmExpressPassThru uses on this controller). Clamp the result to
// [READ_CHUNK_MIN, READ_CHUNK_MAX] so we don't blow allocation budgets or
// trip a smaller-than-MDTS internal cap in the PassThru implementation.
// Returns READ_CHUNK_DEFAULT on probe failure.
//
STATIC
UINTN
EFIAPI
ProbeMaxTransfer (
  IN  EFI_NVM_EXPRESS_PASS_THRU_PROTOCOL  *PassThru
  )
{
  EFI_NVM_EXPRESS_PASS_THRU_COMMAND_PACKET  Packet;
  EFI_NVM_EXPRESS_COMMAND                   Cmd;
  EFI_NVM_EXPRESS_COMPLETION                Completion;
  STATIC UINT8                              IdentBuf[4096];
  UINTN                                     Chunk;
  EFI_STATUS                                Status;

  ZeroMem (IdentBuf, sizeof (IdentBuf));
  ZeroMem (&Cmd, sizeof (Cmd));
  ZeroMem (&Completion, sizeof (Completion));
  Cmd.Cdw0.Opcode = NVME_ADMIN_IDENTIFY;
  Cmd.Cdw10       = 0x01;  // CNS = 1 (Identify Controller)
  Cmd.Flags       = CDW10_VALID;

  ZeroMem (&Packet, sizeof (Packet));
  Packet.CommandTimeout = 5ULL * 10000000ULL;
  Packet.TransferBuffer = IdentBuf;
  Packet.TransferLength = sizeof (IdentBuf);
  Packet.QueueType      = NVME_ADMIN_QUEUE;
  Packet.NvmeCmd        = &Cmd;
  Packet.NvmeCompletion = &Completion;

  Status = PassThru->PassThru (PassThru, 0, &Packet, NULL);
  if (EFI_ERROR (Status)) {
    DEBUG ((DEBUG_WARN, "[BpRecoveryLoader] Identify Controller failed: %r; falling back to %u KiB chunk\n",
            Status, (UINT32)(READ_CHUNK_DEFAULT / 1024)));
    return READ_CHUNK_DEFAULT;
  }

  UINT8  Mdts = IdentBuf[77];
  if (Mdts == 0) {
    Chunk = READ_CHUNK_MAX;  // "no limit" -> our self-imposed ceiling
  } else {
    UINT64  MaxTransfer = ((UINT64)1 << Mdts) * 4096;
    Chunk = (MaxTransfer >= READ_CHUNK_MAX) ? READ_CHUNK_MAX
          : (MaxTransfer <= READ_CHUNK_MIN) ? READ_CHUNK_MIN
          : (UINTN)MaxTransfer;
  }

  DEBUG ((DEBUG_INFO, "[BpRecoveryLoader] Identify Controller: MDTS=%u -> chunk %u KiB\n",
          Mdts, (UINT32)(Chunk / 1024)));
  return Chunk;
}

//
// Get Log Page LID=0x15 (Boot Partition log) read.
// CDW10: NUMDL<<16 | LSP(=BPID)<<8 | 0x15.
//
STATIC
EFI_STATUS
EFIAPI
GetBpLogPage (
  IN  EFI_NVM_EXPRESS_PASS_THRU_PROTOCOL  *PassThru,
  IN  UINT8                                Bpid,
  IN  UINT64                               OffsetBytes,
  IN  UINTN                                Bytes,
  OUT UINT8                               *Buffer,
  OUT UINT32                              *Sct,
  OUT UINT32                              *Sc
  )
{
  EFI_NVM_EXPRESS_PASS_THRU_COMMAND_PACKET  Packet;
  EFI_NVM_EXPRESS_COMMAND                   Cmd;
  EFI_NVM_EXPRESS_COMPLETION                Completion;
  UINT32                                    StatusField;
  UINT32                                    NumD;

  ZeroMem (&Cmd, sizeof (Cmd));
  ZeroMem (&Completion, sizeof (Completion));
  ZeroMem (Buffer, Bytes);

  NumD = (UINT32)((Bytes / 4) - 1);

  Cmd.Cdw0.Opcode = NVME_ADMIN_GET_LOG_PAGE;
  Cmd.Cdw10       = ((NumD & 0xFFFFu) << 16)
                  | (((UINT32)Bpid & 0x7Fu) << 8)
                  | LID_BOOT_PARTITION;
  Cmd.Cdw11       = (NumD >> 16) & 0xFFFFu;
  Cmd.Cdw12       = (UINT32)(OffsetBytes & 0xFFFFFFFFull);            // LPOL
  Cmd.Cdw13       = (UINT32)((OffsetBytes >> 32) & 0xFFFFFFFFull);    // LPOU
  Cmd.Flags       = CDW10_VALID | CDW11_VALID | CDW12_VALID | CDW13_VALID;

  ZeroMem (&Packet, sizeof (Packet));
  Packet.CommandTimeout = 10ULL * 10000000ULL;
  Packet.TransferBuffer = Buffer;
  Packet.TransferLength = (UINT32)Bytes;
  Packet.QueueType      = NVME_ADMIN_QUEUE;
  Packet.NvmeCmd        = &Cmd;
  Packet.NvmeCompletion = &Completion;

  EFI_STATUS  Status = PassThru->PassThru (PassThru, 0, &Packet, NULL);
  StatusField = (Completion.DW3 >> 17) & 0x7FFF;
  *Sc  = (StatusField >> 0) & 0xFF;
  *Sct = (StatusField >> 8) & 0x7;
  return Status;
}

//
// Read BpSize bytes of BP `BpId` into a fresh DRAM allocation. The log
// page response on this controller starts with a 16-byte header, so we
// issue reads at LPOL = LID_BP_HEADER_BYTES + bp_offset to land pure BP
// image bytes directly into the destination buffer with no per-chunk
// copy/skip.
//
STATIC
EFI_STATUS
EFIAPI
LoadBpIntoPages (
  IN  EFI_NVM_EXPRESS_PASS_THRU_PROTOCOL  *PassThru,
  IN  UINT8                                BpId,
  IN  UINT64                               BpSize,
  IN  UINTN                                ChunkBytes,
  OUT EFI_PHYSICAL_ADDRESS                *OutPageAddr,
  OUT UINTN                               *OutPages,
  OUT UINT64                              *OutBytesRead
  )
{
  EFI_STATUS            Status;
  EFI_PHYSICAL_ADDRESS  PageAddr = 0;
  UINTN                 Pages;

  *OutPageAddr  = 0;
  *OutPages     = 0;
  *OutBytesRead = 0;

  Pages = (UINTN)((BpSize + EFI_PAGE_SIZE - 1) / EFI_PAGE_SIZE);
  Status = gBS->AllocatePages (AllocateAnyPages, EfiBootServicesData, Pages, &PageAddr);
  if (EFI_ERROR (Status)) {
    DEBUG ((DEBUG_ERROR, "[BpRecoveryLoader] AllocatePages(%lu pages = %lu MiB): %r\n",
            (UINT64)Pages, (UINT64)(Pages * EFI_PAGE_SIZE / SIZE_1MB), Status));
    return Status;
  }
  ZeroMem ((VOID *)(UINTN)PageAddr, Pages * EFI_PAGE_SIZE);

  UINT8  *Dest       = (UINT8 *)(UINTN)PageAddr;
  UINT64  BpOff      = 0;
  UINTN   ChunkIndex = 0;

  while (BpOff < BpSize) {
    UINTN  ReadBytes = ChunkBytes;
    if (BpOff + ReadBytes > BpSize) {
      ReadBytes = (UINTN)(BpSize - BpOff);
    }
    UINT64  LPOL = (UINT64)LID_BP_HEADER_BYTES + BpOff;
    UINT32  Sct = 0, Sc = 0;
    Status = GetBpLogPage (PassThru, BpId, LPOL, ReadBytes, Dest + BpOff, &Sct, &Sc);
    if (EFI_ERROR (Status) || Sct != 0 || Sc != 0) {
      DEBUG ((DEBUG_ERROR, "[BpRecoveryLoader] chunk #%u BP off %lu (LPOL=%lu, %lu bytes) FAILED: %r SCT=%u SC=0x%02x\n",
              (UINT32)ChunkIndex, BpOff, LPOL, (UINT64)ReadBytes, Status, Sct, Sc));
      gResult.LastNvmeStatus = (Sct << 8) | Sc;
      gBS->FreePages (PageAddr, Pages);
      return EFI_ERROR (Status) ? Status : EFI_DEVICE_ERROR;
    }
    BpOff += ReadBytes;
    ChunkIndex++;
    if ((BpOff & (READ_PROGRESS_INTERVAL - 1)) == 0) {
      DEBUG ((DEBUG_INFO, "[BpRecoveryLoader] %lu MiB read\n", BpOff / SIZE_1MB));
    }
  }

  // Capture first 32 bytes for NVRAM diagnostics.
  CopyMem (gResult.First32Bytes, Dest, 32);
  DEBUG ((DEBUG_INFO, "[BpRecoveryLoader] %lu MiB total\n", BpOff / SIZE_1MB));

  *OutPageAddr  = PageAddr;
  *OutPages     = Pages;
  *OutBytesRead = BpOff;
  return EFI_SUCCESS;
}

// Core SRE flow: read BP1 -> DRAM -> register as RAM disk -> chainload
// \EFI\Boot\bootx64.efi from the resulting FAT volume. Does NOT manage
// BootNext or reset itself; the caller owns boot policy.
//
// Returns:
//   EFI_SUCCESS only if the chainloaded image's StartImage returned
//                 cleanly back to us;
//   any other EFI_STATUS means an earlier step failed and chainload
//                 did not happen.
//
STATIC
EFI_STATUS
EFIAPI
RunSreFlow (
  IN EFI_HANDLE  ImageHandle
  )
{
  EFI_STATUS  Status;

  ZeroMem (&gResult, sizeof (gResult));
  DEBUG ((DEBUG_INFO, "[BpRecoveryLoader] enter RunSreFlow (BPID=%u)\n", TARGET_BPID));

  // Disable BDS watchdog. BP read alone is several seconds; BDS arms a
  // 5-minute watchdog around boots and we don't want to race it.
  gBS->SetWatchdogTimer (0, 0, 0, NULL);

  gResult.PhaseReached = BPL_PHASE_INIT;
  WriteResultToNvRam ();

  // ConnectAll. Priority-boot dispatch happens before BdsConnectAll, so
  // NVMe (and anything else we LocateProtocol below) hasn't been bound to
  // its parent controller yet. Run it ourselves so LocateProtocol can find
  // the producer. Harmless if already connected.
  {
    UINTN       HandleCount = 0;
    EFI_HANDLE  *Handles    = NULL;
    if (!EFI_ERROR (gBS->LocateHandleBuffer (AllHandles, NULL, NULL, &HandleCount, &Handles))) {
      for (UINTN i = 0; i < HandleCount; i++) {
        (VOID)gBS->ConnectController (Handles[i], NULL, NULL, TRUE);
      }
      gBS->FreePool (Handles);
      DEBUG ((DEBUG_INFO, "[BpRecoveryLoader] ConnectAll: %u handles\n", (UINT32)HandleCount));
    }
  }

  // Step 1: locate NVMe Pass-Thru.
  DEBUG ((DEBUG_INFO, "[BpRecoveryLoader] step 1 - LocateProtocol(NvmExpressPassThru)\n"));
  EFI_NVM_EXPRESS_PASS_THRU_PROTOCOL  *PassThru = NULL;
  Status = gBS->LocateProtocol (&gEfiNvmExpressPassThruProtocolGuid, NULL, (VOID **)&PassThru);
  if (EFI_ERROR (Status)) {
    DEBUG ((DEBUG_ERROR, "[BpRecoveryLoader] step 1 FAIL: %r\n", Status));
    gResult.LastEfiStatus = (INT32)Status;
    WriteResultToNvRam ();
    return Status;
  }
  DEBUG ((DEBUG_INFO, "[BpRecoveryLoader] step 1 OK (PassThru=%p)\n", PassThru));
  gResult.PhaseReached = BPL_PHASE_PASSTHRU_LOCATED;
  WriteResultToNvRam ();

  // Step 2: probe MDTS then stream BPSIZE bytes from BP1 via LID 0x15 into DRAM.
  UINTN  ChunkBytes = ProbeMaxTransfer (PassThru);
  DEBUG ((DEBUG_INFO, "[BpRecoveryLoader] step 2 - reading %lu MiB from BP%u via LID 0x15 (LPOL base = %u, chunk = %u KiB)\n",
          BPSIZE_BYTES / SIZE_1MB, TARGET_BPID, LID_BP_HEADER_BYTES, (UINT32)(ChunkBytes / 1024)));
  EFI_PHYSICAL_ADDRESS  PageAddr  = 0;
  UINTN                 Pages     = 0;
  UINT64                BytesRead = 0;
  Status = LoadBpIntoPages (PassThru, TARGET_BPID, BPSIZE_BYTES, ChunkBytes, &PageAddr, &Pages, &BytesRead);
  if (EFI_ERROR (Status)) {
    DEBUG ((DEBUG_ERROR, "[BpRecoveryLoader] step 2 FAIL: %r\n", Status));
    gResult.LastEfiStatus = (INT32)Status;
    WriteResultToNvRam ();
    return Status;
  }
  DEBUG ((DEBUG_INFO, "[BpRecoveryLoader] step 2 OK (%lu bytes -> phys 0x%lx)\n", BytesRead, (UINT64)PageAddr));
  gResult.BpBytesRead     = BytesRead;
  gResult.RamDiskBaseAddr = (UINT64)PageAddr;
  gResult.PhaseReached    = BPL_PHASE_BP_LOADED;
  WriteResultToNvRam ();

  // BP write protection (lock/unlock) is owned by the FMP capsule flow.
  // SRE boot is read-only and intentionally does NOT touch BPWPS.

  // Step 3: register the DRAM region as a RAM disk via EFI_RAM_DISK_PROTOCOL.
  DEBUG ((DEBUG_INFO, "[BpRecoveryLoader] step 3 - LocateProtocol(RamDisk)\n"));
  EFI_RAM_DISK_PROTOCOL  *RamDisk = NULL;
  Status = gBS->LocateProtocol (&gEfiRamDiskProtocolGuid, NULL, (VOID **)&RamDisk);
  if (EFI_ERROR (Status)) {
    DEBUG ((DEBUG_ERROR, "[BpRecoveryLoader] step 3 FAIL - LocateProtocol(RamDisk): %r (platform missing RamDiskDxe?)\n", Status));
    gResult.LastEfiStatus = (INT32)Status;
    WriteResultToNvRam ();
    return Status;
  }

  EFI_DEVICE_PATH_PROTOCOL  *RamDp = NULL;
  Status = RamDisk->Register ((UINT64)PageAddr, BytesRead, &gEfiVirtualDiskGuid, NULL, &RamDp);
  if (EFI_ERROR (Status)) {
    DEBUG ((DEBUG_ERROR, "[BpRecoveryLoader] step 3 FAIL - RamDisk->Register: %r\n", Status));
    gResult.LastEfiStatus = (INT32)Status;
    WriteResultToNvRam ();
    return Status;
  }
  DEBUG ((DEBUG_INFO, "[BpRecoveryLoader] step 3 OK (%lu MiB at phys 0x%lx, RamDp=%p)\n",
          BytesRead / SIZE_1MB, (UINT64)PageAddr, RamDp));
  gResult.RamDiskRegistered = 1;
  gResult.PhaseReached      = BPL_PHASE_RAMDISK_REGISTERED;
  WriteResultToNvRam ();

  gResult.PhaseReached = BPL_PHASE_DONE;
  WriteResultToNvRam ();

  // Step 4: chainload \EFI\Boot\bootx64.efi from the new RAM disk.
  DEBUG ((DEBUG_INFO, "[BpRecoveryLoader] step 4 - chainload %s from RAM disk\n", CHAINLOAD_FILE_PATH));
  gResult.ChainloadAttempted = 1;
  WriteResultToNvRam ();

  EFI_HANDLE                 RamHandle    = NULL;
  EFI_DEVICE_PATH_PROTOCOL  *RemainingDp  = RamDp;
  Status = gBS->LocateDevicePath (&gEfiDevicePathProtocolGuid, &RemainingDp, &RamHandle);
  if (EFI_ERROR (Status)) {
    DEBUG ((DEBUG_ERROR, "[BpRecoveryLoader] step 4 FAIL - LocateDevicePath(RamDp): %r\n", Status));
    gResult.LastEfiStatus = (INT32)Status;
    WriteResultToNvRam ();
    return Status;
  }
  (VOID)gBS->ConnectController (RamHandle, NULL, NULL, TRUE);

  EFI_HANDLE  *SfsHandles  = NULL;
  UINTN        SfsCount    = 0;
  Status = gBS->LocateHandleBuffer (ByProtocol, &gEfiSimpleFileSystemProtocolGuid, NULL, &SfsCount, &SfsHandles);
  if (EFI_ERROR (Status)) {
    DEBUG ((DEBUG_ERROR, "[BpRecoveryLoader] step 4 FAIL - LocateHandleBuffer(SimpleFileSystem): %r (BP content probably not FAT)\n", Status));
    gResult.LastEfiStatus = (INT32)Status;
    WriteResultToNvRam ();
    return Status;
  }

  UINTN  PrefixSize = GetDevicePathSize (RamDp) - END_DEVICE_PATH_LENGTH;
  EFI_HANDLE  FatHandle = NULL;
  for (UINTN i = 0; i < SfsCount; i++) {
    EFI_DEVICE_PATH_PROTOCOL  *Dp = NULL;
    if (EFI_ERROR (gBS->HandleProtocol (SfsHandles[i], &gEfiDevicePathProtocolGuid, (VOID **)&Dp))) {
      continue;
    }
    if (GetDevicePathSize (Dp) < PrefixSize) {
      continue;
    }
    if (CompareMem (Dp, RamDp, PrefixSize) == 0) {
      FatHandle = SfsHandles[i];
      break;
    }
  }
  gBS->FreePool (SfsHandles);
  if (FatHandle == NULL) {
    DEBUG ((DEBUG_ERROR, "[BpRecoveryLoader] step 4 FAIL - no SimpleFileSystem rooted at RamDp (BP content probably not FAT)\n"));
    Status = EFI_NOT_FOUND;
    gResult.LastEfiStatus = (INT32)Status;
    WriteResultToNvRam ();
    return Status;
  }
  DEBUG ((DEBUG_INFO, "[BpRecoveryLoader] step 4 FAT volume on handle %p\n", FatHandle));

  EFI_DEVICE_PATH_PROTOCOL  *BootDp = FileDevicePath (FatHandle, CHAINLOAD_FILE_PATH);
  if (BootDp == NULL) {
    DEBUG ((DEBUG_ERROR, "[BpRecoveryLoader] step 4 FAIL - FileDevicePath: out of resources\n"));
    Status = EFI_OUT_OF_RESOURCES;
    gResult.LastEfiStatus = (INT32)Status;
    WriteResultToNvRam ();
    return Status;
  }

  EFI_HANDLE  NewImage = NULL;
  Status = gBS->LoadImage (TRUE, ImageHandle, BootDp, NULL, 0, &NewImage);
  FreePool (BootDp);
  if (EFI_ERROR (Status)) {
    DEBUG ((DEBUG_ERROR, "[BpRecoveryLoader] step 4 LoadImage(%s) FAIL: %r\n", CHAINLOAD_FILE_PATH, Status));
    gResult.LastEfiStatus = (INT32)Status;
    WriteResultToNvRam ();
    return Status;
  }
  DEBUG ((DEBUG_INFO, "[BpRecoveryLoader] step 4 LoadImage OK, StartImage...\n"));
  gResult.ChainloadImageLoaded = 1;
  WriteResultToNvRam ();

  Status = gBS->StartImage (NewImage, NULL, NULL);
  DEBUG ((DEBUG_INFO, "[BpRecoveryLoader] StartImage returned: %r (chainload exited back to us)\n", Status));
  gResult.LastEfiStatus = (INT32)Status;
  WriteResultToNvRam ();

  return Status;
}

//
// UEFI_APPLICATION entry point. Dispatched by DeviceBootManagerPriorityBoot.
// Runs the SRE flow unconditionally and cold-resets on return; BootOrder
// selects the next boot target. Does not set BootNext.
//
EFI_STATUS
EFIAPI
AppEntry (
  IN EFI_HANDLE        ImageHandle,
  IN EFI_SYSTEM_TABLE  *SystemTable
  )
{
  (VOID)RunSreFlow (ImageHandle);

  // RunSreFlow returned: chainload didn't reach StartImage or the
  // chainloaded image exited back. Cold-reset; BDS picks the next target.
  DEBUG ((DEBUG_INFO, "[BpRecoveryLoader] AppEntry resetting cold\n"));
  gRT->ResetSystem (EfiResetCold, EFI_SUCCESS, 0, NULL);
  return EFI_SUCCESS;
}

