/** @file
  NvmeBpWrite — UEFI shell application that writes a host-supplied image
  (e.g. a recovery WIM) to the NVMe Boot Partition and verifies it via
  readback. Persists results to NVRAM so they can be retrieved from
  Windows after the controller cold-resets.

  Operations exercised:
    - Set Features FID=0x85 ("Boot Partition Write Protection Config",
      NVMe Base Spec 2.3 §5.2.26.1.36) with CDW11=0x09 to unlock both BP0
      and BP1. REQUIRED; both BPs default to "Write Locked" after every
      power cycle, and Firmware Commit CA=110b against a locked BP returns
      SCT=1 SC=0x1E "Boot Partition Write Prohibited".
    - Firmware Image Download (admin opcode 0x11) of the image file in
      FWUG-granularity chunks. Sends the vendor route hint CDW12=1
      ("Data for boot partition download") on every chunk per the Kioxia
      documented example.
    - Firmware Commit (admin opcode 0x10) with CA=110b (Download to BP) and
      BPID selector in CDW10 bit 31. Per the controller this requires the
      staged image to equal the BP size (BPSZ); upload is zero-padded to
      BPSIZE accordingly.
    - Readback via EFI_PCI_IO_PROTOCOL MMIO against the NVMe controller's
      BAR0 (BPINFO=0x40 / BPRSEL=0x44 / BPMBL=0x48), the NVMe 1.4 §3.1.21
      drive loop.
    - Readback via Get Log Page LID=0x15 (Boot Partition log) with LSP=BPID,
      a 64 KiB scan for the WIM signature.

  Controller-specific (non-spec) behaviors honored by this code:
    - CDW12=1 vendor route hint on Firmware Image Download (Kioxia "Data
      for boot partition download").
    - Staged image MUST equal BPSIZE; sub-BPSIZE commits return SCT=1
      SC=0x1E even after FID=0x85 unlock.
    - BPMBL/BPRSEL MMIO drive loop returns BRS=ERROR regardless of
      BPID/state; LID 0x15 is the only working readback.
    - LID 0x15 response has a 16-byte header preamble before the BP
      image bytes (4B LID echo + 4B BPSZ + 8B reserved).

  Content-hash-driven, no NVRAM state machine. On each boot:

    1. Read the first CHECK_BYTES of \ValidationOS.wim from the USB.
    2. Read the first CHECK_BYTES of BP1 via Get Log Page LID=0x15 (after
       stripping the 16-byte LID preamble).
    3. If the two match -> BP1 already contains this image. Cold-reset to
       Windows. No NVRAM read, no NVRAM write.
    4. If they differ -> unlock (Set Features FID=0x85), upload the full
       WIM via chunked Firmware Image Download CDW12=1, commit CA=110b
       BPID=1, then re-read CHECK_BYTES of BP1 and report PASS/FAIL.
       Cold-reset to Windows.

  This makes the tool self-resetting: re-flashing is "drop a new
  ValidationOS.wim on the USB and boot the tool again". No Windows-side
  Reset-NvmeBpResult.ps1 is required, which matters for factory / field /
  no-OS / corrupted-OS scenarios where Windows can't be assumed.

  Firmware Commit CA=110b returns EFI_WARN_RESET_REQUIRED per spec; BP
  contents become host-visible after the cold reset.

  Logs to ConOut for live observation, and writes the BP_RESULT struct to
  the NvmeBpResult NVRAM variable (runtime-readable) so a host can read the
  run outcome and the BP write-protection readback without capturing the
  console.

  Copyright (c) Microsoft Corporation. All rights reserved.
  SPDX-License-Identifier: BSD-2-Clause-Patent
**/

#include <Uefi.h>
#include <Library/UefiLib.h>
#include <Library/UefiBootServicesTableLib.h>
#include <Library/UefiApplicationEntryPoint.h>
#include <Library/MemoryAllocationLib.h>
#include <Library/BaseMemoryLib.h>
#include <Library/BaseLib.h>
#include <Library/DebugLib.h>
#include <Library/PrintLib.h>
#include <Library/UefiRuntimeServicesTableLib.h>
#include <Protocol/NvmExpressPassthru.h>
#include <Protocol/PciIo.h>
#include <Protocol/LoadedImage.h>
#include <Protocol/SimpleFileSystem.h>
#include <Guid/FileInfo.h>
#include <Guid/GlobalVariable.h>

//
// ConOut-only logger. Durable result handoff to the host is via the
// NvmeBpResult NVRAM variable (see WriteResultToNvRam below).
//
STATIC EFI_HANDLE  gAppImageHandle = NULL;

STATIC
VOID
EFIAPI
LogPrint (
  IN CONST CHAR16  *Fmt,
  ...
  )
{
  CHAR16   Buf[1024];
  VA_LIST  Args;

  VA_START (Args, Fmt);
  UnicodeVSPrint (Buf, sizeof (Buf), Fmt, Args);
  VA_END (Args);

  gST->ConOut->OutputString (gST->ConOut, Buf);
}

//
// LogPrint a 32-byte hex dump in 4-byte groups: "aa bb cc dd  ee ff 00 11 ..."
//
STATIC
VOID
EFIAPI
LogHexDump32 (
  IN CONST CHAR16  *Prefix,
  IN CONST UINT8   *Bytes
  )
{
  CHAR16  Hex[128];
  UINTN   Pos = 0;
  for (UINTN I = 0; I < 32; I++) {
    UnicodeSPrint (Hex + Pos, sizeof (Hex) - Pos * sizeof (CHAR16),
                   L"%02x%s", Bytes[I], ((I + 1) % 4 == 0 && I != 31) ? L" " : L"");
    Pos = StrLen (Hex);
  }
  LogPrint (L"%s%s\n", Prefix, Hex);
}

#define NVME_ADMIN_IDENTIFY                 0x06
#define NVME_ADMIN_FIRMWARE_IMAGE_DOWNLOAD  0x11
#define NVME_ADMIN_FIRMWARE_COMMIT          0x10
#define NVME_ADMIN_GET_LOG_PAGE             0x02
#define NVME_ADMIN_SET_FEATURES             0x09
#define NVME_ADMIN_GET_FEATURES             0x0A

#define NVME_IDENTIFY_CNS_CONTROLLER        0x01

// Firmware Commit Action (CDW10 bits 5:3 per NVMe 1.4 §5.11 Figure 184).
// Only CA=110b (Download to BP) is exercised by this tool. CA=111b (Activate
// Boot Partition) is intentionally not used; per vendor guidance the CA=110b
// commit by itself takes effect at next reset on this controller.
#define NVME_FW_COMMIT_ACTION_DOWNLOAD_BP   0x6

// Boot Partition Write Protection Config — NVMe Base Spec 2.3 §5.2.26.1.36
// Figure 471. This is a spec-defined Set Features Feature Identifier
// (Controller scope, not saveable). REQUIRED before BP writes — both BPs
// default to "Write Locked" after every power cycle and a Firmware Commit
// CA=110b against a locked BP returns SCT=1 SC=0x1E "Boot Partition Write
// Prohibited".
//
// CDW11 layout (Figure 471):
//   bits 31:06  Reserved
//   bits 05:03  BP1WPS (Boot Partition 1 Write Protection State)
//   bits 02:00  BP0WPS (Boot Partition 0 Write Protection State)
// BPxWPS values:
//   000b  Change in state not requested  (leave this BP alone)
//   001b  Write Unlocked
//   010b  Write Locked
//   011b  Write Locked Until Power Cycle
//   100b  Write Protection controlled by RPMB  (read-only via Get; Set aborts)
//   rest  Reserved
#define NVME_FID_BP_WRITE_PROTECTION_CFG    0x85
#define BPWPS_NO_CHANGE                     0x0
#define BPWPS_UNLOCKED                      0x1
#define BPWPS_LOCKED                        0x2
#define BPWPS_LOCKED_UNTIL_POWER_CYCLE      0x3
#define BPWPS_BP1_SHIFT                     3   // CDW11 bits 5:3 = BP1WPS
#define BPWPS_BP0_SHIFT                     0   // CDW11 bits 2:0 = BP0WPS
#define BPWPS_FIELD_MASK                    0x7

#define IDENTIFY_BUFFER_SIZE                4096
// Target boot partition. BP0 and BP1 use the same write-protection encoding:
// Get Features FID=0x85 returns both states in the standard CDW11 layout
// (BP1WPS bits 5:3, BP0WPS bits 2:0), and CDW11=0x09 unlocks both. Either
// partition is writable by this path; change this constant and the Firmware
// Commit BPID bit together to retarget.
#define TARGET_BPID                         1

// Identify Controller offsets (NVMe 1.4 Figure 247)
#define ID_CTRL_OFFSET_MDTS                 77   // 1 byte
#define ID_CTRL_OFFSET_OACS                 256  // 2 bytes
#define ID_CTRL_OFFSET_FWUG                 319  // 1 byte
#define ID_CTRL_OFFSET_VS                   3072 // 1024 bytes (3072..4095) vendor-specific region

//
// NVRAM result logger. We write a fixed-size summary to a UEFI variable at
// the very end of the run via gRT->SetVariable, which goes to NVRAM and is
// independent of NVMe namespace I/O — so it survives even if the log file's
// FAT directory update never propagates. Windows can read the variable
// back via GetFirmwareEnvironmentVariableExW.
//
#define BP_RESULT_MAGIC      0x42505456  // 'BPTV'
#define BP_RESULT_VERSION    0x00020000  // first committed baseline

//
// NVMe BAR0 register offsets for the Boot Partition read mechanism (NVMe 1.4 §3.1.21).
//
#define MMIO_OFFSET_BPINFO       0x40   // 32-bit RO; bits 25:24 = BRS, bit 31 = ABPID
#define MMIO_OFFSET_BPRSEL       0x44   // 32-bit RW; bit 31 = BPID, 28:10 = offset_4k, 9:0 = (size_4k - 1)
#define MMIO_OFFSET_BPMBL        0x48   // 64-bit RW; host phys addr (4 KiB aligned)
#define BPINFO_BRS_SHIFT         24
#define BPINFO_BRS_MASK          0x3
#define BPINFO_BRS_IDLE          0x0
#define BPINFO_BRS_IN_PROGRESS   0x1
#define BPINFO_BRS_SUCCESS       0x2
#define BPINFO_BRS_ERROR         0x3
#define BPINFO_ABPID_SHIFT       31
#define BPINFO_ABPID_MASK        0x1
#define BPINFO_BPSZ_MASK         0x7FFF  // bits 14:0, BP size in 128 KiB units
#define BP_CHUNK_ALIGNMENT       4096

