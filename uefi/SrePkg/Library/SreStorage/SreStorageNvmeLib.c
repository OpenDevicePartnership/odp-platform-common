//
// Secure Recovery Environment (SRE) NVMe support for the FMP Device Library.
//
// Copyright (c) Microsoft Corporation. All rights reserved.
// License: MIT
//

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

#include <Library/SreFmpDeviceLib.h>
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

//
// Firmware Image Download CDW12 route: 1 = "Data for boot partition download"
//
#define SRE_NVME_FW_DOWNLOAD_BP_DATA          0x1

//
// Set Features FID=0x85 (Boot Partition Write Protection Config) CDW11 field
// encodings.
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
#define SRE_NVME_ID_CTRL_OFFSET_BPCAP         102   // 1 byte: Boot Partition Capabilities
#define SRE_NVME_ID_CTRL_OFFSET_FWUG          319   // 1 byte: Firmware Update Granularity
#define SRE_NVME_ID_CTRL_OFFSET_LPA           261   // 1 byte: Log Page Attributes
#define SRE_NVME_BPCAP_SFBPWPS                BIT2  // Set Features Boot Partition Write Protection Support
#define SRE_NVME_LPA_LPEDS                    0x04  // LPA bit 2: Log Page Extended Data Support

//
// Default used when granularity reported by FWUG is 0 (no info) or 0xFF (no restriction)
//
#define SRE_NVME_FWUG_DEFAULT_GRANULARITY     1
#define SRE_NVME_FWUG_NO_INFO                 0x00
#define SRE_NVME_FWUG_NO_RESTRICTION          0xFF
#define SRE_NVME_FWUG_RESOLUTION              SIZE_4KB

//
// Boot Partition geometry via the controller's PCI BAR0 MMIO registers
// (NVMe Base Spec §3.1, BPINFO). Readable whenever the controller is powered.
//
#define SRE_NVME_BAR0_INDEX                   0
#define SRE_NVME_BPINFO_BPSZ_MASK             0x7FFF // bits 14:0, BP size in 128 KiB units

//
// Boot Partition read via Get Log Page LID 0x15 field encodings (NVMe Base
// Spec 2.1 §8.1.3 / §5.1.12). Used by SreStorageRead.
//
#define SRE_NVME_BP_LOG_HEADER_SIZE           16    // 16-byte header prepended to the LID 0x15 stream
#define SRE_NVME_LSP_BPID_MASK                0x7F  // Get Log Page CDW10 LSP field carries the BPID

//
// Boot Partition Write Protection State (BPxWPS): the 3-bit NVMe field values
// (NVMe Base Spec 2.1 §5.1.25.1.32, Boot Partition Write Protection Config):
//   000b  Change in state not requested
//   001b  Write Unlocked
//   010b  Write Locked
//   011b  Write Locked Until Power Cycle
//   100b  Write Protection controlled by RPMB
//
#define SRE_BPWPS_CHANGE_NOT_REQUESTED            0x0  // 000b Change in state not requested
#define SRE_BPWPS_WRITE_UNLOCKED                  0x1  // 001b Write Unlocked
#define SRE_BPWPS_WRITE_LOCKED                    0x2  // 010b Write Locked
#define SRE_BPWPS_WRITE_LOCKED_UNTIL_POWER_CYCLE  0x3  // 011b Write Locked Until Power Cycle
#define SRE_BPWPS_CONTROLLED_BY_RPMB              0x4  // 100b Write Protection controlled by RPMB

//
// This enum is for writes to the BPWPS register.  RPMB is a read only bit and change not requested is a lack of any
// bit, so neither are part of the enum.
//
typedef enum NVME_LOCK_STATE {
  WriteUnlocked              = SRE_BPWPS_WRITE_UNLOCKED,
  WriteLocked                = SRE_BPWPS_WRITE_LOCKED,
  WriteLockedUntilPowerCycle = SRE_BPWPS_WRITE_LOCKED_UNTIL_POWER_CYCLE
} NVME_LOCK_STATE;


//
// Private globals for context across function calls
//

