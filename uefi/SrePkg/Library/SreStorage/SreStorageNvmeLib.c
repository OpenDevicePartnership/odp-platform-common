/** @file
  Secure Recovery Environment (SRE) NVMe support for the FMP Device Library.

  Copyright (c) Microsoft Corporation. All rights reserved.

  SPDX-License-Identifier: BSD-2-Clause-Patent

**/
#include <PiDxe.h>
#include <IndustryStandard/Nvme.h>
#include <Library/DebugLib.h>
#include <Library/BaseMemoryLib.h>
#include <Library/MemoryAllocationLib.h>
#include <Library/DevicePathLib.h>
#include <Library/PcdLib.h>
#include <Library/UefiBootServicesTableLib.h>
#include <Protocol/DevicePath.h>
#include <Protocol/PciIo.h>
#include <Protocol/NvmExpressPassthru.h>
#include <Library/SreStorage.h>

//
// NVMe field encodings and offsets used by the Boot Partition path that have
// no (or no convenient) definition in <IndustryStandard/Nvme.h>, and are not
// already provided by SreStorage.h. Spec citations are in the NvmeBpWrite
// reference application and NVMe Base Spec 2.1 (§8.1.3 Boot Partitions,
// §5.1.25.1.32 BP Write Protection Config).
//
#define SRE_NVME_FW_COMMIT_ACTION_DOWNLOAD_BP 0x6   // Firmware Commit CDW10 bits 5:3 = 110b (Download to BP)
#define SRE_NVME_FW_COMMIT_BPID_SHIFT         31    // Firmware Commit CDW10 bit 31 = BPID
#define SRE_NVME_FW_COMMIT_ACTION_SHIFT       3     // Firmware Commit CDW10 bits 5:3 = CA
#define SRE_FW_IMG_DOWNLOAD_CDW12_BP_DATA     0x1   // vendor route hint: data for BP download

//
// Set/Get Features FID=0x85 (Boot Partition Write Protection Config) field
// encodings. CDW11 (Set) and completion Dword 0 (Get) share this layout.
//
#define SRE_NVME_FID_BP_WRITE_PROTECTION_CFG  0x85
#define SRE_BPWPS_FIELD_MASK                  0x7
#define SRE_BPWPS_BP1_SHIFT                   3     // bits 5:3 = BP1WPS
#define SRE_BPWPS_BP0_SHIFT                   0     // bits 2:0 = BP0WPS

//
// Identify Controller (CNS=01h) layout: a 4 KiB structure with the fields we
// need at fixed byte offsets (NVMe Base Spec, Identify Controller data).
//
#define SRE_NVME_IDENTIFY_BUFFER_SIZE         4096
#define SRE_NVME_ID_CTRL_OFFSET_FWUG          319   // 1 byte: Firmware Update Granularity

//
// Default to 1 page if granularity reported by FWUG is 0 (no info) or 0xFF (no restriction)
//
#define SRE_NVME_DEFAULT_GRANULARITY          1

//
// Boot Partition geometry via the controller's PCI BAR0 MMIO registers
// (NVMe Base Spec §3.1, BPINFO). Readable whenever the controller is powered.
//
#define SRE_NVME_BAR0_INDEX            0
#define SRE_NVME_BPINFO_BPSZ_MASK      0x7FFF       // bits 14:0, BP size in 128 KiB units

//
// Boot Partition read via Get Log Page LID 0x15 field encodings (NVMe Base
// Spec 2.1 §8.1.3 / §5.1.12). Used by SreStorageRead.
//
#define SRE_NVME_BP_LOG_HEADER_SIZE    16    // 16-byte header prepended to the LID 0x15 stream
#define SRE_NVME_LSP_BPID_MASK         0x7F  // Get Log Page CDW10 LSP field carries the BPID

//
// Boot Partition Write Protection State (BPxWPS): the 3-bit NVMe field values
// (NVMe Base Spec 2.1 §5.1.25.1.32, Boot Partition Write Protection Config):
//   000b  Change in state not requested
//   001b  Write Unlocked
//   010b  Write Locked
//   011b  Write Locked Until Power Cycle
//   100b  Write Protection controlled by RPMB
//
#define SRE_BPWPS_WRITE_UNLOCKED                  0x1  // 001b Write Unlocked
#define SRE_BPWPS_WRITE_LOCKED                    0x2  // 010b Write Locked
#define SRE_BPWPS_WRITE_LOCKED_UNTIL_POWER_CYCLE  0x3  // 011b Write Locked Until Power Cycle