//
// Tool version. Bumped when the user-facing behavior changes; printed in the
// startup banner so an engineer can tell which generation they're running.
//   0.1    =  NVRAM-state-machine design (WRITE -> VERIFY -> NOOP via NvmeBpResult)
//   0.2    =  Content-hash design (no NVRAM state; idempotent re-boots)
//   0.2.1  =  Force-reflash flag (drop \force-reflash.flag on USB to skip the
//             content check and re-WRITE even if BP1 already matches)
//   0.2.2  =  No .efi behavior change vs 0.2.1; bump tracks the kit ship that
//             includes x64 boot-x64/bootmgfw.efi + boot.sdi + arch guard in
//             Stage-SreflashUsb.ps1 (fixes cross-arch FAT-wrap on non-x64
//             staging hosts). Engineers reading the banner know which kit
//             generation they're on even though the tool source is unchanged.
//
#define NVME_BP_WRITE_VERSION    "0.2.2"

//
// Content-hash check parameters: read the first CHECK_BYTES of the WIM and
// of BP1, compare. 4 KiB is enough to detect any meaningful content change
// (WIM header / FAT BPB / etc.) and minimizes the time spent on the
// no-write-needed path.
//
#define CHECK_BYTES              4096
#define LID15_PREAMBLE_BYTES     16    // LID 0x15 response = 16B preamble + image bytes

//
// Force-reflash override. If a file at this path exists on any FAT volume at
// startup, the tool skips the content hash check and runs WRITE
// unconditionally. Drop an empty file at <USB>:\force-reflash.flag (e.g. from
// Windows Explorer, cmd `type nul > E:\force-reflash.flag`, or the
// Stage-SreflashUsb.ps1 -ForceReflash switch) to trigger.
//
#define FORCE_REFLASH_FLAG_PATH  L"\\force-reflash.flag"

//
// Run mode (legacy — kept compiled for source-compat with WriteBpTestPattern's
// gResult.RunMode field; no longer drives flow).
//
#define RUN_MODE_WRITE   0
#define RUN_MODE_VERIFY  1
#define RUN_MODE_NOOP    2

// Custom vendor GUID for the NvmeBpResult variable. Surface UEFI variable
// services apply name/policy filters under EFI_GLOBAL_VARIABLE_GUID; using
// a dedicated GUID sidesteps any restrictions on non-spec names.
//   {7B5A1F3E-2D8C-4A91-B6E3-D8F2C9A4E105}
STATIC EFI_GUID  gNvmeBpResultGuid = {
  0x7B5A1F3E, 0x2D8C, 0x4A91, { 0xB6, 0xE3, 0xD8, 0xF2, 0xC9, 0xA4, 0xE1, 0x05 }
};

typedef enum {
  BP_PHASE_INIT                  = 0,
  BP_PHASE_WIM_LOADED            = 1,
  BP_PHASE_VENDOR_WRITE_ENABLE   = 2,   // Set Features FID=0x85 (vendor BP write enable)
  BP_PHASE_A_DOWNLOAD            = 3,
  BP_PHASE_B_COMMIT              = 4,
  BP_PHASE_DONE                  = 5,   // WRITE mode completed
  // Verify-mode phases (only reached when RunMode == VERIFY)
  BP_PHASE_VERIFY_BPINFO_READ         = 10,
  BP_PHASE_VERIFY_MMIO_READ           = 11,
  BP_PHASE_VERIFY_BPINFO_RECHECK      = 12,
  BP_PHASE_VERIFY_LID15_READ          = 13,
  BP_PHASE_VERIFY_DONE                = 14,
} BP_PHASE;

#pragma pack(push, 1)
typedef struct {
  // === Header + run state ===
  UINT32  Magic;                  // BP_RESULT_MAGIC
  UINT32  Version;                // BP_RESULT_VERSION
  UINT32  PhaseReached;           // BP_PHASE_*
  UINT32  RunMode;                // RUN_MODE_WRITE / RUN_MODE_VERIFY / RUN_MODE_NOOP
  UINT32  EndToEndSuccess;        // 1 if write-side Commit returned SC=0x00, else 0
  // === WRITE-mode results ===
  UINT64  WimFileBytes;
  UINT64  WimPaddedBytes;
  UINT64  BytesUploaded;
  UINT32  LastDw3;                // last NVMe completion DW3 (commit, or fail-point chunk)
  UINT32  LastSct;
  UINT32  LastSc;
  INT32   LastEfiStatus;          // (INT32) cast of EFI_STATUS for legibility from Windows
  // Set Features FID=0x85 (BP Write Protection Config) unlock. Captures
  // completion status so we can confirm the unlock was accepted before
  // chunked download started.
  UINT32  VendorEnableDw3;        // (field name preserved for NVRAM layout compat)
  UINT32  VendorEnableStatus;     // SCT<<8 | SC
  INT32   VendorEnableEfiStatus;
  // === Get Features FID=0x85 write-protection readback (BP0/BP1 experiment) ===
  UINT32  BpWpsPreDw0;            // Get Features DW0 before unlock (BP1WPS 5:3, BP0WPS 2:0)
  UINT32  BpWpsPreStatus;         // SCT<<8 | SC for the pre-unlock read
  UINT32  BpWpsPostDw0;           // Get Features DW0 after  unlock
  UINT32  BpWpsPostStatus;        // SCT<<8 | SC for the post-unlock read
  // === VERIFY-mode results: BPMBL/BPRSEL/BPINFO MMIO drive loop ===
  UINT32  MmioBpinfoStart;        // raw BPINFO at start of verify (BPSZ + BRS + ABPID)
  UINT32  MmioBprselEnc;          // encoded BPRSEL value we wrote (BPID|offset|size)
  UINT32  MmioBrsLast;            // last BRS value polled (0=IDLE 1=InProg 2=Success 3=ERROR)
  INT32   MmioReadEfiStatus;
  UINT8   MmioReadFirst32[32];    // first 32 bytes returned via BPMBL DMA
  UINT32  MmioBpinfoSecondRead;   // raw BPINFO after the drive loop
  // === VERIFY-mode results: Get Log Page LID=0x15 (Boot Partition log) ===
  UINT32  LidPageDw3;
  UINT32  LidPageStatus;          // SCT<<8 | SC
  INT32   LidPageEfiStatus;
  UINT8   LidPageFirst32[32];     // first 32 bytes of the LID 0x15 response
  UINT32  LidPageBytesRead;       // total bytes requested in the LID 0x15 read
  UINT32  LidPageMswimOffset;     // byte offset of MSWIM signature, or 0xFFFFFFFF if not found
} BP_RESULT;
#pragma pack(pop)

STATIC BP_RESULT  gResult;

//
// Persist gResult to the NvmeBpResult NVRAM variable with RUNTIME access so a
// host OS can read the run outcome and the BP write-protection readback via
// GetFirmwareEnvironmentVariableEx, without capturing the console. Called at
// several points during the run so a crash still leaves the last state behind.
//
STATIC
VOID
EFIAPI
WriteResultToNvRam (
  VOID
  )
{
  EFI_STATUS  Status;

  gResult.Magic   = BP_RESULT_MAGIC;
  gResult.Version = BP_RESULT_VERSION;
  Status          = gRT->SetVariable (
                          L"NvmeBpResult",
                          &gNvmeBpResultGuid,
                          EFI_VARIABLE_NON_VOLATILE | EFI_VARIABLE_BOOTSERVICE_ACCESS | EFI_VARIABLE_RUNTIME_ACCESS,
                          sizeof (gResult),
                          &gResult
                          );
  if (EFI_ERROR (Status)) {
    LogPrint (L"[warn] WriteResultToNvRam: SetVariable(NvmeBpResult) failed: %r\n", Status);
  }
}

// Path to the BP image on the load volume (typically \ValidationOS.wim on
// the ESP this app was launched from). Loaded into DRAM once at startup so
// the chunked download loop has no FS I/O.
#define WIM_FILE_PATH                       L"\\ValidationOS.wim"

// CDW12 vendor route hint on Firmware Image Download (per Kioxia documented
// example): 1 = "Data for boot partition download", 0 = "SSD FW download".
#define FW_IMG_DOWNLOAD_CDW12_BP_DATA       0x00000001

// Firmware Commit CA=110b on this controller requires the staged image to
// equal BPSIZE; sub-BPSIZE stages are rejected with SC=0x1E "Boot Partition
// Write Prohibited". Pad the source image up to BPSIZE with zeros on the
// host side. BPSIZE is reported by BPINFO bits 14:0 in 128 KiB units; on
// the target controllers in scope this is 0x2000 = 1 GiB.
#define BPSIZE_BYTES                        (1024ULL * SIZE_1MB)
#define TARGET_UPLOAD_BYTES                 BPSIZE_BYTES