BOOLEAN                             mIsWriteOpen = FALSE;
PARTITION_INDEX                     mPartitionIndex = SrePartition_A;
UINTN                               mBlockSize = 0;
UINTN                               mBlockCount = 0;
UINTN                               mBlockIndex = 0;
UINTN                               mBlockAlignment = 0;
EFI_NVM_EXPRESS_PASS_THRU_PROTOCOL  *mNvmePassThru = NULL;
EFI_PCI_IO_PROTOCOL                 *mPciIo = NULL;
// Constructor sets supported once all other checks and init pass
BOOLEAN                             mIsSupported = FALSE;


//
// Private functions
//

//
// A targeted connect to the device specified by the PcdSreDevicePathString
//
// [OUT] Handle - A handle to the device that was just connected
//
EFI_STATUS
EFIAPI
ConnectStorageDevice(
  OUT EFI_HANDLE  *Handle
)
{
  EFI_DEVICE_PATH_PROTOCOL *TargetPath;
  EFI_STATUS                Status;
  EFI_DEVICE_PATH_PROTOCOL  *RemainingPath;
  EFI_HANDLE                PreviousHandle;

  if (Handle == NULL) {
    return EFI_INVALID_PARAMETER;
  }

  TargetPath = ConvertTextToDevicePath ((CONST CHAR16*) PcdGetPtr (PcdSreDevicePathString));
  if (TargetPath == NULL) {
    return EFI_INVALID_PARAMETER;
  }
  if (IsDevicePathEnd (TargetPath)) {
    FreePool (TargetPath);
    return EFI_INVALID_PARAMETER;
  }

  // Targeted connect of just this device path (not a connect-all).
  PreviousHandle = NULL;
  *Handle = NULL;
  do {

    // LocateDevicePath returns a handle to the target device or its closest parent if not found
    RemainingPath = TargetPath;
    Status = gBS->LocateDevicePath (&gEfiDevicePathProtocolGuid, &RemainingPath, Handle);
    if (!EFI_ERROR(Status) && PreviousHandle == *Handle) {
      Status = EFI_NOT_FOUND;
    }
    if (EFI_ERROR(Status)) {
      break;
    }
    PreviousHandle = *Handle;

    // Perform a connect of this device to enumerate its children
    Status = gBS->ConnectController (*Handle, NULL, NULL, FALSE);
    if (EFI_ERROR(Status)) {
      break;
    }

  // If RemainingPath is the DeviceEndPath node, we just connected our target device and can exit the loop
  } while (!IsDevicePathEnd (RemainingPath));

  FreePool (TargetPath);
  return Status;
}

//
// Common private function to initiate a PassThru call and verify the completion status
//
// [IN] Packet - A pointer to the NVMe pass-thru command packet to execute
//
EFI_STATUS
EFIAPI
ExecuteNvmePassThru (
  IN  EFI_NVM_EXPRESS_PASS_THRU_COMMAND_PACKET  *Packet
  )
{
  NVME_CQ *CompletionEntry;
  EFI_STATUS Status;
  UINT32 Opcode;

  if (Packet == NULL) {
    return EFI_INVALID_PARAMETER;
  }
  if (mNvmePassThru == NULL) {
    return EFI_NOT_READY;
  }

  Opcode = Packet->NvmeCmd->Cdw0.Opcode;

  // Perform passthru call
  Status = mNvmePassThru->PassThru (mNvmePassThru, 0, Packet, NULL);
  if (EFI_ERROR (Status)) {
    DEBUG ((DEBUG_ERROR, "[SreStorageNvmeLib] PassThru transport error - %r (Opcode=0x%02x)\n", Status, Opcode));
    return Status;
  }

  // The MdeModulePkg completion struct (EFI_NVM_EXPRESS_COMPLETION) does not expose the Status Code / Status Code
  // type fields, but MdePkg's NVME_CQ does.  Using the NVME_CQ struct to decode the completion status.
  CompletionEntry = (NVME_CQ *)Packet->NvmeCompletion;
  if (CompletionEntry->Sct != 0 || CompletionEntry->Sc != 0) {
    DEBUG ((DEBUG_ERROR, "[SreStorageNvmeLib] NVMe command rejected: Opcode=0x%02x SCT=0x%x SC=0x%x\n", Opcode, CompletionEntry->Sct, CompletionEntry->Sc));
    return EFI_PROTOCOL_ERROR;
  }

  return EFI_SUCCESS;
}