//
// Logical write-protection state of a Boot Partition, used by NvmeSetLockState.
// Values are the NVMe BPxWPS field encodings (SRE_BPWPS_*), named with the
// spec's Boot Partition Write Protection State terminology.
//
typedef enum NVME_LOCK_STATE {
  WriteUnlocked              = SRE_BPWPS_WRITE_UNLOCKED,
  WriteLocked                = SRE_BPWPS_WRITE_LOCKED,
  WriteLockedUntilPowerCycle = SRE_BPWPS_WRITE_LOCKED_UNTIL_POWER_CYCLE
} NVME_LOCK_STATE;

//
// Write-session state tracked between SreStorageWriteOpen and SreStorageWriteClose
//
typedef struct {
  PARTITION_INDEX  PartitionIndex;
  UINTN            BlockSize;
  UINTN            BlockCount;
  UINTN            BlocksWritten;
} SRE_WRITE_SESSION;

SRE_WRITE_SESSION  mSreWriteSession = {0, 0, 0, 0};
BOOLEAN mIsSupported = FALSE;
EFI_NVM_EXPRESS_PASS_THRU_PROTOCOL  *mNvmePassThru = NULL;
EFI_PCI_IO_PROTOCOL  *mPciIo = NULL;

//
// Private functions
//

// A targeted connect to the device specified by the supplied device path
//
EFI_STATUS
EFIAPI
ConnectStorageDevice(
  IN  EFI_DEVICE_PATH_PROTOCOL  *TargetPath,
  OUT EFI_HANDLE                *Handle
)
{
  EFI_STATUS                Status;
  EFI_DEVICE_PATH_PROTOCOL  *RemainingPath;
  EFI_HANDLE                PreviousHandle;

  if ((TargetPath == NULL) || IsDevicePathEnd (TargetPath)) {
    return EFI_INVALID_PARAMETER;
  }

  // Targeted connect of just this device path (not a connect-all).
  PreviousHandle = NULL;
  *Handle = NULL;
  do {

    // LocateDevicePath returns a handle to the target device or its closest parent if not found.  If successful but
    // the handle returned is the same as the previous handle, it means the device could not be located.
    RemainingPath = TargetPath;
    Status = gBS->LocateDevicePath (&gEfiDevicePathProtocolGuid, &RemainingPath, Handle);
    if (!EFI_ERROR(Status) && PreviousHandle == *Handle) {
      Status = EFI_NOT_FOUND;
    }
    if (EFI_ERROR(Status)) {
      return Status;
    }
    PreviousHandle = *Handle;

    // Perform a connect of this device to enumerate its children
    Status = gBS->ConnectController (*Handle, NULL, NULL, FALSE);
    if (EFI_ERROR (Status)) {
      return Status;
    }

  // If RemainingPath is the DeviceEndPath node, we just connected our target device and can exit the loop
  } while (!IsDevicePathEnd (RemainingPath));

  return EFI_SUCCESS;
}

// Common private function to initiate a PassThru call and verify the completion status of the NVME command.
//
EFI_STATUS
EFIAPI
ExecuteNvmePassThru (
  IN  EFI_NVM_EXPRESS_PASS_THRU_COMMAND_PACKET  *Packet
  )
{
  NVME_CQ *CompletionEntry;
  EFI_STATUS Status;

  if (!mIsSupported) {
    return EFI_UNSUPPORTED;
  }

  // Perform passthru call
  Status = mNvmePassThru->PassThru (mNvmePassThru, 0, Packet, NULL);
  if (EFI_ERROR (Status)) {
    return Status;
  }

  // The MdeModulePkg completion struct (EFI_NVM_EXPRESS_COMPLETION) does not expose the Status Code / Status Code
  // Type fields, but MdePkg's NVME_CQ does.  Using the NVME_CQ struct to decode the completion status.
  CompletionEntry = (NVME_CQ *)Packet->NvmeCompletion;
  if (CompletionEntry->Sct != 0 || CompletionEntry->Sc != 0) {
    return EFI_PROTOCOL_ERROR;
  }

  return EFI_SUCCESS;
}