//
// Read the WIM from the load volume into a freshly-allocated DRAM buffer,
// padded up to ChunkBytes alignment with zero bytes so every chunk can be
// sent at full size. Caller owns the returned buffer (FreePages with the
// matching page count) and gets back both the real file size and the
// padded size that will be uploaded.
//
// Performed before any NVMe admin commands so FS I/O on the same namespace
// can't collide with controller BP staging state mid-upload.
//
STATIC
EFI_STATUS
EFIAPI
LoadWimIntoBuffer (
  IN  CONST CHAR16  *FilePath,
  IN  UINTN          ChunkBytes,
  OUT UINT8        **OutBuffer,
  OUT UINTN         *OutPages,
  OUT UINT64        *OutFileBytes,
  OUT UINT64        *OutPaddedBytes
  )
{
  EFI_STATUS                       Status;
  EFI_LOADED_IMAGE_PROTOCOL        *LoadedImage;
  EFI_SIMPLE_FILE_SYSTEM_PROTOCOL  *Sfs = NULL;
  EFI_FILE_PROTOCOL                *Wim;
  EFI_FILE_INFO                    *Info;
  UINTN                            InfoSize;
  UINT64                           FileBytes;
  UINT64                           PaddedBytes;
  UINTN                            Pages;
  EFI_PHYSICAL_ADDRESS             PageAddr;
  UINT8                            *Buf;
  UINTN                            ReadSize;

  *OutBuffer      = NULL;
  *OutPages       = 0;
  *OutFileBytes   = 0;
  *OutPaddedBytes = 0;

  // Search all SimpleFileSystem volumes for FilePath. Load volume
  // (typically ESP) is tried first, then any other FAT-mounted volume
  // (e.g. USB staging).
  Wim = NULL;
  Status = gBS->HandleProtocol (gAppImageHandle, &gEfiLoadedImageProtocolGuid, (VOID **)&LoadedImage);
  if (EFI_ERROR (Status)) {
    LogPrint (L"  [wim] HandleProtocol(LoadedImage): %r\n", Status);
    return Status;
  }

  EFI_HANDLE  LoadHandle = LoadedImage->DeviceHandle;

  // Surface firmware only connects drivers for the boot-path device when
  // launching a Firmware Application boot entry. USB / removable storage
  // controllers aren't bound by default, so their SimpleFileSystem handles
  // never appear in LocateHandleBuffer. Force-connect every handle to
  // trigger USB-bus + FAT driver binding on USB sticks before we enumerate.
  {
    EFI_HANDLE  *EveryHandle = NULL;
    UINTN       HandleTotal  = 0;
    if (!EFI_ERROR (gBS->LocateHandleBuffer (AllHandles, NULL, NULL, &HandleTotal, &EveryHandle))) {
      for (UINTN I = 0; I < HandleTotal; I++) {
        (VOID)gBS->ConnectController (EveryHandle[I], NULL, NULL, TRUE);
      }
      gBS->FreePool (EveryHandle);
    }
  }

  EFI_HANDLE  *FsHandles = NULL;
  UINTN       FsCount    = 0;
  Status = gBS->LocateHandleBuffer (ByProtocol, &gEfiSimpleFileSystemProtocolGuid, NULL, &FsCount, &FsHandles);
  if (EFI_ERROR (Status)) {
    LogPrint (L"  [wim] LocateHandleBuffer(SFS): %r\n", Status);
    return Status;
  }
  LogPrint (L"  [wim] %u SFS handles after ConnectAll\n", (UINT32)FsCount);

  // Two-pass: load volume first, then everything else.
  for (UINT32 Pass = 0; Pass < 2 && Wim == NULL; Pass++) {
    for (UINTN I = 0; I < FsCount; I++) {
      EFI_HANDLE  H = FsHandles[I];
      if (Pass == 0 && H != LoadHandle) continue;
      if (Pass == 1 && H == LoadHandle) continue;

      EFI_SIMPLE_FILE_SYSTEM_PROTOCOL  *Cand = NULL;
      if (EFI_ERROR (gBS->HandleProtocol (H, &gEfiSimpleFileSystemProtocolGuid, (VOID **)&Cand))) continue;

      EFI_FILE_PROTOCOL  *CandRoot = NULL;
      if (EFI_ERROR (Cand->OpenVolume (Cand, &CandRoot))) continue;

      EFI_FILE_PROTOCOL  *CandWim = NULL;
      Status = CandRoot->Open (CandRoot, &CandWim, (CHAR16 *)FilePath, EFI_FILE_MODE_READ, 0);
      CandRoot->Close (CandRoot);
      if (!EFI_ERROR (Status)) {
        Wim = CandWim;
        Sfs = Cand;
        LogPrint (L"  [wim] Found %s on volume handle %p (%s)\n",
                  FilePath, H, (H == LoadHandle) ? L"load volume" : L"non-load volume, e.g. USB");
        break;
      }
    }
  }
  gBS->FreePool (FsHandles);
  if (Wim == NULL) {
    LogPrint (L"  [wim] %s not found on any FS volume (load + %u others searched)\n", FilePath, (UINT32)FsCount);
    return EFI_NOT_FOUND;
  }
  (VOID)Sfs;  // Sfs is informational; the actual handle we use is Wim.

  InfoSize = 0;
  Status = Wim->GetInfo (Wim, &gEfiFileInfoGuid, &InfoSize, NULL);
  if (Status != EFI_BUFFER_TOO_SMALL) {
    LogPrint (L"  [wim] GetInfo probe: %r\n", Status);
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
    LogPrint (L"  [wim] GetInfo: %r\n", Status);
    FreePool (Info);
    Wim->Close (Wim);
    return Status;
  }
  FileBytes = Info->FileSize;
  FreePool (Info);

  // This controller requires a full BPSIZE staged image at commit. Files
  // larger than that get rejected (truncating would silently lose data).
  // Smaller files get the buffer allocated at full BPSIZE and the tail
  // left zero, so the chunked download writes a deterministic image.
  if (FileBytes > TARGET_UPLOAD_BYTES) {
    LogPrint (L"  [wim] file too large: %lu bytes (max %lu bytes)\n",
              FileBytes, (UINT64)TARGET_UPLOAD_BYTES);
    Wim->Close (Wim);
    return EFI_BAD_BUFFER_SIZE;
  }

  if ((TARGET_UPLOAD_BYTES % ChunkBytes) != 0) {
    LogPrint (L"  [wim] internal error: TARGET_UPLOAD_BYTES (%lu) not a multiple of chunk size (%lu)\n",
              (UINT64)TARGET_UPLOAD_BYTES, (UINT64)ChunkBytes);
    Wim->Close (Wim);
    return EFI_UNSUPPORTED;
  }

  if (FileBytes < TARGET_UPLOAD_BYTES) {
    PaddedBytes = TARGET_UPLOAD_BYTES;
  } else {
    // FileBytes == TARGET_UPLOAD_BYTES; round up to chunk boundary so every
    // download chunk is full-size (already aligned when equal).
    PaddedBytes = ((FileBytes + (UINT64)ChunkBytes - 1) / (UINT64)ChunkBytes) * (UINT64)ChunkBytes;
  }

  Pages = (UINTN)((PaddedBytes + EFI_PAGE_SIZE - 1) / EFI_PAGE_SIZE);

  Status = gBS->AllocatePages (AllocateAnyPages, EfiBootServicesData, Pages, &PageAddr);
  if (EFI_ERROR (Status)) {
    LogPrint (L"  [wim] AllocatePages(%lu pages = %lu MiB): %r\n",
              (UINT64)Pages, (UINT64)(Pages * EFI_PAGE_SIZE / SIZE_1MB), Status);
    Wim->Close (Wim);
    return Status;
  }
  Buf = (UINT8 *)(UINTN)PageAddr;

  // Zero the entire buffer first so the tail padding is deterministic.
  ZeroMem (Buf, (UINTN)PaddedBytes);

  ReadSize = (UINTN)FileBytes;
  Status = Wim->Read (Wim, &ReadSize, Buf);
  Wim->Close (Wim);
  if (EFI_ERROR (Status)) {
    LogPrint (L"  [wim] Read: %r\n", Status);
    gBS->FreePages (PageAddr, Pages);
    return Status;
  }
  if ((UINT64)ReadSize != FileBytes) {
    LogPrint (L"  [wim] short read: got %lu of %lu bytes\n", (UINT64)ReadSize, FileBytes);
    gBS->FreePages (PageAddr, Pages);
    return EFI_DEVICE_ERROR;
  }

  *OutBuffer      = Buf;
  *OutPages       = Pages;
  *OutFileBytes   = FileBytes;
  *OutPaddedBytes = PaddedBytes;
  return EFI_SUCCESS;
}

//
// Read just the first HeadBytes of FilePath into Buf. Cheap, no BPSIZE
// allocation. Same FS-discovery logic as LoadWimIntoBuffer (force-connect
// all controllers, prefer load volume, fall back to other SFS volumes).
// Used for the content-hash check at startup so we can decide WRITE vs
// skip-write without loading the full ~250-1024 MB WIM upfront.
//
STATIC
EFI_STATUS
EFIAPI
ReadWimHead (
  IN  CONST CHAR16  *FilePath,
  IN  UINTN          HeadBytes,
  OUT UINT8         *Buf,
  OUT UINTN         *BytesRead
  )
{
  EFI_STATUS                       Status;
  EFI_LOADED_IMAGE_PROTOCOL        *LoadedImage;
  EFI_FILE_PROTOCOL                *Wim = NULL;

  *BytesRead = 0;

  Status = gBS->HandleProtocol (gAppImageHandle, &gEfiLoadedImageProtocolGuid, (VOID **)&LoadedImage);
  if (EFI_ERROR (Status)) {
    return Status;
  }
  EFI_HANDLE  LoadHandle = LoadedImage->DeviceHandle;

  // Force-connect every handle so USB controllers' FAT driver binds.
  {
    EFI_HANDLE  *EveryHandle = NULL;
    UINTN       HandleTotal  = 0;
    if (!EFI_ERROR (gBS->LocateHandleBuffer (AllHandles, NULL, NULL, &HandleTotal, &EveryHandle))) {
      for (UINTN I = 0; I < HandleTotal; I++) {
        (VOID)gBS->ConnectController (EveryHandle[I], NULL, NULL, TRUE);
      }
      gBS->FreePool (EveryHandle);
    }
  }

  EFI_HANDLE  *FsHandles = NULL;
  UINTN       FsCount    = 0;
  Status = gBS->LocateHandleBuffer (ByProtocol, &gEfiSimpleFileSystemProtocolGuid, NULL, &FsCount, &FsHandles);
  if (EFI_ERROR (Status)) {
    return Status;
  }

  for (UINT32 Pass = 0; Pass < 2 && Wim == NULL; Pass++) {
    for (UINTN I = 0; I < FsCount; I++) {
      EFI_HANDLE  H = FsHandles[I];
      if (Pass == 0 && H != LoadHandle) continue;
      if (Pass == 1 && H == LoadHandle) continue;

      EFI_SIMPLE_FILE_SYSTEM_PROTOCOL  *Cand = NULL;
      if (EFI_ERROR (gBS->HandleProtocol (H, &gEfiSimpleFileSystemProtocolGuid, (VOID **)&Cand))) continue;

      EFI_FILE_PROTOCOL  *CandRoot = NULL;
      if (EFI_ERROR (Cand->OpenVolume (Cand, &CandRoot))) continue;

      EFI_FILE_PROTOCOL  *CandWim = NULL;
      if (!EFI_ERROR (CandRoot->Open (CandRoot, &CandWim, (CHAR16 *)FilePath, EFI_FILE_MODE_READ, 0))) {
        Wim = CandWim;
      }
      CandRoot->Close (CandRoot);
      if (Wim != NULL) break;
    }
  }
  gBS->FreePool (FsHandles);
  if (Wim == NULL) {
    return EFI_NOT_FOUND;
  }

  UINTN ReadSize = HeadBytes;
  Status = Wim->Read (Wim, &ReadSize, Buf);
  Wim->Close (Wim);
  if (EFI_ERROR (Status)) {
    return Status;
  }
  *BytesRead = ReadSize;
  return EFI_SUCCESS;
}