//
// Read the fields the library needs from Identify Controller (CNS=01h) in a single command
//
// [out] FirmwareUpdateGranularity - Write granularity as reported by the identify command
// [out] SetLockStateSupported     - TRUE if Set Features may configure Boot Partition write protection
// [out] LpedsSupported            - TRUE if the controller supports the Get Log Page extended Log Page Offset
//                                   (CDW12/CDW13) and 16-bit number of Dwords fields (LPA bit 2, LPEDS). The boot-
//                                   partition read path depends on these fields.
//
EFI_STATUS
EFIAPI
IdentifyController (
  OUT UINT8   *FirmwareUpdateGranularity,
  OUT BOOLEAN *SetLockStateSupported,
  OUT BOOLEAN *LpedsSupported)
{
  EFI_STATUS  Status;
  UINT8       *IdCtrl;

  if (FirmwareUpdateGranularity == NULL || SetLockStateSupported == NULL || LpedsSupported == NULL) {
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
    .TransferBuffer = AllocateAlignedPages (EFI_SIZE_TO_PAGES (SRE_NVME_IDENTIFY_BUFFER_SIZE), mBlockAlignment),
    .TransferLength = SRE_NVME_IDENTIFY_BUFFER_SIZE
  };
  if (Packet.TransferBuffer == NULL) {
    return EFI_OUT_OF_RESOURCES;
  }

  Status = ExecuteNvmePassThru (&Packet);
  if (!EFI_ERROR (Status)) {
    IdCtrl                     = (UINT8 *)Packet.TransferBuffer;
    *FirmwareUpdateGranularity = IdCtrl[SRE_NVME_ID_CTRL_OFFSET_FWUG];
    *SetLockStateSupported     = (IdCtrl[SRE_NVME_ID_CTRL_OFFSET_BPCAP] & SRE_NVME_BPCAP_SFBPWPS) != 0;
    *LpedsSupported            = (IdCtrl[SRE_NVME_ID_CTRL_OFFSET_LPA] & SRE_NVME_LPA_LPEDS) != 0;
  }

  FreeAlignedPages (Packet.TransferBuffer, EFI_SIZE_TO_PAGES (SRE_NVME_IDENTIFY_BUFFER_SIZE));
  return Status;
}