// Return the page alignment when performing writes
//
EFI_STATUS
EFIAPI
WriteGranularity (
  OUT UINT8 *PageCount)
{
  EFI_STATUS  Status;
  UINT8       Fwug;

  if (PageCount == NULL) {
    return EFI_INVALID_PARAMETER;
  }

  EFI_NVM_EXPRESS_COMMAND  Cmd = {
    .Cdw0.Opcode = NVME_ADMIN_IDENTIFY_CMD,
    .Cdw10       = IdentifyControllerCns,
    .Flags       = CDW10_VALID
  };
  EFI_NVM_EXPRESS_COMPLETION                Completion = {0};
  EFI_NVM_EXPRESS_PASS_THRU_COMMAND_PACKET  Packet     = {
    .CommandTimeout = 1ULL * 10000000ULL,
    .QueueType      = NVME_ADMIN_QUEUE,
    .NvmeCmd        = &Cmd,
    .NvmeCompletion = &Completion,
    .TransferBuffer = AllocateAlignedPages (EFI_SIZE_TO_PAGES (SRE_NVME_IDENTIFY_BUFFER_SIZE), mNvmePassThru->Mode->IoAlign),
    .TransferLength = SRE_NVME_IDENTIFY_BUFFER_SIZE
  };
  if (Packet.TransferBuffer == NULL) {
    return EFI_OUT_OF_RESOURCES;
  }

  Status = ExecuteNvmePassThru (&Packet);
  if (!EFI_ERROR (Status)) {
    Fwug = ((UINT8*)Packet.TransferBuffer)[SRE_NVME_ID_CTRL_OFFSET_FWUG];

    if ((Fwug == 0x00) || (Fwug == 0xFF)) {
      *PageCount = SRE_NVME_DEFAULT_GRANULARITY;
    } else {
      *PageCount = Fwug;
    }
  }

  FreeAlignedPages (Packet.TransferBuffer, EFI_SIZE_TO_PAGES (SRE_NVME_IDENTIFY_BUFFER_SIZE));
  return Status;
}

// The constructor locates resources that are needed in the library.
//
EFI_STATUS
EFIAPI
SreStorageLibConstructor (
  IN EFI_HANDLE         ImageHandle,
  IN EFI_SYSTEM_TABLE   *SystemTable
  )
{
  EFI_STATUS Status;
  EFI_HANDLE Handle;
  NVME_CAP Cap;
  EFI_DEVICE_PATH_PROTOCOL  *TargetPath;

  // Assume unsupported then set TRUE if all checks pass
  mIsSupported = FALSE;

  // Convert the SRE Device Path PCD string to a DevicePathProtocol
  TargetPath = ConvertTextToDevicePath ((CONST CHAR16*) PcdGetPtr (PcdSreDevicePathString));
  Status = ConnectStorageDevice (TargetPath, &Handle);
  FreePool (TargetPath);
  if (EFI_ERROR(Status)) {
    DEBUG (((Status == EFI_NOT_FOUND) ? DEBUG_INFO : DEBUG_ERROR, "[SreStorageNvmeLib] ConnectStorageDevice - %r\n", Status));
    return EFI_SUCCESS;
  }

  Status = gBS->HandleProtocol (Handle, &gEfiPciIoProtocolGuid, (VOID **)&mPciIo);
  if (EFI_ERROR (Status)) {
    DEBUG ((DEBUG_ERROR, "[SreStorageNvmeLib] Failed to locate gEfiPciIoProtocolGuid - %r\n", Status));
    return EFI_SUCCESS;
  }

  Status = gBS->HandleProtocol (Handle, &gEfiNvmExpressPassThruProtocolGuid, (VOID **)&mNvmePassThru);
  if (EFI_ERROR (Status)) {
    DEBUG ((DEBUG_ERROR, "[SreStorageNvmeLib] Failed to locate gEfiNvmExpressPassThruProtocolGuid - %r\n", Status));
    return EFI_SUCCESS;
  }

  ZeroMem (&Cap, sizeof (Cap));
  Status = mPciIo->Mem.Read (mPciIo, EfiPciIoWidthUint32, SRE_NVME_BAR0_INDEX, NVME_CAP_OFFSET, sizeof (Cap) / sizeof (UINT32), &Cap);
  if (EFI_ERROR (Status)) {
    DEBUG ((DEBUG_ERROR, "[SreStorageNvmeLib] Failed to read NVME_CAP register - %r\n", Status));
    return EFI_SUCCESS;
  }

  // Report NVME capabilities
  if (Cap.Bps == 0) {
    mIsSupported = FALSE;
    DEBUG ((DEBUG_INFO, "[SreStorageNvmeLib] Boot partition support = FALSE\n"));
    return EFI_SUCCESS;
  }

  mIsSupported = TRUE;
  UINTN PartitionSize = 0;
  UINT8 PageGranularity = 0;
  DEBUG ((DEBUG_INFO, "[SreStorageNvmeLib] Boot partition support = TRUE\n"));
  
  SreStorageSize (&PartitionSize);
  DEBUG ((DEBUG_INFO, "[SreStorageNvmeLib] PartitionSize = 0x%016lx MB\n", PartitionSize / 0x100000));
  WriteGranularity (&PageGranularity);
  DEBUG ((DEBUG_INFO, "[SreStorageNvmeLib] WriteGranularity = %d pages\n", (UINT32)PageGranularity));

  return EFI_SUCCESS;
}