//
// Check if FilePath exists on any FAT-mounted volume. Same connect-all +
// two-pass volume search as ReadWimHead (load volume first, then others).
// Used to detect the force-reflash override flag at startup.
//
STATIC
BOOLEAN
EFIAPI
FileExistsOnAnyVolume (
  IN CONST CHAR16  *FilePath
  )
{
  EFI_STATUS                       Status;
  EFI_LOADED_IMAGE_PROTOCOL        *LoadedImage;
  BOOLEAN                          Found = FALSE;

  Status = gBS->HandleProtocol (gAppImageHandle, &gEfiLoadedImageProtocolGuid, (VOID **)&LoadedImage);
  if (EFI_ERROR (Status)) {
    return FALSE;
  }
  EFI_HANDLE  LoadHandle = LoadedImage->DeviceHandle;

  // Force-connect every handle so USB FAT volumes bind.
  {
    EFI_HANDLE  *EveryHandle = NULL;
    UINTN       HandleTotal  = 0;
    if (!EFI_ERROR (gBS->LocateHandleBuffer (AllHandles, NULL, NULL, &HandleTotal, &EveryHandle))) {
      for (UINTN I = 0; I < HandleTotal; I++) {
        (VOID)gBS->ConnectController (EveryHandle[I], NULL, NULL, TRUE);
      }
      gBS->FreePool (EveryHandle);
    }
  }

  EFI_HANDLE  *FsHandles = NULL;
  UINTN       FsCount    = 0;
  Status = gBS->LocateHandleBuffer (ByProtocol, &gEfiSimpleFileSystemProtocolGuid, NULL, &FsCount, &FsHandles);
  if (EFI_ERROR (Status)) {
    return FALSE;
  }

  for (UINT32 Pass = 0; Pass < 2 && !Found; Pass++) {
    for (UINTN I = 0; I < FsCount; I++) {
      EFI_HANDLE  H = FsHandles[I];
      if (Pass == 0 && H != LoadHandle) continue;
      if (Pass == 1 && H == LoadHandle) continue;

      EFI_SIMPLE_FILE_SYSTEM_PROTOCOL  *Cand = NULL;
      if (EFI_ERROR (gBS->HandleProtocol (H, &gEfiSimpleFileSystemProtocolGuid, (VOID **)&Cand))) continue;

      EFI_FILE_PROTOCOL  *CandRoot = NULL;
      if (EFI_ERROR (Cand->OpenVolume (Cand, &CandRoot))) continue;

      EFI_FILE_PROTOCOL  *Test = NULL;
      if (!EFI_ERROR (CandRoot->Open (CandRoot, &Test, (CHAR16 *)FilePath, EFI_FILE_MODE_READ, 0))) {
        Test->Close (Test);
        Found = TRUE;
      }
      CandRoot->Close (CandRoot);
      if (Found) break;
    }
  }
  gBS->FreePool (FsHandles);
  return Found;
}

STATIC
EFI_STATUS
QueryIdentifyController (
  IN  EFI_NVM_EXPRESS_PASS_THRU_PROTOCOL  *PassThru,
  OUT UINT8                                *Mdts,
  OUT UINT8                                *Fwug,
  OUT UINT16                               *Oacs,
  OUT UINT8                                *VsRegion OPTIONAL  // 1024 bytes if non-NULL
  )
{
  EFI_NVM_EXPRESS_PASS_THRU_COMMAND_PACKET  Packet;
  EFI_NVM_EXPRESS_COMMAND                   Cmd;
  EFI_NVM_EXPRESS_COMPLETION                Completion;
  EFI_STATUS                                Status;
  UINT8                                     *Buffer;

  Buffer = AllocateZeroPool (IDENTIFY_BUFFER_SIZE);
  if (Buffer == NULL) {
    return EFI_OUT_OF_RESOURCES;
  }

  ZeroMem (&Cmd, sizeof (Cmd));
  ZeroMem (&Completion, sizeof (Completion));
  Cmd.Cdw0.Opcode = NVME_ADMIN_IDENTIFY;
  Cmd.Cdw10       = NVME_IDENTIFY_CNS_CONTROLLER;
  Cmd.Flags       = CDW10_VALID;

  ZeroMem (&Packet, sizeof (Packet));
  Packet.CommandTimeout = 1ULL * 10000000ULL;
  Packet.TransferBuffer = Buffer;
  Packet.TransferLength = IDENTIFY_BUFFER_SIZE;
  Packet.QueueType      = NVME_ADMIN_QUEUE;
  Packet.NvmeCmd        = &Cmd;
  Packet.NvmeCompletion = &Completion;

  Status = PassThru->PassThru (PassThru, 0, &Packet, NULL);
  if (EFI_ERROR (Status)) {
    LogPrint (L"  Identify Controller via Pass-Thru: %r  DW3=0x%08x\n", Status, Completion.DW3);
    FreePool (Buffer);
    return Status;
  }
  if (((Completion.DW3 >> 17) & 0x7FFF) != 0) {
    LogPrint (L"  Identify Controller rejected: status field 0x%x  DW3=0x%08x\n",
           (Completion.DW3 >> 17) & 0x7FFF, Completion.DW3);
    FreePool (Buffer);
    return EFI_DEVICE_ERROR;
  }

  *Mdts = Buffer[ID_CTRL_OFFSET_MDTS];
  *Fwug = Buffer[ID_CTRL_OFFSET_FWUG];
  *Oacs = *(UINT16 *)(Buffer + ID_CTRL_OFFSET_OACS);
  if (VsRegion != NULL) {
    CopyMem (VsRegion, Buffer + ID_CTRL_OFFSET_VS, IDENTIFY_BUFFER_SIZE - ID_CTRL_OFFSET_VS);
  }

  FreePool (Buffer);
  return EFI_SUCCESS;
}

//
// Unlock both Boot Partitions for writes via Set Features FID=0x85
// (NVMe Base Spec 2.3 §5.2.26.1.36). REQUIRED before Firmware Image
// Download / Firmware Commit will propagate the staged image to BP —
// both BPs default to "Write Locked" after every power cycle.
//
// CDW11 packs both BPs side-by-side:
//   BP1WPS (bits 5:3) = 001b (Write Unlocked)
//   BP0WPS (bits 2:0) = 001b (Write Unlocked)
//   => CDW11 = 0x09
//
STATIC
EFI_STATUS
EFIAPI
UnlockBpWriteProtection (
  IN  EFI_NVM_EXPRESS_PASS_THRU_PROTOCOL  *PassThru,
  OUT UINT32                              *Dw3,
  OUT UINT32                              *Sct,
  OUT UINT32                              *Sc
  )
{
  EFI_NVM_EXPRESS_PASS_THRU_COMMAND_PACKET  Packet;
  EFI_NVM_EXPRESS_COMMAND                   Cmd;
  EFI_NVM_EXPRESS_COMPLETION                Completion;
  UINT32                                    StatusField;

  UINT32  Cdw11 =
      (((UINT32)BPWPS_UNLOCKED & BPWPS_FIELD_MASK) << BPWPS_BP1_SHIFT) |
      (((UINT32)BPWPS_UNLOCKED & BPWPS_FIELD_MASK) << BPWPS_BP0_SHIFT);

  ZeroMem (&Cmd, sizeof (Cmd));
  ZeroMem (&Completion, sizeof (Completion));
  Cmd.Cdw0.Opcode = NVME_ADMIN_SET_FEATURES;
  Cmd.Cdw10       = NVME_FID_BP_WRITE_PROTECTION_CFG;  // FID at bits 7:0, SV=0
  Cmd.Cdw11       = Cdw11;
  Cmd.Flags       = CDW10_VALID | CDW11_VALID;

  ZeroMem (&Packet, sizeof (Packet));
  Packet.CommandTimeout = 2ULL * 10000000ULL;
  Packet.QueueType      = NVME_ADMIN_QUEUE;
  Packet.NvmeCmd        = &Cmd;
  Packet.NvmeCompletion = &Completion;

  EFI_STATUS  Status = PassThru->PassThru (PassThru, 0, &Packet, NULL);
  *Dw3 = Completion.DW3;
  StatusField = (Completion.DW3 >> 17) & 0x7FFF;
  *Sc  = (StatusField >> 0) & 0xFF;
  *Sct = (StatusField >> 8) & 0x7;
  return Status;
}

//
// Read back the current Boot Partition Write Protection state via
// Get Features FID=0x85. Completion DW0 carries the current value in the
// CDW11 layout: BP1WPS at bits 5:3, BP0WPS at bits 2:0.
//
STATIC
EFI_STATUS
EFIAPI
ReadBpWriteProtection (
  IN  EFI_NVM_EXPRESS_PASS_THRU_PROTOCOL  *PassThru,
  OUT UINT32                              *Dw0,
  OUT UINT32                              *Sct,
  OUT UINT32                              *Sc
  )
{
  EFI_NVM_EXPRESS_PASS_THRU_COMMAND_PACKET  Packet;
  EFI_NVM_EXPRESS_COMMAND                   Cmd;
  EFI_NVM_EXPRESS_COMPLETION                Completion;
  UINT32                                    StatusField;

  ZeroMem (&Cmd, sizeof (Cmd));
  ZeroMem (&Completion, sizeof (Completion));
  Cmd.Cdw0.Opcode = NVME_ADMIN_GET_FEATURES;
  Cmd.Cdw10       = NVME_FID_BP_WRITE_PROTECTION_CFG;  // FID at bits 7:0, SEL=000b (current)
  Cmd.Flags       = CDW10_VALID;

  ZeroMem (&Packet, sizeof (Packet));
  Packet.CommandTimeout = 2ULL * 10000000ULL;
  Packet.QueueType      = NVME_ADMIN_QUEUE;
  Packet.NvmeCmd        = &Cmd;
  Packet.NvmeCompletion = &Completion;

  EFI_STATUS  Status = PassThru->PassThru (PassThru, 0, &Packet, NULL);
  *Dw0 = Completion.DW0;
  StatusField = (Completion.DW3 >> 17) & 0x7FFF;
  *Sc  = (StatusField >> 0) & 0xFF;
  *Sct = (StatusField >> 8) & 0x7;
  return Status;
}

//
// Decode and log both BPxWPS fields from a Get Features FID=0x85 DW0 value.
//
STATIC
VOID
LogBpWpsState (
  IN CONST CHAR16  *Tag,
  IN UINT32        Dw0
  )
{
  STATIC CONST CHAR16  *WpsNames[8] = {
    L"NoChangeRequested/None", L"WriteUnlocked", L"WriteLocked",
    L"WriteLockedUntilPowerCycle", L"RPMB-controlled",
    L"Reserved(5)", L"Reserved(6)", L"Reserved(7)"
  };
  UINT32  Bp0 = (Dw0 >> BPWPS_BP0_SHIFT) & BPWPS_FIELD_MASK;
  UINT32  Bp1 = (Dw0 >> BPWPS_BP1_SHIFT) & BPWPS_FIELD_MASK;
  LogPrint (L"  %s: DW0=0x%08x  BP0WPS=%u (%s)  BP1WPS=%u (%s)\n",
            Tag, Dw0, Bp0, WpsNames[Bp0], Bp1, WpsNames[Bp1]);
}