//
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
  UINT32 BpInfo;
  UINT8 FirmwareUpdateGranularity;
  BOOLEAN SetLockStateSupported;
  BOOLEAN LpedsSupported;
  UINTN BpSize;

  // Execute a connect command to the storage device
  Status = ConnectStorageDevice (&Handle);
  if (EFI_ERROR(Status)) {
    DEBUG (((Status == EFI_NOT_FOUND) ? DEBUG_INFO : DEBUG_ERROR, "[SreStorageNvmeLib] ConnectStorageDevice returned (%r), SRE not supported\n", Status));
    return EFI_SUCCESS;
  }

  // Use the handle to retrieve a linked PCI IO and NVMe PassThru protocol
  Status = gBS->HandleProtocol (Handle, &gEfiPciIoProtocolGuid, (VOID **)&mPciIo);
  if (EFI_ERROR (Status)) {
    DEBUG ((DEBUG_ERROR, "[SreStorageNvmeLib] Failed to locate gEfiPciIoProtocolGuid (%r), SRE not supported\n", Status));
    return EFI_SUCCESS;
  }
  Status = gBS->HandleProtocol (Handle, &gEfiNvmExpressPassThruProtocolGuid, (VOID **)&mNvmePassThru);
  if (EFI_ERROR (Status)) {
    DEBUG ((DEBUG_ERROR, "[SreStorageNvmeLib] Failed to locate gEfiNvmExpressPassThruProtocolGuid (%r), SRE not supported\n", Status));
    return EFI_SUCCESS;
  }
  mBlockAlignment = (mNvmePassThru->Mode->IoAlign == 0) ? EFI_PAGE_SIZE : mNvmePassThru->Mode->IoAlign;

  // Does the NVME support the boot partition?
  ZeroMem (&Cap, sizeof (Cap));
  Status = mPciIo->Mem.Read (mPciIo, EfiPciIoWidthUint32, SRE_NVME_BAR0_INDEX, NVME_CAP_OFFSET, sizeof (Cap) / sizeof (UINT32), &Cap);
  if (EFI_ERROR (Status)) {
    DEBUG ((DEBUG_ERROR, "[SreStorageNvmeLib] Failed to read NVME_CAP register (%r), SRE not supported\n", Status));
    return EFI_SUCCESS;
  }
  if (Cap.Bps == 0) {
    DEBUG((DEBUG_INFO, "[SreStorageNvmeLib] NVME_CAP.BPS=0, boot partition not supported\n"));
    return EFI_SUCCESS;
  }

  // Set global block size
  Status = IdentifyController (&FirmwareUpdateGranularity, &SetLockStateSupported, &LpedsSupported);
  if (EFI_ERROR (Status)) {
    DEBUG ((DEBUG_ERROR, "[SreStorageNvmeLib] Failed to identify controller (%r), SRE not supported\n", Status));
    return EFI_SUCCESS;
  }
  if (!LpedsSupported) {
    DEBUG ((
      DEBUG_ERROR,
      "[SreStorageNvmeLib] Controller does not support Log Page Extended Data "
      "(LPA.LPEDS) to allow boot partition reads\n"
      ));
    return EFI_SUCCESS;
  }
  if (!SetLockStateSupported) {
    DEBUG ((
      DEBUG_ERROR,
      "[SreStorageNvmeLib] Controller does not support Boot Partition Write "
      "Protection Support (BPCAP.SFBPWPS)\n"
      ));
    return EFI_SUCCESS;
  }
  mBlockSize =
    ((FirmwareUpdateGranularity == SRE_NVME_FWUG_NO_INFO) ||
     (FirmwareUpdateGranularity == SRE_NVME_FWUG_NO_RESTRICTION))
      ? SRE_NVME_FWUG_RESOLUTION * SRE_NVME_FWUG_DEFAULT_GRANULARITY
      : SRE_NVME_FWUG_RESOLUTION * FirmwareUpdateGranularity;

  // Set global block count
  Status = mPciIo->Mem.Read (mPciIo, EfiPciIoWidthUint32, SRE_NVME_BAR0_INDEX, NVME_BPINFO_OFFSET, 1, &BpInfo);
  if (EFI_ERROR (Status)) {
    DEBUG ((DEBUG_ERROR, "[SreStorageNvmeLib] Failed to read NVME_BPINFO register - %r\n", Status));
    return EFI_SUCCESS;
  }
  BpInfo = BpInfo & SRE_NVME_BPINFO_BPSZ_MASK;
  if (BpInfo == 0) {
    DEBUG ((DEBUG_ERROR, "[SreStorageNvmeLib] Undefined boot partition info register value (0x00), SRE not supported\n"));
    return EFI_SUCCESS;
  }
  BpSize = (UINTN)BpInfo * SIZE_128KB;
  if (BpSize < mBlockSize) {
    DEBUG ((
      DEBUG_ERROR,
      "[SreStorageNvmeLib] Block size reported by NVMe exceeds boot partition "
      "size, SRE not supported\n"
      ));
    return EFI_SUCCESS;
  }
  mBlockCount = BpSize / mBlockSize;

  // Supported
  mIsSupported = TRUE;
  DEBUG ((DEBUG_INFO, "[SreStorageNvmeLib] Boot partition support = TRUE\n"));
  return EFI_SUCCESS;
}

//
// Set the NVMe boot partition write-protection state.
//
// [in] PartitionIndex  Target boot partition (A or B).
// [in] LockState       Desired lock state.
//
EFI_STATUS
EFIAPI
NvmeSetLockState (
  IN  PARTITION_INDEX PartitionIndex,
  IN  NVME_LOCK_STATE LockState
)
{
  UINT32  Shift;
  UINT32  Config;

  if (PartitionIndex > SrePartition_B) {
    return EFI_INVALID_PARAMETER;
  }

  // A zero BPxWPS field means "change in state not requested" for that
  // partition, so only populate the field for the target partition.
  Shift  = (PartitionIndex == SrePartition_A) ? SRE_BPWPS_BP0_SHIFT : SRE_BPWPS_BP1_SHIFT;
  Config = ((UINT32)LockState & SRE_BPWPS_FIELD_MASK) << Shift;

  EFI_NVM_EXPRESS_COMMAND  Cmd = {
    .Cdw0.Opcode = NVME_ADMIN_SET_FEATURES_CMD,
    .Cdw10       = SRE_NVME_FID_BP_WRITE_PROTECTION_CFG,
    .Cdw11       = Config,
    .Flags       = CDW10_VALID | CDW11_VALID
  };
  EFI_NVM_EXPRESS_COMPLETION                Completion = {0};
  EFI_NVM_EXPRESS_PASS_THRU_COMMAND_PACKET  Packet     = {
    .CommandTimeout = 2ULL * 10000000ULL,
    .QueueType      = NVME_ADMIN_QUEUE,
    .NvmeCmd        = &Cmd,
    .NvmeCompletion = &Completion
  };

  return ExecuteNvmePassThru (&Packet);
}