//
// Public API functions to this module (.c file)
//

BOOLEAN
EFIAPI
IsSupported (
  VOID
  )
{
  return mIsSupported;
}

EFI_STATUS
EFIAPI
SreStorageSize (
  OUT UINTN  *Size
  )
{
  EFI_STATUS  Status;
  UINT32      Bpinfo;

  if (Size == NULL) {
    return EFI_INVALID_PARAMETER;
  }
  if (!mIsSupported) {
    return EFI_UNSUPPORTED;
  }

  // Boot partition size comes from the BPINFO register (BPSZ in 128 KiB units).
  Status = mPciIo->Mem.Read (mPciIo, EfiPciIoWidthUint32, SRE_NVME_BAR0_INDEX, NVME_BPINFO_OFFSET, 1, &Bpinfo);
  if (EFI_ERROR (Status)) {
    return Status;
  }
  Bpinfo = Bpinfo & SRE_NVME_BPINFO_BPSZ_MASK;
  if (Bpinfo == 0) {
    return EFI_UNSUPPORTED;
  }

  *Size = (UINTN)Bpinfo * SIZE_128KB;
  return EFI_SUCCESS;
}

EFI_STATUS
EFIAPI
SreStorageRead (
  IN  PARTITION_INDEX PartitionIndex,
  IN  UINT64  Offset,
  OUT VOID    *Buffer,
  IN  UINTN   Length
  )
{
  EFI_STATUS    Status;
  UINT64        LogOffset;
  UINT32        NumD;

  if (Buffer == NULL || (Length % sizeof (UINT32)) != 0 || (Offset % sizeof (UINT32)) != 0) {
    return EFI_INVALID_PARAMETER;
  }
  if (Length == 0) {
    return EFI_SUCCESS;
  }
  if (!mIsSupported) {
    return EFI_UNSUPPORTED;
  }

  // The controller prepends a 16-byte header to the LID 0x15 stream, so the
  // boot-partition byte at offset N is returned at log offset N + 16.
  LogOffset = Offset + SRE_NVME_BP_LOG_HEADER_SIZE;

  // Number of Dwords to read, zero-based, split across CDW10 (NUMDL, lower 16)
  // and CDW11 (NUMDU, upper 16) per the NVMe Get Log Page definition.
  NumD = (UINT32)((Length / sizeof (UINT32)) - 1);

  EFI_NVM_EXPRESS_COMMAND  Cmd = {
    .Cdw0.Opcode = NVME_ADMIN_GET_LOG_PAGE_CMD,
    .Cdw10       = ((NumD & 0xFFFF) << 16) |
                   (((UINT32)PartitionIndex & SRE_NVME_LSP_BPID_MASK) << 8) |
                   LID_BP_INFO,
    .Cdw11       = (NumD >> 16) & 0xFFFF,
    .Cdw12       = (UINT32)LogOffset,
    .Cdw13       = (UINT32)(LogOffset >> 32),
    .Flags       = CDW10_VALID | CDW11_VALID | CDW12_VALID | CDW13_VALID
  };
  EFI_NVM_EXPRESS_COMPLETION                Completion = {0};
  EFI_NVM_EXPRESS_PASS_THRU_COMMAND_PACKET  Packet     = {
    .CommandTimeout = 10ULL * 10000000ULL,
    .QueueType      = NVME_ADMIN_QUEUE,
    .NvmeCmd        = &Cmd,
    .NvmeCompletion = &Completion,
    .TransferBuffer = AllocateAlignedPages(EFI_SIZE_TO_PAGES(Length), mNvmePassThru->Mode->IoAlign),
    .TransferLength = (UINT32)Length
  };

  if (Packet.TransferBuffer == NULL) {
    return EFI_OUT_OF_RESOURCES;
  }

  Status = ExecuteNvmePassThru (&Packet);
  if (!EFI_ERROR (Status)) {
    CopyMem(Buffer, Packet.TransferBuffer, Length);
  }

  FreeAlignedPages(Packet.TransferBuffer, EFI_SIZE_TO_PAGES (Length));
  return Status;
}