//
// Send a single Firmware Image Download chunk and decode the completion.
// Logs nothing; caller decides whether/how to log based on the returned values.
//
STATIC
EFI_STATUS
SendDownloadChunk (
  IN  EFI_NVM_EXPRESS_PASS_THRU_PROTOCOL  *PassThru,
  IN  UINT8                                *Buffer,
  IN  UINTN                                 PatternSize,
  IN  UINT64                                OffsetBytes,
  OUT UINT32                               *Dw3,
  OUT UINT32                               *Sct,
  OUT UINT32                               *Sc
  )
{
  EFI_NVM_EXPRESS_PASS_THRU_COMMAND_PACKET  Packet;
  EFI_NVM_EXPRESS_COMMAND                   Cmd;
  EFI_NVM_EXPRESS_COMPLETION                Completion;
  UINT32                                    StatusField;

  ZeroMem (&Cmd, sizeof (Cmd));
  ZeroMem (&Completion, sizeof (Completion));
  Cmd.Cdw0.Opcode = NVME_ADMIN_FIRMWARE_IMAGE_DOWNLOAD;
  Cmd.Cdw10       = (UINT32)((PatternSize / 4) - 1);
  Cmd.Cdw11       = (UINT32)(OffsetBytes / 4);
  Cmd.Cdw12       = FW_IMG_DOWNLOAD_CDW12_BP_DATA;
  Cmd.Flags       = CDW10_VALID | CDW11_VALID | CDW12_VALID;

  ZeroMem (&Packet, sizeof (Packet));
  Packet.CommandTimeout = 5ULL * 10000000ULL;
  Packet.TransferBuffer = Buffer;
  Packet.TransferLength = (UINT32)PatternSize;
  Packet.QueueType      = NVME_ADMIN_QUEUE;
  Packet.NvmeCmd        = &Cmd;
  Packet.NvmeCompletion = &Completion;

  EFI_STATUS  Status = PassThru->PassThru (PassThru, 0, &Packet, NULL);
  *Dw3 = Completion.DW3;
  StatusField = (Completion.DW3 >> 17) & 0x7FFF;
  *Sc  = (StatusField >> 0) & 0xFF;
  *Sct = (StatusField >> 8) & 0x7;
  return Status;
}

//
// Send a Firmware Commit (action=DownloadBP) and decode the completion.
//
STATIC
EFI_STATUS
SendDownloadBpCommit (
  IN  EFI_NVM_EXPRESS_PASS_THRU_PROTOCOL  *PassThru,
  OUT UINT32                               *Dw3,
  OUT UINT32                               *Sct,
  OUT UINT32                               *Sc
  )
{
  EFI_NVM_EXPRESS_PASS_THRU_COMMAND_PACKET  Packet;
  EFI_NVM_EXPRESS_COMMAND                   Cmd;
  EFI_NVM_EXPRESS_COMPLETION                Completion;
  UINT32                                    StatusField;

  ZeroMem (&Cmd, sizeof (Cmd));
  ZeroMem (&Completion, sizeof (Completion));
  // Standard NVMe Firmware Commit (opcode 0x10).
  // CDW10 layout per NVMe 1.4 §5.11 Figure 184:
  //   bits  2:0  FS   (Firmware Slot)   = 0  (ignored for action=110b)
  //   bits  5:3  CA   (Commit Action)   = 110b (Download to BP)
  //   bits 30:6  reserved
  //   bit  31    BPID (Boot Partition ID) = TARGET_BPID
  Cmd.Cdw0.Opcode = NVME_ADMIN_FIRMWARE_COMMIT;
  Cmd.Cdw10       = ((UINT32)TARGET_BPID << 31)
                  | ((UINT32)NVME_FW_COMMIT_ACTION_DOWNLOAD_BP << 3);
  Cmd.Flags       = CDW10_VALID;

  ZeroMem (&Packet, sizeof (Packet));
  // 30 s timeout: a healthy Commit returns near-instantly (it just flips
  // pointers / activates staged image); a hung Commit means staging is
  // bad. Fail fast so the bench iteration stays short.
  Packet.CommandTimeout = 30ULL * 10000000ULL;
  Packet.QueueType      = NVME_ADMIN_QUEUE;
  Packet.NvmeCmd        = &Cmd;
  Packet.NvmeCompletion = &Completion;

  EFI_STATUS  Status = PassThru->PassThru (PassThru, 0, &Packet, NULL);
  *Dw3 = Completion.DW3;
  StatusField = (Completion.DW3 >> 17) & 0x7FFF;
  *Sc  = (StatusField >> 0) & 0xFF;
  *Sct = (StatusField >> 8) & 0x7;
  return Status;
}

//
// Find the NVMe controller's PCI IO protocol by enumerating PCI IO handles
// and matching class code 0x010802 (Mass Storage / NVM / NVMe). Surface
// systems have a single NVMe controller, so the first match is the one.
//
STATIC
EFI_STATUS
EFIAPI
FindNvmePciIo (
  OUT EFI_PCI_IO_PROTOCOL  **OutPci
  )
{
  EFI_STATUS  Status;
  EFI_HANDLE  *Handles    = NULL;
  UINTN       HandleCount = 0;
  UINTN       Index;

  *OutPci = NULL;
  Status = gBS->LocateHandleBuffer (ByProtocol, &gEfiPciIoProtocolGuid, NULL, &HandleCount, &Handles);
  if (EFI_ERROR (Status)) {
    LogPrint (L"  [pci] LocateHandleBuffer(PciIo): %r\n", Status);
    return Status;
  }
  for (Index = 0; Index < HandleCount; Index++) {
    EFI_PCI_IO_PROTOCOL  *Pci;
    if (EFI_ERROR (gBS->HandleProtocol (Handles[Index], &gEfiPciIoProtocolGuid, (VOID **)&Pci))) {
      continue;
    }
    UINT32  ClassDw = 0;
    if (EFI_ERROR (Pci->Pci.Read (Pci, EfiPciIoWidthUint32, 0x08, 1, &ClassDw))) {
      continue;
    }
    UINT32  ClassCode = (ClassDw >> 8) & 0x00FFFFFFu;
    if (ClassCode == 0x00010802u) {
      LogPrint (L"  [pci] NVMe controller handle matched class 0x010802 (raw dw=0x%08x)\n", ClassDw);
      *OutPci = Pci;
      FreePool (Handles);
      return EFI_SUCCESS;
    }
  }
  FreePool (Handles);
  return EFI_NOT_FOUND;
}

//
// Read up to 4 MiB from BP `BpId` at `OffsetBytes` (4 KiB aligned) into the
// caller's destination buffer via the BPMBL/BPRSEL/BPINFO MMIO drive loop.
// NVMe 1.4 §3.1.21. Captures the final BPINFO + BRS for diagnostics.
//
STATIC
EFI_STATUS
EFIAPI
BpReadChunk (
  IN  EFI_PCI_IO_PROTOCOL  *Pci,
  IN  UINT8                 BpId,
  IN  UINT64                OffsetBytes,
  IN  UINTN                 LenBytes,
  OUT UINT8                *Dest,
  OUT UINT32               *OutLastBpinfo,
  OUT UINT32               *OutBprselEnc,
  OUT UINT32               *OutBrsLast
  )
{
  EFI_STATUS            Status;
  EFI_PHYSICAL_ADDRESS  ScratchAddr  = 0;
  UINTN                 ScratchPages = (LenBytes + EFI_PAGE_SIZE - 1) / EFI_PAGE_SIZE;

  *OutLastBpinfo = 0;
  *OutBprselEnc  = 0;
  *OutBrsLast    = 0;

  if (LenBytes == 0 ||
      (LenBytes & (BP_CHUNK_ALIGNMENT - 1)) != 0 ||
      (OffsetBytes & (BP_CHUNK_ALIGNMENT - 1)) != 0) {
    return EFI_INVALID_PARAMETER;
  }

  Status = gBS->AllocatePages (AllocateAnyPages, EfiBootServicesData, ScratchPages, &ScratchAddr);
  if (EFI_ERROR (Status)) {
    return Status;
  }
  ZeroMem ((VOID *)(UINTN)ScratchAddr, LenBytes);

  // Step 1: BPMBL = scratch phys addr (64-bit write).
  UINT64  BpmblVal = ScratchAddr;
  Status = Pci->Mem.Write (Pci, EfiPciIoWidthUint64, 0, MMIO_OFFSET_BPMBL, 1, &BpmblVal);
  if (EFI_ERROR (Status)) {
    gBS->FreePages (ScratchAddr, ScratchPages);
    return Status;
  }

  // Step 2: BPRSEL = (BpId << 31) | (offset_4k << 10) | (size_4k - 1).
  UINT32  Offset4k  = (UINT32)(OffsetBytes / BP_CHUNK_ALIGNMENT);
  UINT32  Size4k    = (UINT32)(LenBytes    / BP_CHUNK_ALIGNMENT);
  UINT32  BprselVal = ((UINT32)BpId << 31) | ((Offset4k & 0x3FFFFu) << 10) | ((Size4k - 1) & 0x3FFu);
  *OutBprselEnc = BprselVal;
  Status = Pci->Mem.Write (Pci, EfiPciIoWidthUint32, 0, MMIO_OFFSET_BPRSEL, 1, &BprselVal);
  if (EFI_ERROR (Status)) {
    gBS->FreePages (ScratchAddr, ScratchPages);
    return Status;
  }

  // Step 3: poll BPINFO until BRS leaves "in progress".
  UINT32  BpinfoVal = 0;
  UINT32  Brs       = 0;
  UINTN   PollCount = 0;
  for (;;) {
    Status = Pci->Mem.Read (Pci, EfiPciIoWidthUint32, 0, MMIO_OFFSET_BPINFO, 1, &BpinfoVal);
    if (EFI_ERROR (Status)) {
      *OutLastBpinfo = BpinfoVal;
      gBS->FreePages (ScratchAddr, ScratchPages);
      return Status;
    }
    Brs = (BpinfoVal >> BPINFO_BRS_SHIFT) & BPINFO_BRS_MASK;
    if (Brs == BPINFO_BRS_IN_PROGRESS) {
      if (++PollCount > 10000000UL) {
        *OutLastBpinfo = BpinfoVal;
        *OutBrsLast    = Brs;
        gBS->FreePages (ScratchAddr, ScratchPages);
        return EFI_TIMEOUT;
      }
      continue;
    }
    break;
  }
  *OutLastBpinfo = BpinfoVal;
  *OutBrsLast    = Brs;

  if (Brs != BPINFO_BRS_SUCCESS) {
    gBS->FreePages (ScratchAddr, ScratchPages);
    return EFI_DEVICE_ERROR;
  }

  CopyMem (Dest, (VOID *)(UINTN)ScratchAddr, LenBytes);
  gBS->FreePages (ScratchAddr, ScratchPages);
  return EFI_SUCCESS;
}