//
// Public API functions to this library
//

// Return the block geometry of the SRE boot partition.
EFI_STATUS
EFIAPI
SreStorageInfo (
  OUT UINTN  *BlockCount,
  OUT UINTN  *BlockSize,
  OUT UINTN  *BlockBufferAlignment
  )
{
  if (BlockCount == NULL || BlockSize == NULL || BlockBufferAlignment == NULL) {
    return EFI_INVALID_PARAMETER;
  }
  if (!mIsSupported) {
    return EFI_UNSUPPORTED;
  }

  *BlockCount = mBlockCount;
  *BlockSize  = mBlockSize;
  *BlockBufferAlignment = mBlockAlignment;
  return EFI_SUCCESS;
}

// Read a single block from an NVMe boot partition via Get Log Page.
EFI_STATUS
EFIAPI
SreStorageRead (
  IN  PARTITION_INDEX PartitionIndex,
  IN  UINTN           BlockIndex,
  OUT VOID            *BlockBuffer
  )
{
  UINT64        Offset;
  UINT64        LogOffset;
  UINT32        NumD;

  if (PartitionIndex > SrePartition_B || BlockBuffer == NULL) {
    return EFI_INVALID_PARAMETER;
  }
  if (!mIsSupported) {
    return EFI_UNSUPPORTED;
  }
  if (BlockIndex >= mBlockCount) {
    return EFI_INVALID_PARAMETER;
  }
  if (mIsWriteOpen) {
    return EFI_ABORTED;
  }
  if (((UINTN)BlockBuffer % mBlockAlignment) != 0) {
    return EFI_INVALID_PARAMETER;
  }

  Offset = (UINT64)BlockIndex * (UINT64)mBlockSize;

  // The controller prepends a 16-byte header to the LID 0x15 stream, so the
  // boot-partition byte at offset N is returned at log offset N + 16.
  LogOffset = Offset + SRE_NVME_BP_LOG_HEADER_SIZE;

  // Number of Dwords to read, zero-based, split across CDW10 (NUMDL, lower 16)
  // and CDW11 (NUMDU, upper 16) per the NVMe Get Log Page definition.
  NumD = (UINT32)((mBlockSize / sizeof (UINT32)) - 1);

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
    .TransferBuffer = BlockBuffer,
    .TransferLength = (UINT32)mBlockSize
  };

  return ExecuteNvmePassThru (&Packet);
}

// Open a write session to the target boot partition.
EFI_STATUS
EFIAPI
SreStorageWriteOpen (
  IN  PARTITION_INDEX PartitionIndex
  )
{
  EFI_STATUS  Status;

  if (PartitionIndex > SrePartition_B) {
    return EFI_INVALID_PARAMETER;
  }
  if (!mIsSupported) {
    return EFI_UNSUPPORTED;
  }
  if (mIsWriteOpen) {
    return EFI_ABORTED;
  }

  // Unlocking the partition exposes the temporary write location for the download.
  Status = NvmeSetLockState (PartitionIndex, WriteUnlocked);
  if (EFI_ERROR (Status)) {
    return Status;
  }

  // Start the session at block 0 of the target partition.
  mPartitionIndex = PartitionIndex;
  mBlockIndex     = 0;
  mIsWriteOpen    = TRUE;
  return EFI_SUCCESS;
}