STATIC
EFI_STATUS
EFIAPI
NvmeSetLockState (
  IN  PARTITION_INDEX PartitionIndex,
  IN  NVME_LOCK_STATE LockState
)
{
  EFI_STATUS  Status;
  UINT32      ConfigDw0;
  UINT32      Shift;

  // Read the write protect configuration
  EFI_NVM_EXPRESS_COMMAND  CmdR = {
    .Cdw0.Opcode = NVME_ADMIN_GET_FEATURES_CMD,
    .Cdw10       = SRE_NVME_FID_BP_WRITE_PROTECTION_CFG,
    .Flags       = CDW10_VALID
  };
  EFI_NVM_EXPRESS_COMPLETION                CompletionR = {0};
  EFI_NVM_EXPRESS_PASS_THRU_COMMAND_PACKET  PacketR     = {
    .CommandTimeout = 2ULL * 10000000ULL,
    .QueueType      = NVME_ADMIN_QUEUE,
    .NvmeCmd        = &CmdR,
    .NvmeCompletion = &CompletionR
  };

  Status = ExecuteNvmePassThru (&PacketR);
  if (EFI_ERROR (Status)) {
    return Status;
  }
  ConfigDw0 = CompletionR.DW0;

  // Modify bits
  Shift      = (PartitionIndex == SrePartition_A) ? SRE_BPWPS_BP0_SHIFT : SRE_BPWPS_BP1_SHIFT;
  ConfigDw0 &= ~((UINT32)SRE_BPWPS_FIELD_MASK << Shift);
  ConfigDw0 |= ((UINT32)LockState & SRE_BPWPS_FIELD_MASK) << Shift;

  // Write the write protect configuration
  EFI_NVM_EXPRESS_COMMAND  CmdW = {
    .Cdw0.Opcode = NVME_ADMIN_SET_FEATURES_CMD,
    .Cdw10       = SRE_NVME_FID_BP_WRITE_PROTECTION_CFG,
    .Cdw11       = ConfigDw0,
    .Flags       = CDW10_VALID | CDW11_VALID
  };
  EFI_NVM_EXPRESS_COMPLETION                CompletionW = {0};
  EFI_NVM_EXPRESS_PASS_THRU_COMMAND_PACKET  PacketW     = {
    .CommandTimeout = 2ULL * 10000000ULL,
    .QueueType      = NVME_ADMIN_QUEUE,
    .NvmeCmd        = &CmdW,
    .NvmeCompletion = &CompletionW
  };

  return ExecuteNvmePassThru (&PacketW);
}

EFI_STATUS
EFIAPI
SreStorageLock (
  VOID
  )
{
  EFI_STATUS StatusA = NvmeSetLockState (SrePartition_A, WriteLockedUntilPowerCycle);
  EFI_STATUS StatusB = NvmeSetLockState (SrePartition_B, WriteLockedUntilPowerCycle);

  return (EFI_ERROR (StatusA)) ? StatusA : StatusB;
}

EFI_STATUS
EFIAPI
SreStorageWriteOpen (
  IN  PARTITION_INDEX PartitionIndex,
  OUT UINTN   *BlockCount,
  OUT UINTN   *BlockSize,
  OUT VOID    **BlockBuffer
  )
{
  EFI_STATUS  Status;
  UINTN       PartitionSize;
  UINT8       PageGranularity;

  if (BlockCount == NULL || BlockSize == NULL || BlockBuffer == NULL) {
    return EFI_INVALID_PARAMETER;
  }

  // Total writable size of the partition.
  Status = SreStorageSize (&PartitionSize);
  if (EFI_ERROR (Status)) {
    return Status;
  }

  // The device's preferred write chunk (granularity) becomes the block size.
  Status = WriteGranularity (&PageGranularity);
  if (EFI_ERROR (Status)) {
    return Status;
  }

  // Unlock the partition so blocks can be written.
  Status = NvmeSetLockState (PartitionIndex, WriteUnlocked);
  if (EFI_ERROR (Status)) {
    return Status;
  }

  // Record state and return
  mSreWriteSession.PartitionIndex = PartitionIndex;
  mSreWriteSession.BlockSize      = (UINTN)PageGranularity * EFI_PAGE_SIZE;
  mSreWriteSession.BlockCount     = (PartitionSize + mSreWriteSession.BlockSize - 1) / mSreWriteSession.BlockSize;
  mSreWriteSession.BlocksWritten  = 0;
  
  *BlockSize  = mSreWriteSession.BlockSize;
  *BlockCount = mSreWriteSession.BlockCount;
  *BlockBuffer = AllocateAlignedPages(EFI_SIZE_TO_PAGES(mSreWriteSession.BlockSize), mNvmePassThru->Mode->IoAlign);
  
  return EFI_SUCCESS;
}