//
// Get Log Page LID=0x15 (Boot Partition log) read. Used by VerifyBpFromMmio
// as a cross-check alongside the BPMBL/BPRSEL MMIO drive loop. CDW10 layout
// per NVMe 1.4 §5.14.1.16:
//   bits  7:0   LID    = 0x15
//   bits 14:8   LSP    = BPID  (which boot partition)
//   bit  15     RAE    = 0
//   bits 31:16  NUMDL  = (Bytes/4 - 1) lower 16 bits
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
  OUT UINT32                              *Dw3,
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
                  | 0x15u;
  Cmd.Cdw11       = (NumD >> 16) & 0xFFFFu;
  Cmd.Cdw12       = (UINT32)(OffsetBytes & 0xFFFFFFFFull);
  Cmd.Cdw13       = (UINT32)((OffsetBytes >> 32) & 0xFFFFFFFFull);
  Cmd.Flags       = CDW10_VALID | CDW11_VALID | CDW12_VALID | CDW13_VALID;

  ZeroMem (&Packet, sizeof (Packet));
  Packet.CommandTimeout = 10ULL * 10000000ULL;
  Packet.TransferBuffer = Buffer;
  Packet.TransferLength = (UINT32)Bytes;
  Packet.QueueType      = NVME_ADMIN_QUEUE;
  Packet.NvmeCmd        = &Cmd;
  Packet.NvmeCompletion = &Completion;

  EFI_STATUS  Status = PassThru->PassThru (PassThru, 0, &Packet, NULL);
  *Dw3 = Completion.DW3;
  StatusField = (Completion.DW3 >> 17) & 0x7FFF;
  *Sc  = (StatusField >> 0) & 0xFF;
  *Sct = (StatusField >> 8) & 0x7;
  return Status;
}

STATIC
EFI_STATUS
WriteBpTestPattern (
  IN EFI_NVM_EXPRESS_PASS_THRU_PROTOCOL  *PassThru
  )
{
  EFI_STATUS                                Status;
  UINT8                                     *WimBuffer;
  UINTN                                     WimPages;
  UINT64                                    WimFileBytes;
  UINT64                                    WimPaddedBytes;
  UINT8                                     Mdts;
  UINT8                                     Fwug;
  UINTN                                     ChunkBytes;
  UINTN                                     GranularityBytes;
  UINTN                                     MaxTransferBytes;
  UINT32                                    Dw3;
  UINT32                                    Sct;
  UINT32                                    Sc;

  //
  // === Step 0: query Identify Controller for MDTS + FWUG so the chunk size
  // we feed Firmware Image Download is actually valid.
  //
  Mdts = 0;
  Fwug = 0;
  UINT16 Oacs = 0;
  Status = QueryIdentifyController (PassThru, &Mdts, &Fwug, &Oacs, NULL);
  if (EFI_ERROR (Status)) {
    LogPrint (L"  [0/2] Identify Controller failed (%r); using 4 KiB default\n", Status);
    GranularityBytes = SIZE_4KB;
    MaxTransferBytes = SIZE_64KB;
  } else {
    LogPrint (L"  [0/3] Identify Controller: MDTS=%u  FWUG=0x%02x  OACS=0x%04x\n", Mdts, Fwug, Oacs);
    LogPrint (L"        OACS bits: SecSendRecv=%u FormatNVM=%u FwCommitDownload=%u NsMgmt=%u SelfTest=%u Directives=%u NVMeMI=%u VirtMgmt=%u DoorbellBufCfg=%u GetLBAStatus=%u\n",
              (Oacs >> 0) & 1, (Oacs >> 1) & 1, (Oacs >> 2) & 1, (Oacs >> 3) & 1, (Oacs >> 4) & 1,
              (Oacs >> 5) & 1, (Oacs >> 6) & 1, (Oacs >> 7) & 1, (Oacs >> 8) & 1, (Oacs >> 9) & 1);
    if ((Oacs >> 10) != 0) {
      LogPrint (L"        OACS upper bits (10..15) NON-ZERO = 0x%04x  <-- possible vendor flags!\n",
                (Oacs >> 10) & 0x3F);
    }
    // FWUG: 0 = no info -> default to 4 KiB; 0xFF = no restriction; otherwise N * 4 KiB
    if (Fwug == 0 || Fwug == 0xFF) {
      GranularityBytes = SIZE_4KB;
    } else {
      GranularityBytes = (UINTN)Fwug * SIZE_4KB;
    }
    // MDTS: 0 = no limit; N -> 2^N * page (page = 4 KiB assumed; CAP.MPSMIN read needed for exact)
    if (Mdts == 0) {
      MaxTransferBytes = SIZE_1MB;
    } else if (Mdts >= 20) {
      MaxTransferBytes = SIZE_1MB;
    } else {
      MaxTransferBytes = (UINTN)SIZE_4KB << Mdts;
    }
    LogPrint (L"        => granularity=%lu bytes, MDTS cap=%lu bytes\n",
           (UINT64)GranularityBytes, (UINT64)MaxTransferBytes);
  }

  // Use FWUG granularity for chunk size. Larger chunks up to MDTS would be
  // faster, but the Pass-Thru path on this platform isn't reliably tested at
  // MDTS-sized transfers; keep to the known-good FWUG granularity.
  ChunkBytes = GranularityBytes;
  if (ChunkBytes > MaxTransferBytes) {
    ChunkBytes = MaxTransferBytes;
  }

  //
  // Pre-load the WIM into DRAM before any admin commands. Once Phase A
  // starts, the only thing that can touch the load volume is the deferred
  // log flush at the very end — the upload loop reads chunks from this
  // buffer, not from disk.
  //
  Status = LoadWimIntoBuffer (
             WIM_FILE_PATH,
             ChunkBytes,
             &WimBuffer,
             &WimPages,
             &WimFileBytes,
             &WimPaddedBytes
             );
  if (EFI_ERROR (Status)) {
    LogPrint (L"  [wim] preload failed: %r\n", Status);
    gResult.LastEfiStatus = (INT32)Status;
    return Status;
  }
  gResult.WimFileBytes   = WimFileBytes;
  gResult.WimPaddedBytes = WimPaddedBytes;
  gResult.PhaseReached   = BP_PHASE_WIM_LOADED;
  WriteResultToNvRam ();
  LogPrint (L"        => chunk size %lu bytes; WIM file=%lu bytes (%lu MiB), padded=%lu bytes (%lu MiB, %lu chunks)\n",
            (UINT64)ChunkBytes,
            WimFileBytes, WimFileBytes / SIZE_1MB,
            WimPaddedBytes, WimPaddedBytes / SIZE_1MB,
            WimPaddedBytes / ChunkBytes);

  //
  // === BP write unlock (Set Features FID=0x85) ===
  // Required by this controller before Firmware Image Download / Firmware
  // Commit will actually take effect. Without it the commit returns SC=0x00
  // but BP contents stay unchanged.
  //
  LogPrint (L"\n=== Unlock: Set Features FID=0x85 CDW11=0x09 (BP0+BP1 = Write Unlocked) ===\n");
  {
    // Read the write-protection state around the unlock so the controller's
    // actual BP0WPS/BP1WPS behavior is observable, not inferred from write
    // failures. Read-only; failures are logged and do not gate the flow.
    UINT32      WpsDw0, WpsSct, WpsSc;
    EFI_STATUS  WpsStatus = ReadBpWriteProtection (PassThru, &WpsDw0, &WpsSct, &WpsSc);
    gResult.BpWpsPreDw0    = WpsDw0;
    gResult.BpWpsPreStatus = EFI_ERROR (WpsStatus) ? 0xFFFFFFFF : ((WpsSct << 8) | WpsSc);
    if (EFI_ERROR (WpsStatus) || WpsSct != 0 || WpsSc != 0) {
      LogPrint (L"  Get Features FID=0x85 (pre-unlock): %r SCT=%u SC=0x%02x\n", WpsStatus, WpsSct, WpsSc);
    } else {
      LogBpWpsState (L"BPWPS before unlock", WpsDw0);
    }
  }
  Status = UnlockBpWriteProtection (PassThru, &Dw3, &Sct, &Sc);
  gResult.VendorEnableDw3       = Dw3;
  gResult.VendorEnableStatus    = (Sct << 8) | Sc;
  gResult.VendorEnableEfiStatus = (INT32)Status;
  gResult.PhaseReached          = BP_PHASE_VENDOR_WRITE_ENABLE;
  WriteResultToNvRam ();
  LogPrint (L"  Set Features FID=0x85: %r  DW3=0x%08x  SCT=%u SC=0x%02x\n",
            Status, Dw3, Sct, Sc);
  {
    UINT32      WpsDw0, WpsSct, WpsSc;
    EFI_STATUS  WpsStatus = ReadBpWriteProtection (PassThru, &WpsDw0, &WpsSct, &WpsSc);
    gResult.BpWpsPostDw0    = WpsDw0;
    gResult.BpWpsPostStatus = EFI_ERROR (WpsStatus) ? 0xFFFFFFFF : ((WpsSct << 8) | WpsSc);
    WriteResultToNvRam ();
    if (EFI_ERROR (WpsStatus) || WpsSct != 0 || WpsSc != 0) {
      LogPrint (L"  Get Features FID=0x85 (post-unlock): %r SCT=%u SC=0x%02x\n", WpsStatus, WpsSct, WpsSc);
    } else {
      LogBpWpsState (L"BPWPS after unlock ", WpsDw0);
    }
  }
  if (EFI_ERROR (Status) || Sct != 0 || Sc != 0) {
    LogPrint (L"  unlock FAILED — aborting before Phase A (download would silently no-op)\n");
    gResult.LastEfiStatus = (INT32)Status;
    gResult.LastDw3       = Dw3;
    gResult.LastSct       = Sct;
    gResult.LastSc        = Sc;
    gBS->FreePages ((EFI_PHYSICAL_ADDRESS)(UINTN)WimBuffer, WimPages);
    return Status;
  }

  //
  // === Phase A: upload via Firmware Image Download with CDW12=1 ===
  //
  LogPrint (L"\n=== Phase A: upload via Firmware Image Download CDW12=1 ===\n");

  // WIM loader guarantees WimPaddedBytes == TARGET_UPLOAD_BYTES (rejects
  // larger files, zero-pads smaller ones).
  UINT64  UploadBytes = WimPaddedBytes;
  UINTN   MaxChunks   = (UINTN)(UploadBytes / ChunkBytes);
  UINTN   ChunkIndex;
  UINT64  Offset = 0;
  UINT64  LastGoodBytes = 0;
  BOOLEAN PhaseAFailed = FALSE;
  UINT32  FailDw3 = 0;
  UINT32  FailSct = 0;
  UINT32  FailSc  = 0;
  EFI_STATUS  FailStatus = EFI_SUCCESS;

  for (ChunkIndex = 0; ChunkIndex < MaxChunks; ChunkIndex++) {
    Status = SendDownloadChunk (PassThru, WimBuffer + Offset, ChunkBytes, Offset, &Dw3, &Sct, &Sc);
    if (EFI_ERROR (Status) || Sct != 0 || Sc != 0) {
      PhaseAFailed = TRUE;
      FailStatus   = Status;
      FailDw3      = Dw3;
      FailSct      = Sct;
      FailSc       = Sc;
      break;
    }
    LastGoodBytes = Offset + ChunkBytes;
    Offset += ChunkBytes;
  }
  EFI_STATUS  PhaseAStatus = EFI_SUCCESS;
  if (PhaseAFailed) {
    PhaseAStatus = EFI_ERROR (FailStatus) ? FailStatus : EFI_DEVICE_ERROR;
  }

  gResult.PhaseReached  = BP_PHASE_A_DOWNLOAD;
  gResult.BytesUploaded = LastGoodBytes;
  gResult.LastDw3       = PhaseAFailed ? FailDw3 : Dw3;
  gResult.LastSct       = PhaseAFailed ? FailSct : Sct;
  gResult.LastSc        = PhaseAFailed ? FailSc  : Sc;
  gResult.LastEfiStatus = PhaseAFailed ? (INT32)PhaseAStatus : (INT32)EFI_SUCCESS;
  WriteResultToNvRam ();

  if (PhaseAFailed) {
    LogPrint (
      L"  FAIL at cumulative=%lu bytes (%lu MiB), OFST=%lu bytes: %r  DW3=0x%08x  SCT=%u SC=0x%02x\n",
      LastGoodBytes, LastGoodBytes / SIZE_1MB, Offset, PhaseAStatus, FailDw3, FailSct, FailSc
      );
    gBS->FreePages ((EFI_PHYSICAL_ADDRESS)(UINTN)WimBuffer, WimPages);
    return PhaseAStatus;
  }

  LogPrint (L"  uploaded %lu MiB (%lu chunks) without failure\n",
            UploadBytes / SIZE_1MB, (UINT64)MaxChunks);

  //
  // === Phase B: Firmware Commit (CA=110b Download to BP, BPID=N) ===
  // 30 s timeout — short on purpose. A healthy Commit returns near-instantly;
  // a hang means the staged image is bad (or the controller wants something
  // we haven't done) and we'd rather hear about it fast than burn the bench.
  //
  LogPrint (L"\n=== Phase B: Firmware Commit action=110b BPID=%u ===\n", TARGET_BPID);
  gResult.PhaseReached = BP_PHASE_B_COMMIT;
  WriteResultToNvRam ();

  Status = SendDownloadBpCommit (PassThru, &Dw3, &Sct, &Sc);
  LogPrint (L"  Firmware Commit: %r  DW3=0x%08x  SCT=%u SC=0x%02x\n",
            Status, Dw3, Sct, Sc);

  gResult.LastDw3         = Dw3;
  gResult.LastSct         = Sct;
  gResult.LastSc          = Sc;
  gResult.LastEfiStatus   = (INT32)Status;
  // Commit returning EFI_WARN_RESET_REQUIRED (0x00000007) is per spec for
  // CA=110b — success-with-warning, not a failure. Accept any of
  // (EFI_SUCCESS, EFI_WARN_RESET_REQUIRED) so long as NVMe SCT/SC == 0.
  gResult.EndToEndSuccess =
    ((Status == EFI_SUCCESS || Status == EFI_WARN_RESET_REQUIRED) && Sct == 0 && Sc == 0) ? 1 : 0;
  WriteResultToNvRam ();

  // BP re-lock is intentionally NOT done here. Lock/unlock is owned by
  // the FMP capsule update flow.

  // Surface a non-zero NVMe completion as EFI_DEVICE_ERROR so the caller
  // doesn't advance to BP_PHASE_DONE on a failed write.
  EFI_STATUS  ReturnStatus = Status;
  if ((gResult.EndToEndSuccess == 0) && !EFI_ERROR (Status)) {
    ReturnStatus = EFI_DEVICE_ERROR;
  }

  gBS->FreePages ((EFI_PHYSICAL_ADDRESS)(UINTN)WimBuffer, WimPages);
  return ReturnStatus;
}