// Write the next block via Firmware Image Download.
EFI_STATUS
EFIAPI
SreStorageWriteBlock (
  IN  VOID  *BlockBuffer
  )
{
  EFI_STATUS  Status;

  if (BlockBuffer == NULL) {
    return EFI_INVALID_PARAMETER;
  }
  if (!mIsSupported) {
    return EFI_UNSUPPORTED;
  }
  if (!mIsWriteOpen) {
    return EFI_NOT_READY;
  }
  if (mBlockIndex >= mBlockCount) {
    return EFI_END_OF_MEDIA;
  }
  if (((UINTN)BlockBuffer % mBlockAlignment) != 0) {
    return EFI_INVALID_PARAMETER;
  }

  // Firmware Image Download uses the Data Pointer, CDW10 (NUMD), CDW11 (OFST)
  // and CDW12
  EFI_NVM_EXPRESS_COMMAND  Cmd = {
    .Cdw0.Opcode = NVME_ADMIN_FW_IAMGE_DOWNLOAD_CMD,
    .Cdw10       = (UINT32)((mBlockSize / sizeof (UINT32)) - 1),
    .Cdw11       = (UINT32)((mBlockIndex * mBlockSize) / sizeof (UINT32)),
    .Cdw12       = SRE_NVME_FW_DOWNLOAD_BP_DATA,
    .Flags       = CDW10_VALID | CDW11_VALID | CDW12_VALID
  };
  EFI_NVM_EXPRESS_COMPLETION                Completion = {0};
  EFI_NVM_EXPRESS_PASS_THRU_COMMAND_PACKET  Packet     = {
    .CommandTimeout = 5ULL * 10000000ULL,
    .QueueType      = NVME_ADMIN_QUEUE,
    .NvmeCmd        = &Cmd,
    .NvmeCompletion = &Completion,
    .TransferBuffer = BlockBuffer,
    .TransferLength = (UINT32)mBlockSize
  };

  Status = ExecuteNvmePassThru (&Packet);
  if (EFI_ERROR (Status)) {
    DEBUG ((
      DEBUG_ERROR,
      "[SreStorageNvmeLib] FW Image Download failed at block %d of %d (BlockSize=0x%x Align=0x%x Buffer=%p NUMD=0x%x OFST=0x%x) - %r\n",
      (UINT32)mBlockIndex, (UINT32)mBlockCount, (UINT32)mBlockSize, (UINT32)mBlockAlignment,
      BlockBuffer, Cmd.Cdw10, Cmd.Cdw11, Status));
  } else {
    mBlockIndex++;
  }

  return Status;
}

// Commit the write session, activating the downloaded image in the boot partition.
EFI_STATUS
EFIAPI
SreStorageWriteClose (
  VOID
  )
{
  EFI_STATUS  Status;

  if (!mIsSupported) {
    return EFI_UNSUPPORTED;
  }
  if (!mIsWriteOpen) {
    return EFI_NOT_READY;
  }

  // The whole partition must have been written before it can be committed.
  if (mBlockIndex != mBlockCount) {
    return EFI_ABORTED;
  }

  EFI_NVM_EXPRESS_COMMAND  Cmd = {
    .Cdw0.Opcode = NVME_ADMIN_FW_COMMIT_CMD,
    .Cdw10       = ((UINT32)mPartitionIndex << SRE_NVME_FW_COMMIT_BPID_SHIFT) |
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
    mIsWriteOpen = FALSE;
    mBlockIndex  = 0;
  }

  return Status;
}

// Abort an open write session without committing. Clears the session state
// unconditionally (so a failed apply cannot wedge later operations) and
// restores write protection on the partition; returns the re-lock status.
EFI_STATUS
EFIAPI
SreStorageWriteAbort (
  VOID
  )
{
  EFI_STATUS  Status;

  if (!mIsSupported) {
    return EFI_UNSUPPORTED;
  }
  if (!mIsWriteOpen) {
    return EFI_NOT_READY;
  }

  Status = NvmeSetLockState (mPartitionIndex, WriteLocked);

  mIsWriteOpen = FALSE;
  mBlockIndex  = 0;

  return Status;
}

// SRE Storage lock mechanism that requires a power reset to unlock
EFI_STATUS
EFIAPI
SreStorageLock (
  IN  PARTITION_INDEX PartitionIndex
  )
{
  if (PartitionIndex > SrePartition_B) {
    return EFI_INVALID_PARAMETER;
  }
  if (!mIsSupported) {
    return EFI_UNSUPPORTED;
  }
  if (mIsWriteOpen) {
    return EFI_ABORTED;
  }

  return NvmeSetLockState (PartitionIndex, WriteLockedUntilPowerCycle);
}