EFI_STATUS
EFIAPI
SreStorageWriteBlock (
  IN  VOID  *BlockBuffer
  )
{
  EFI_STATUS  Status;
  if (mSreWriteSession.BlockSize == 0) {
    return EFI_NOT_READY;
  }
  if (BlockBuffer == NULL) {
    return EFI_INVALID_PARAMETER;
  }

  if (mSreWriteSession.BlocksWritten >= mSreWriteSession.BlockCount) {
    return EFI_OUT_OF_RESOURCES;
  }

  EFI_NVM_EXPRESS_COMMAND  Cmd = {
    .Cdw0.Opcode = NVME_ADMIN_FW_IAMGE_DOWNLOAD_CMD,
    .Cdw10       = (UINT32)((mSreWriteSession.BlockSize / sizeof (UINT32)) - 1),
    .Cdw11       = (UINT32)((mSreWriteSession.BlocksWritten * mSreWriteSession.BlockSize) / sizeof (UINT32)),
    .Cdw12       = SRE_FW_IMG_DOWNLOAD_CDW12_BP_DATA,
    .Flags       = CDW10_VALID | CDW11_VALID | CDW12_VALID
  };
  EFI_NVM_EXPRESS_COMPLETION                Completion = {0};
  EFI_NVM_EXPRESS_PASS_THRU_COMMAND_PACKET  Packet     = {
    .CommandTimeout = 5ULL * 10000000ULL,
    .QueueType      = NVME_ADMIN_QUEUE,
    .NvmeCmd        = &Cmd,
    .NvmeCompletion = &Completion,
    .TransferBuffer = BlockBuffer,
    .TransferLength = (UINT32)mSreWriteSession.BlockSize
  };

  Status = ExecuteNvmePassThru (&Packet);
  if (!EFI_ERROR (Status)) {
    mSreWriteSession.BlocksWritten++;
  }

  return Status;
}

EFI_STATUS
EFIAPI
SreStorageWriteClose (
  VOID
  )
{
  EFI_STATUS  Status = EFI_SUCCESS;

  if (mSreWriteSession.BlockCount != 0) {

    if (mSreWriteSession.BlocksWritten != mSreWriteSession.BlockCount) {
      return EFI_NOT_READY;
    }

    EFI_NVM_EXPRESS_COMMAND  Cmd = {
      .Cdw0.Opcode = NVME_ADMIN_FW_COMMIT_CMD,
      .Cdw10       = ((UINT32)mSreWriteSession.BlockCount << SRE_NVME_FW_COMMIT_BPID_SHIFT) |
                    ((UINT32)SRE_NVME_FW_COMMIT_ACTION_DOWNLOAD_BP << SRE_NVME_FW_COMMIT_ACTION_SHIFT),
      .Flags       = CDW10_VALID
    };
    EFI_NVM_EXPRESS_COMPLETION                Completion = {0};
    EFI_NVM_EXPRESS_PASS_THRU_COMMAND_PACKET  Packet     = {
      .CommandTimeout = 30ULL * 10000000ULL,
      .QueueType      = NVME_ADMIN_QUEUE,
      .NvmeCmd        = &Cmd,
      .NvmeCompletion = &Completion
    };

    Status = ExecuteNvmePassThru (&Packet);
    if (!EFI_ERROR (Status)) {
      mSreWriteSession.BlockSize      = 0;
      mSreWriteSession.BlockCount     = 0;
      mSreWriteSession.BlocksWritten  = 0;
    }
  }

  return Status;
}