//
// VERIFY-mode worker. Called on the second boot, after the WRITE-mode run
// + cold reset has settled the controller. Two independent BP read paths:
//   1. BPMBL/BPRSEL/BPINFO MMIO drive loop (NVMe 1.4 §3.1.21)
//   2. Get Log Page LID=0x15 (NVMe 1.4 §5.14.1.16) with 64 KiB MSWIM scan
// Captures the first 32 bytes from each path into NVRAM for cross-check.
//
STATIC
EFI_STATUS
EFIAPI
VerifyBpFromMmio (
  IN EFI_NVM_EXPRESS_PASS_THRU_PROTOCOL  *PassThru
  )
{
  STATIC CONST UINT8  WimSignature[8] = { 0x4D, 0x53, 0x57, 0x49, 0x4D, 0x00, 0x00, 0x00 };
  EFI_STATUS          Status;
  EFI_PCI_IO_PROTOCOL *Pci = NULL;

  // V.0: locate NVMe controller's PCI IO protocol.
  Status = FindNvmePciIo (&Pci);
  if (EFI_ERROR (Status)) {
    LogPrint (L"  [verify] FindNvmePciIo: %r — cannot proceed with MMIO read\n", Status);
    return Status;
  }

  // V.1: read raw BPINFO and decode.
  UINT32 BpinfoStart = 0;
  Status = Pci->Mem.Read (Pci, EfiPciIoWidthUint32, 0, MMIO_OFFSET_BPINFO, 1, &BpinfoStart);
  gResult.MmioBpinfoStart = BpinfoStart;
  UINT32 Bpsz       = BpinfoStart & BPINFO_BPSZ_MASK;
  UINT32 BrsAtStart = (BpinfoStart >> BPINFO_BRS_SHIFT)   & BPINFO_BRS_MASK;
  UINT32 AbpidStart = (BpinfoStart >> BPINFO_ABPID_SHIFT) & BPINFO_ABPID_MASK;
  LogPrint (L"  [bpinfo] raw=0x%08x  BPSZ=0x%x (128 KiB units)  BRS=%u  ABPID=%u\n",
            BpinfoStart, Bpsz, BrsAtStart, AbpidStart);
  gResult.PhaseReached = BP_PHASE_VERIFY_BPINFO_READ;
  WriteResultToNvRam ();
  if (EFI_ERROR (Status)) {
    LogPrint (L"  [verify] BPINFO read failed: %r\n", Status);
    return Status;
  }

  // V.2: BpReadChunk(BPID=TARGET_BPID, offset=0, 4 KiB) via BPMBL/BPRSEL drive loop.
  LogPrint (L"\n=== Verify V.2: BPMBL/BPRSEL read of BP%u offs=0 len=4096 ===\n", TARGET_BPID);
  UINT8  *Buf = AllocateZeroPool (4096);
  if (Buf == NULL) {
    EFI_STATUS  OorStatus = EFI_OUT_OF_RESOURCES;
    gResult.MmioReadEfiStatus = (INT32)OorStatus;
    LogPrint (L"  [verify] AllocateZeroPool(4096) failed\n");
    return OorStatus;
  }
  UINT32 LastBpinfo = 0;
  UINT32 BprselEnc  = 0;
  UINT32 BrsLast    = 0;
  Status = BpReadChunk (Pci, TARGET_BPID, 0, 4096, Buf, &LastBpinfo, &BprselEnc, &BrsLast);
  gResult.MmioBprselEnc     = BprselEnc;
  gResult.MmioBrsLast       = BrsLast;
  gResult.MmioReadEfiStatus = (INT32)Status;
  CopyMem (gResult.MmioReadFirst32, Buf, 32);

  LogPrint (L"  BPRSEL written: 0x%08x  result=%r  final BPINFO=0x%08x  BRS=%u\n",
            BprselEnc, Status, LastBpinfo, BrsLast);
  LogHexDump32 (L"  first 32 bytes: ", Buf);
  LogPrint (L"  MSWIM signature: %s\n",
            (CompareMem (Buf, WimSignature, sizeof (WimSignature)) == 0) ? L"MATCH" : L"MISMATCH");
  FreePool (Buf);
  gResult.PhaseReached = BP_PHASE_VERIFY_MMIO_READ;
  WriteResultToNvRam ();

  // V.3: second BPINFO read after the drive loop. BRS should reflect the
  // outcome of our DMA (IDLE/SUCCESS/ERROR); ABPID should be unchanged.
  UINT32 BpinfoAfter = 0;
  EFI_STATUS  ReadAgainStatus = Pci->Mem.Read (Pci, EfiPciIoWidthUint32, 0, MMIO_OFFSET_BPINFO, 1, &BpinfoAfter);
  gResult.MmioBpinfoSecondRead = BpinfoAfter;
  UINT32 AbpidAfter = (BpinfoAfter >> BPINFO_ABPID_SHIFT) & BPINFO_ABPID_MASK;
  LogPrint (L"  [bpinfo second read] raw=0x%08x  ABPID=%u (was %u at start)  read-status=%r\n",
            BpinfoAfter, AbpidAfter, AbpidStart, ReadAgainStatus);
  gResult.PhaseReached = BP_PHASE_VERIFY_BPINFO_RECHECK;
  WriteResultToNvRam ();

  // V.4: Get Log Page LID=0x15 LSP=BPID, 64 KiB read with MSWIM scan.
  // Independent of the BPMBL/BPRSEL MMIO path above. MDTS-bounded; 64 KiB
  // is well within the controllers in scope.
  STATIC CONST UINT32  LidReadBytes = 64 * 1024;
  LogPrint (L"\n=== Verify V.4: Get Log Page LID=0x15 BPID=%u offs=0 len=%u with MSWIM scan ===\n",
            TARGET_BPID, LidReadBytes);
  UINT8  *LogBuf = AllocateZeroPool (LidReadBytes);
  if (LogBuf == NULL) {
    EFI_STATUS  OorStatus = EFI_OUT_OF_RESOURCES;
    gResult.LidPageEfiStatus = (INT32)OorStatus;
    gResult.LidPageMswimOffset = 0xFFFFFFFFu;
    LogPrint (L"  [lid15] AllocateZeroPool(%u) failed\n", LidReadBytes);
    return OorStatus;
  }
  UINT32 LDw3 = 0, LSct = 0, LSc = 0;
  EFI_STATUS  LidStatus = GetBpLogPage (PassThru, TARGET_BPID, 0, LidReadBytes, LogBuf, &LDw3, &LSct, &LSc);
  gResult.LidPageDw3        = LDw3;
  gResult.LidPageStatus     = (LSct << 8) | LSc;
  gResult.LidPageEfiStatus  = (INT32)LidStatus;
  gResult.LidPageBytesRead  = LidReadBytes;
  CopyMem (gResult.LidPageFirst32, LogBuf, 32);

  // Scan the whole returned buffer for the WIM signature.
  UINT32  MswimOffset = 0xFFFFFFFFu;
  for (UINT32 I = 0; I + sizeof (WimSignature) <= LidReadBytes; I++) {
    if (CompareMem (LogBuf + I, WimSignature, sizeof (WimSignature)) == 0) {
      MswimOffset = I;
      break;
    }
  }
  gResult.LidPageMswimOffset = MswimOffset;

  LogPrint (L"  LID 0x15: %r  DW3=0x%08x  SCT=%u SC=0x%02x\n", LidStatus, LDw3, LSct, LSc);
  LogHexDump32 (L"  first 32 bytes: ", LogBuf);
  if (MswimOffset == 0xFFFFFFFFu) {
    LogPrint (L"  MSWIM scan: NOT FOUND in first %u bytes\n", LidReadBytes);
  } else {
    LogPrint (L"  MSWIM scan: FOUND at offset 0x%x (%u bytes into the log page)\n",
              MswimOffset, MswimOffset);
  }
  FreePool (LogBuf);
  gResult.PhaseReached = BP_PHASE_VERIFY_LID15_READ;
  WriteResultToNvRam ();

  return EFI_SUCCESS;
}

EFI_STATUS
EFIAPI
EntryPoint (
  IN EFI_HANDLE        ImageHandle,
  IN EFI_SYSTEM_TABLE  *SystemTable
  )
{
  EFI_STATUS                          Status;
  EFI_HANDLE                          *Handles;
  UINTN                               HandleCount;
  UINTN                               Index;
  UINTN                               Tested;
  EFI_NVM_EXPRESS_PASS_THRU_PROTOCOL  *PassThru;

  Handles = NULL;
  Tested  = 0;

  EFI_STATUS  SafetyStatus;

  gAppImageHandle = ImageHandle;

  LogPrint (L"NvmeBpWrite v%a — target=BP%u (content-hash-driven, no NVRAM state)\n",
            NVME_BP_WRITE_VERSION, TARGET_BPID);

  // === Boot-safety prologue ===
  SafetyStatus = gRT->SetVariable (L"BootNext", &gEfiGlobalVariableGuid, 0, 0, NULL);
  LogPrint (L"[safety] BootNext cleared: %r\n", SafetyStatus);
  gBS->SetWatchdogTimer (0, 0, 0, NULL);
  LogPrint (L"[safety] BDS watchdog disabled\n\n");

  LogPrint (L"  Step 1: read first %u bytes of \\ValidationOS.wim from USB\n", (UINT32)CHECK_BYTES);
  LogPrint (L"  Step 2: read first %u bytes of BP%u via Get Log Page LID=0x15 (skip 16B preamble)\n",
            (UINT32)CHECK_BYTES, TARGET_BPID);
  LogPrint (L"  Step 3: compare. If match -> BP%u already current; cold-reset.\n", TARGET_BPID);
  LogPrint (L"  Step 4: if mismatch (or %s present) -> unlock + CDW12=1 upload + commit BPID=%u\n",
            FORCE_REFLASH_FLAG_PATH, TARGET_BPID);
  LogPrint (L"  Step 5: post-write readback; confirm BP%u matches WIM. Cold-reset.\n\n", TARGET_BPID);

  // === Check for force-reflash flag on USB ===
  // If the operator dropped \force-reflash.flag on the USB, skip the content
  // check and run WRITE unconditionally. One-shot semantics: the flag stays
  // on the USB after consumption, so subsequent boots will re-flash again
  // until the operator deletes it. (Intentional — lets the operator do an
  // explicit clean re-write of a possibly-corrupt BP1 multiple times.)
  BOOLEAN  ForceFromFlag = FileExistsOnAnyVolume (FORCE_REFLASH_FLAG_PATH);
  if (ForceFromFlag) {
    LogPrint (L"[force] %s present on USB — content check skipped; running WRITE unconditionally\n",
              FORCE_REFLASH_FLAG_PATH);
    LogPrint (L"        (delete the flag file from the USB after use to revert to normal idempotent re-boots)\n\n");
  }

  Status = gBS->LocateHandleBuffer (
                  ByProtocol, &gEfiNvmExpressPassThruProtocolGuid,
                  NULL, &HandleCount, &Handles);
  if (EFI_ERROR (Status)) {
    LogPrint (L"LocateHandleBuffer(NvmePassThru): %r\n", Status);
    goto Reset;
  }

  for (Index = 0; Index < HandleCount; Index++) {
    Status = gBS->HandleProtocol (
                    Handles[Index],
                    &gEfiNvmExpressPassThruProtocolGuid,
                    (VOID **)&PassThru);
    if (EFI_ERROR (Status)) continue;
    Tested++;

    LogPrint (L"--- NVMe controller #%u ---\n", (UINT32)Tested);

    UINT8  *WimHead = AllocateZeroPool (CHECK_BYTES);
    UINT8  *BpHead  = AllocateZeroPool (CHECK_BYTES + LID15_PREAMBLE_BYTES);
    if (WimHead == NULL || BpHead == NULL) {
      LogPrint (L"  AllocateZeroPool failed\n");
      if (WimHead != NULL) FreePool (WimHead);
      if (BpHead  != NULL) FreePool (BpHead);
      continue;
    }

    // Step 1: WIM head
    UINTN  WimHeadRead = 0;
    Status = ReadWimHead (WIM_FILE_PATH, CHECK_BYTES, WimHead, &WimHeadRead);
    if (EFI_ERROR (Status) || WimHeadRead < CHECK_BYTES) {
      LogPrint (L"  [wim] ReadWimHead failed: %r  bytes=%lu (expected %lu)\n",
                Status, (UINT64)WimHeadRead, (UINT64)CHECK_BYTES);
      FreePool (WimHead);
      FreePool (BpHead);
      continue;
    }
    LogPrint (L"  [wim] first %u bytes loaded\n", (UINT32)CHECK_BYTES);
    LogHexDump32 (L"  [wim] first 32 bytes: ", WimHead);

    // Step 2: BP head via LID 0x15
    UINT32  Dw3 = 0, Sct = 0, Sc = 0;
    EFI_STATUS  LidStatus = GetBpLogPage (
                              PassThru, TARGET_BPID, 0,
                              (UINT32)(CHECK_BYTES + LID15_PREAMBLE_BYTES),
                              BpHead, &Dw3, &Sct, &Sc);
    if (EFI_ERROR (LidStatus)) {
      LogPrint (L"  [bp]  GetBpLogPage: %r  SCT=%u SC=0x%02x\n", LidStatus, Sct, Sc);
      FreePool (WimHead);
      FreePool (BpHead);
      continue;
    }
    LogPrint (L"  [bp]  first %u bytes read\n", (UINT32)CHECK_BYTES);
    LogHexDump32 (L"  [bp]  first 32 bytes: ", BpHead + LID15_PREAMBLE_BYTES);

    // Step 3: Compare
    BOOLEAN  Match = (CompareMem (BpHead + LID15_PREAMBLE_BYTES, WimHead, CHECK_BYTES) == 0);
    LogPrint (L"\n  Content check (%u bytes): %s\n",
              (UINT32)CHECK_BYTES,
              Match ? L"MATCH — BP1 already contains this WIM" : L"DIFFERS — write needed");

    if (Match && !ForceFromFlag) {
      FreePool (WimHead);
      FreePool (BpHead);
      LogPrint (L"  Nothing to do. Cold-resetting to next boot target.\n");
      break;
    }
    if (Match && ForceFromFlag) {
      LogPrint (L"  Force-reflash flag overrides; re-writing anyway.\n");
    }

    // Step 4: WRITE (unchanged path — does its own WIM preload + unlock + upload + commit)
    FreePool (WimHead);
    FreePool (BpHead);
    LogPrint (L"\n  Running WRITE (unlock + Phase A upload + Phase B commit)...\n\n");
    EFI_STATUS  WriteStatus = WriteBpTestPattern (PassThru);
    if (EFI_ERROR (WriteStatus)) {
      LogPrint (L"  WriteBpTestPattern: %r\n", WriteStatus);
      break;
    }

    // Step 5: post-write readback
    LogPrint (L"\n  === Post-write readback ===\n");
    WimHead = AllocateZeroPool (CHECK_BYTES);
    BpHead  = AllocateZeroPool (CHECK_BYTES + LID15_PREAMBLE_BYTES);
    if (WimHead == NULL || BpHead == NULL) {
      LogPrint (L"  alloc failed in readback\n");
      if (WimHead != NULL) FreePool (WimHead);
      if (BpHead  != NULL) FreePool (BpHead);
      break;
    }
    if (EFI_ERROR (ReadWimHead (WIM_FILE_PATH, CHECK_BYTES, WimHead, &WimHeadRead)) ||
        WimHeadRead < CHECK_BYTES) {
      LogPrint (L"  ReadWimHead failed in readback\n");
    } else if (EFI_ERROR (GetBpLogPage (
                            PassThru, TARGET_BPID, 0,
                            (UINT32)(CHECK_BYTES + LID15_PREAMBLE_BYTES),
                            BpHead, &Dw3, &Sct, &Sc))) {
      LogPrint (L"  GetBpLogPage failed in readback\n");
    } else {
      LogHexDump32 (L"  [bp]  post-write first 32 bytes: ", BpHead + LID15_PREAMBLE_BYTES);
      BOOLEAN  PostMatch = (CompareMem (BpHead + LID15_PREAMBLE_BYTES, WimHead, CHECK_BYTES) == 0);
      LogPrint (L"  Post-write content check (%u bytes): %s\n",
                (UINT32)CHECK_BYTES,
                PostMatch ? L"PASS — BP1 now matches WIM" : L"FAIL — BP1 still differs from WIM");
    }
    FreePool (WimHead);
    FreePool (BpHead);
    break;  // process only the first matching NVMe controller
  }

  if (Tested == 0) {
    LogPrint (L"\nNo NVMe Pass-Thru protocol handles found.\n");
  }

  if (Handles != NULL) FreePool (Handles);

Reset:
  SafetyStatus = gRT->SetVariable (L"BootNext", &gEfiGlobalVariableGuid, 0, 0, NULL);
  LogPrint (L"\nBootNext cleared: %r\n", SafetyStatus);

  gRT->ResetSystem (EfiResetCold, EFI_SUCCESS, 0, NULL);
  return EFI_SUCCESS;
}
