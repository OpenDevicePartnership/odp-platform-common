//
// Secure Recovery Environment (SRE) Firmware Management Protocol (FMP) Device Library
// 
// Copyright (c) Microsoft Corporation. All rights reserved.
// SPDX-License-Identifier: BSD-2-Clause-Patent
// 
#include <PiDxe.h>
#include <Guid/SystemResourceTable.h>
#include <Library/BaseMemoryLib.h>
#include <Library/DebugLib.h>
#include <Library/FmpDeviceLib.h>
#include <Library/MemoryAllocationLib.h>
#include <Library/PrintLib.h>
#include <Library/UefiRuntimeServicesTableLib.h>

#include <Library/SreFmpDeviceLib.h>
#include <Library/SreStorage.h>

// Global to keep Register FmpInstaller and FmpUninstaller having the same return values
EFI_STATUS  mFmpRegisterStatus = EFI_SUCCESS;

// Using a macro to guarantee we don't use an ASSERT or other method to halt execution when validating input
// parameters.  This is an FMP driver that should not halt boot execution.
#define INPUT_PARAM_CHECK(param_checks)                                       \
  if (param_checks) {                                                         \
    DEBUG ((DEBUG_ERROR, "[SRE %a] - invalid parameter\n", __FUNCTION__));    \
    return EFI_INVALID_PARAMETER;                                             \
  }


//
// Private FMP library functions
//

/**
  Handler to calculate and display the progress across both partitions

  @param[in]  PartitionIndex  The index of the partition being written to.
  @param[in]  BlockIndex      The index of the block being written.
  @param[in]  BlockCount      The total number of blocks to write.
  @param[in]  Progress        Optional EFI progress callback.
**/
VOID
EFIAPI
SreProgress(
  IN  PARTITION_INDEX  PartitionIndex,
  IN  UINTN   BlockIndex,
  IN  UINTN   BlockCount,
  IN  EFI_FIRMWARE_MANAGEMENT_UPDATE_IMAGE_PROGRESS  Progress OPTIONAL)
{
  STATIC UINTN LastPercent = (UINTN)-1;
  UINTN Percent;

  if (Progress != NULL) {
    Percent = ((BlockIndex * 50) / BlockCount) + (PartitionIndex * 50);

    if (Percent != LastPercent) {
      Progress (Percent);
      LastPercent = Percent;
    }
  }
}

/**
  Stream an in-memory image to the SRE storage partition.

  @param[in] PartitionIndex   Target storage partition index.
  @param[in] Image            In-memory image buffer to stream.
  @param[in] ImageSize        Size of the image buffer in bytes.
  @param[in] CapsuleFwVersion Firmware version recorded in the image trailer.
  @param[in] Progress         Optional progress callback.
**/
EFI_STATUS
EFIAPI
ApplyWimToSreStorage(
  IN  PARTITION_INDEX PartitionIndex,
  IN  CONST VOID *Image,
  IN  UINTN ImageSize,
  IN  UINT32 CapsuleFwVersion,
  IN  EFI_FIRMWARE_MANAGEMENT_UPDATE_IMAGE_PROGRESS Progress OPTIONAL)
{
  EFI_STATUS         Status;
  UINTN              BlockCount;
  UINTN              BlockSize;
  UINTN              BlockBufferAlignment;
  UINTN              BlockIndex;
  VOID               *Chunk = NULL;
  CONST UINT8        *ImageBytes;
  UINTN              RemainingToWrite;

  // Query the storage geometry so we can size, align, and stream the block buffer ourselves.
  Status = SreStorageInfo (&BlockCount, &BlockSize, &BlockBufferAlignment);
  if (EFI_ERROR (Status)) {
    DEBUG ((DEBUG_ERROR, "[SRE %a] failed to read storage geometry - %r\n", __FUNCTION__, Status));
    goto Done;
  }

  // Allocate a single reusable block buffer, aligned to the storage requirement,
  // to stream the image into.
  Chunk = AllocateAlignedPages (EFI_SIZE_TO_PAGES (BlockSize), BlockBufferAlignment);
  if (Chunk == NULL) {
    Status = EFI_OUT_OF_RESOURCES;
    DEBUG ((DEBUG_ERROR, "[SRE %a] failed to allocate block buffer\n", __FUNCTION__));
    goto Done;
  }

  Status = SreStorageWriteOpen (PartitionIndex);
  if (EFI_ERROR (Status)) {
    DEBUG ((DEBUG_ERROR, "[SRE %a] failed to open write session - %r\n", __FUNCTION__, Status));
    goto Done;
  }

  ImageBytes       = (CONST UINT8 *)Image;
  RemainingToWrite = ImageSize;

  for (BlockIndex = 0; BlockIndex < BlockCount; BlockIndex++) {
    SreProgress (PartitionIndex, BlockIndex, BlockCount, Progress);

    // Copy the next chunk of the in-memory image into the block buffer
    UINTN CopyAmt = (RemainingToWrite < BlockSize) ? RemainingToWrite : BlockSize;

    if (CopyAmt > 0) {
      CopyMem (Chunk, ImageBytes, CopyAmt);
      ImageBytes       += CopyAmt;
      RemainingToWrite -= CopyAmt;
    }

    // Zero-pad any remainder of the block (the image ends before the partition does)
    if (CopyAmt < BlockSize) {
      ZeroMem (&((UINT8 *)Chunk)[CopyAmt], BlockSize - CopyAmt);
    }

    // Stamp the image descriptor into the block that spans the fixed offset.
    if (BlockIndex == (SRE_IMAGE_INFO_OFFSET / BlockSize)) {
      SRE_IMAGE_INFO  *ImageInfo;
      UINTN           DescOffset;

      DescOffset = SRE_IMAGE_INFO_OFFSET % BlockSize;
      ASSERT (DescOffset + sizeof (SRE_IMAGE_INFO) <= BlockSize);

      ImageInfo = (SRE_IMAGE_INFO *)&((UINT8 *)Chunk)[DescOffset];
      ImageInfo->Signature     = SRE_IMAGE_INFO_SIG;
      ImageInfo->StructVersion = SRE_IMAGE_INFO_STRUCT_VER;
      ImageInfo->SreFwVersion  = CapsuleFwVersion;
      ZeroMem (ImageInfo->Reserved, sizeof (ImageInfo->Reserved));
    }

    Status = SreStorageWriteBlock (Chunk);
    if (EFI_ERROR (Status)) {
      DEBUG ((DEBUG_ERROR, "[SRE %a] failed to write block - %r\n", __FUNCTION__, Status));
      goto Done;
    }
  }
  
  if (Progress != NULL) {
    Progress ((PartitionIndex * 50) + 50);
  }

  Status = SreStorageWriteClose ();
  if (EFI_ERROR (Status)) {
    DEBUG ((DEBUG_ERROR, "[SRE %a] failed to close write session for partition %d - %r\n", __FUNCTION__, PartitionIndex, Status));
    goto Done;
  }

Done:
  if (Chunk != NULL) {
    FreeAlignedPages (Chunk, EFI_SIZE_TO_PAGES (BlockSize));
  }
  return Status;
}


//
// Public FMP library functions
//

/**
  Register the FMP installer callback.

  @param[in] Function  Installer callback to register.
**/
EFI_STATUS
EFIAPI
RegisterFmpInstaller (
  IN FMP_DEVICE_LIB_REGISTER_FMP_INSTALLER  Function)
{
  UINTN  BlockCount;
  UINTN  BlockSize;
  UINTN  BlockBufferAlignment;

  // Returning EFI_UNSUPPORTED will cause the FMP framework to install a single FMP instance on the ImageHandle.
  // Returning EFI_SUCCESS without registering an installer will result in this driver never binding to a device and properly not installing an ESRT entry.
  mFmpRegisterStatus = !EFI_ERROR (SreStorageInfo (&BlockCount, &BlockSize, &BlockBufferAlignment)) ? EFI_UNSUPPORTED : EFI_SUCCESS;
  return mFmpRegisterStatus;
}

/**
  Register the FMP uninstaller callback.

  @param[in] Function  Uninstaller callback to register.
**/
EFI_STATUS
EFIAPI
RegisterFmpUninstaller (
  IN FMP_DEVICE_LIB_REGISTER_FMP_UNINSTALLER  Function)
{
  return mFmpRegisterStatus;
}

/**
  Set the device context for this FMP instance.

  @param[in]     Handle   Device handle.
  @param[in,out] Context  Device context pointer.
**/
EFI_STATUS
EFIAPI
FmpDeviceSetContext (
  IN EFI_HANDLE  Handle,
  IN OUT VOID    **Context)
{
  // Single FMP instance for platform, no context is needed.
  return EFI_UNSUPPORTED;
}

/**
  Return the size of the SRE storage region.

  @param[out] Size  Size in bytes.
**/
EFI_STATUS
EFIAPI
FmpDeviceGetSize (
  OUT UINTN  *Size
  )
{
  EFI_STATUS  Status;
  UINTN       BlockCount;
  UINTN       BlockSize;
  UINTN       BlockBufferAlignment;

  INPUT_PARAM_CHECK(Size == NULL);

  Status = SreStorageInfo (&BlockCount, &BlockSize, &BlockBufferAlignment);
  if (EFI_ERROR (Status)) {
    DEBUG ((DEBUG_ERROR, "[SRE %a] failed to read boot partition size - %r\n", __FUNCTION__, Status));
    return Status;
  }

  *Size = BlockCount * BlockSize;
  DEBUG ((DEBUG_INFO, "[SRE %a] Size = 0x%08X_%08X - success\n", __FUNCTION__, (*Size >> 32), (*Size & 0xFFFFFFFF)));

  return EFI_SUCCESS;
}

/**
  Return a pointer to the image type ID GUID.

  @param[out] Guid  Image type GUID pointer.
**/
EFI_STATUS
EFIAPI
FmpDeviceGetImageTypeIdGuidPtr (
  OUT EFI_GUID  **Guid
  )
{
  // Return EFI_UNSUPPORTED to indicate gFmpDevicePkgTokenSpaceGuid.PcdFmpDeviceImageTypeIdGuid should be used.
  return EFI_UNSUPPORTED;
}

/**
  Return the supported and current attribute flags.

  @param[out] Supported  Bitmask of supported attributes.
  @param[out] Setting    Bitmask of current attribute settings.
**/
EFI_STATUS
EFIAPI
FmpDeviceGetAttributes (
  OUT UINT64  *Supported,
  OUT UINT64  *Setting
  )
{
  INPUT_PARAM_CHECK(Supported == NULL || Setting == NULL);

  // Supported attribute flags (mask)
  *Supported = (IMAGE_ATTRIBUTE_IMAGE_UPDATABLE |
                IMAGE_ATTRIBUTE_RESET_REQUIRED |
                IMAGE_ATTRIBUTE_AUTHENTICATION_REQUIRED |
                IMAGE_ATTRIBUTE_IN_USE);

  // Flag states (settings)
  *Setting = (IMAGE_ATTRIBUTE_IMAGE_UPDATABLE |
              IMAGE_ATTRIBUTE_RESET_REQUIRED |
              IMAGE_ATTRIBUTE_AUTHENTICATION_REQUIRED |
              IMAGE_ATTRIBUTE_IN_USE);

  DEBUG ((DEBUG_INFO, "[SRE %a] - success\n", __FUNCTION__));
  DEBUG ((DEBUG_INFO, "      Supported bit-mask = 0x%016x\n", *Supported));
  DEBUG ((DEBUG_INFO, "      Setting bit-mask = 0x%016x\n", *Setting));

  return EFI_SUCCESS;
}

/**
  Return the lowest supported firmware version.

  @param[out] LowestSupportedVersion  Minimum version allowed.
**/
EFI_STATUS
EFIAPI
FmpDeviceGetLowestSupportedVersion (
  OUT UINT32  *LowestSupportedVersion
  )
{
  INPUT_PARAM_CHECK(LowestSupportedVersion == NULL);

  // The lowest supported version is fixed at build time via the platform DSC.
  *LowestSupportedVersion = PcdGet32 (PcdFmpDeviceBuildTimeLowestSupportedVersion);
  DEBUG ((DEBUG_INFO, "[SRE %a] Version = 0x%08x - success\n", __FUNCTION__, *LowestSupportedVersion));
  return EFI_SUCCESS;
}

/**
  Return the current firmware version as a Unicode string.

  @param[out] VersionString  Caller-freed version string.
**/
EFI_STATUS
EFIAPI
FmpDeviceGetVersionString (
  OUT CHAR16  **VersionString
  )
{
  EFI_STATUS Status;
  UINT32 VersionNumber;
  UINTN VersionStringSize;

  INPUT_PARAM_CHECK(VersionString == NULL);

  Status = FmpDeviceGetVersion (&VersionNumber);
  if (EFI_ERROR (Status)) {
    return Status;
  }

  // Format U32 as U8.U16.U8 then convert to a Unicode string "xxx.xxxxx.xxx\0"
  VersionStringSize = 14 * sizeof (CHAR16);
  *VersionString = AllocatePool (VersionStringSize);
  if (*VersionString == NULL) {
    DEBUG ((DEBUG_ERROR, "[SRE %a] - out of resources\n", __FUNCTION__));
    return EFI_OUT_OF_RESOURCES;
  }
  if (0 == UnicodeSPrint (
    *VersionString,
    VersionStringSize,
    L"%d.%d.%d",
    (VersionNumber >> 24) & 0xFF,
    (VersionNumber >> 8)  & 0xFFFF,
    VersionNumber         & 0xFF
    ))
  {
    DEBUG ((DEBUG_ERROR, "[SRE %a] - failed to format version string\n", __FUNCTION__));
    return EFI_DEVICE_ERROR;
  }

  DEBUG ((DEBUG_INFO, "[SRE %a] - Version = %S - success\n", __FUNCTION__, *VersionString));
  return EFI_SUCCESS;
}

/**
  Return the current firmware version from the installed descriptor.

  @param[out] Version  Current firmware version.
**/
EFI_STATUS
EFIAPI
FmpDeviceGetVersion (
  OUT UINT32  *Version
  )
{
  EFI_STATUS      Status;
  UINTN           BlockCount;
  UINTN           BlockSize;
  UINTN           BlockBufferAlignment;
  VOID            *BlockBuffer;
  SRE_IMAGE_INFO  *ImageInfo;

  INPUT_PARAM_CHECK(Version == NULL);

  Status = SreStorageInfo (&BlockCount, &BlockSize, &BlockBufferAlignment);
  if (EFI_ERROR (Status)) {
    DEBUG ((DEBUG_ERROR, "[SRE %a] failed to read storage geometry - %r\n", __FUNCTION__, Status));
    return Status;
  }

  // The image descriptor (SRE_IMAGE_INFO) lives at a fixed byte offset from the
  // start of the partition. Read the block that spans that offset and index into
  // it by (SRE_IMAGE_INFO_OFFSET % BlockSize).
  BlockBuffer = AllocateAlignedPages (EFI_SIZE_TO_PAGES (BlockSize), BlockBufferAlignment);
  if (BlockBuffer == NULL) {
    DEBUG ((DEBUG_ERROR, "[SRE %a] - out of resources\n", __FUNCTION__));
    return EFI_OUT_OF_RESOURCES;
  }

  Status = SreStorageRead (SrePartition_A, SRE_IMAGE_INFO_OFFSET / BlockSize, BlockBuffer);
  if (EFI_ERROR (Status)) {
    DEBUG ((DEBUG_ERROR, "[SRE %a] failed to read image descriptor - %r\n", __FUNCTION__, Status));
    FreeAlignedPages (BlockBuffer, EFI_SIZE_TO_PAGES (BlockSize));
    return Status;
  }

  ImageInfo = (SRE_IMAGE_INFO *)&((UINT8 *)BlockBuffer)[SRE_IMAGE_INFO_OFFSET % BlockSize];

  // If no valid image descriptor is present, force version 0x00000000
  if ((ImageInfo->Signature != SRE_IMAGE_INFO_SIG) ||
      (ImageInfo->StructVersion != SRE_IMAGE_INFO_STRUCT_VER)) {
    *Version = 0x00000000;
    DEBUG ((DEBUG_WARN, "[SRE %a] No valid image trailer found, forcing version 0x00000000\n", __FUNCTION__));
    FreeAlignedPages (BlockBuffer, EFI_SIZE_TO_PAGES (BlockSize));
    return EFI_SUCCESS;
  }

  *Version = ImageInfo->SreFwVersion;
  DEBUG ((DEBUG_INFO, "[SRE %a] Version = 0x%08x - success\n", __FUNCTION__, *Version));
  FreeAlignedPages (BlockBuffer, EFI_SIZE_TO_PAGES (BlockSize));
  return EFI_SUCCESS;
}

/**
  Return the hardware instance identifier.

  @param[out] HardwareInstance  Hardware instance value.
**/
EFI_STATUS
EFIAPI
FmpDeviceGetHardwareInstance (
  OUT UINT64  *HardwareInstance
  )
{
  // HW instances not supported
  return EFI_UNSUPPORTED;
}

/**
  Read the installed SRE descriptor from the boot partition.

  @param[out]    Image      Buffer to receive the descriptor.
  @param[in,out] ImageSize  On input, buffer size; on output, bytes written.
**/
EFI_STATUS
EFIAPI
FmpDeviceGetImage (
  OUT    VOID   *Image,
  IN OUT UINTN  *ImageSize
  )
{
  return EFI_UNSUPPORTED;
}

/**
  Validate the capsule image before applying.

  @param[in]  Image            Descriptor image buffer.
  @param[in]  ImageSize        Size of Image.
  @param[out] ImageUpdatable   Result of updatability check.
  @param[out] LastAttemptStatus Detailed status code.
**/
EFI_STATUS
EFIAPI
FmpDeviceCheckImageWithStatus (
  IN  CONST VOID  *Image,
  IN  UINTN       ImageSize,
  OUT UINT32      *ImageUpdatable,
  OUT UINT32      *LastAttemptStatus
  )
{
  EFI_STATUS                Status;
  UINTN                     BpSize;
  UINTN                     BlockCount;
  UINTN                     BlockSize;
  UINTN                     BlockBufferAlignment;

  //
  // Per spec, return EFI_SUCCESS for all checks unless an underlying UEFI based call fails.  The caller is expected
  // to check the ImageUpdatable and LastAttemptStatus variables for information.
  //

  INPUT_PARAM_CHECK(Image == NULL || ImageSize == 0 || ImageUpdatable == NULL || LastAttemptStatus == NULL);

  *ImageUpdatable    = IMAGE_UPDATABLE_INVALID;
  *LastAttemptStatus = LAST_ATTEMPT_STATUS_ERROR_INVALID_FORMAT;

  // Check that the staged WIM will fit into the SRE boot partition
  Status = SreStorageInfo (&BlockCount, &BlockSize, &BlockBufferAlignment);
  if (EFI_ERROR (Status)) {
    DEBUG ((DEBUG_ERROR, "[SRE %a] failed to read the boot partition size - %r\n", __FUNCTION__, Status));
    return Status;
  }
  BpSize = BlockCount * BlockSize;
  if (ImageSize > BpSize)
  {
    DEBUG ((DEBUG_ERROR, "[SRE %a] ERROR: staged WIM will not fit into the SRE boot partition\n", __FUNCTION__));
    return EFI_SUCCESS;
  }

  //
  // TODO:  No security checks are being performed yet, this driver currently relies on the signing of the capsule only
  //

  // Success
  *ImageUpdatable    = IMAGE_UPDATABLE_VALID;
  *LastAttemptStatus = LAST_ATTEMPT_STATUS_SUCCESS;
  return EFI_SUCCESS;
}

/**
  Validate the capsule image (without LastAttemptStatus).

  @param[in]  Image           Descriptor image buffer.
  @param[in]  ImageSize       Size of Image.
  @param[out] ImageUpdatable  Result of updatability check.
**/
EFI_STATUS
EFIAPI
FmpDeviceCheckImage (
  IN  CONST VOID  *Image,
  IN  UINTN       ImageSize,
  OUT UINT32      *ImageUpdatable
  )
{
  UINT32  LastAttemptStatus;
  return FmpDeviceCheckImageWithStatus (Image, ImageSize, ImageUpdatable, &LastAttemptStatus);
}

/**
  Apply the SRE image: stream the staged WIM to the boot partition.

  @param[in]  Image             Descriptor image buffer.
  @param[in]  ImageSize         Size of Image.
  @param[in]  VendorCode        Optional vendor-specific data.
  @param[in]  Progress          Optional progress callback.
  @param[in]  CapsuleFwVersion  Firmware version from the capsule header.
  @param[out] AbortReason       Unused abort reason string.
  @param[out] LastAttemptStatus Detailed status code.
**/
EFI_STATUS
EFIAPI
FmpDeviceSetImageWithStatus (
  IN  CONST VOID                                     *Image,
  IN  UINTN                                          ImageSize,
  IN  CONST VOID                                     *VendorCode        OPTIONAL,
  IN  EFI_FIRMWARE_MANAGEMENT_UPDATE_IMAGE_PROGRESS  Progress           OPTIONAL,
  IN  UINT32                                         CapsuleFwVersion,
  OUT CHAR16                                         **AbortReason,
  OUT UINT32                                         *LastAttemptStatus
  )
{
  EFI_STATUS                Status;
  SRE_FMP_LAS_VARIABLE_DATA LasData;
  UINT32                    ImageUpdatable;
 
  // Input checks
  INPUT_PARAM_CHECK(Image == NULL || ImageSize == 0 || LastAttemptStatus == NULL || AbortReason == NULL);

  // AbortReason string is not used
  *AbortReason = NULL;

  // Check image, propogating any functional errors
  Status = FmpDeviceCheckImageWithStatus (Image, ImageSize, &ImageUpdatable, LastAttemptStatus);
  if (EFI_ERROR (Status)) {
    return EFI_ABORTED;
  }

  // Make sure LastAttemptStatus holds an error if image updatable is not valid
  if (*LastAttemptStatus == LAST_ATTEMPT_STATUS_SUCCESS && ImageUpdatable != IMAGE_UPDATABLE_VALID) {
    *LastAttemptStatus = LAST_ATTEMPT_STATUS_ERROR_UNSUCCESSFUL;
  }

  // If the image is valid, apply the WIM to the SRE storage space
  if (*LastAttemptStatus == LAST_ATTEMPT_STATUS_SUCCESS) {
    Status = ApplyWimToSreStorage (SrePartition_A, Image, ImageSize, CapsuleFwVersion, Progress);
    if (EFI_ERROR(Status)) {
      DEBUG ((DEBUG_ERROR, "[SRE %a] failed to apply WIM to partition 0 - %r\n", __FUNCTION__, Status));
      *LastAttemptStatus = LAST_ATTEMPT_STATUS_ERROR_UNSUCCESSFUL;
    }
    Status = ApplyWimToSreStorage (SrePartition_B, Image, ImageSize, CapsuleFwVersion, Progress);
    if (EFI_ERROR(Status)) {
      DEBUG ((DEBUG_ERROR, "[SRE %a] failed to apply WIM to partition 1 - %r\n", __FUNCTION__, Status));
      *LastAttemptStatus = LAST_ATTEMPT_STATUS_ERROR_UNSUCCESSFUL;
    }
  }

  // Store last attempt status and version for use in ESRT table on next boot
  LasData.LastAttemptStatus = *LastAttemptStatus;
  LasData.LastAttemptVersion = CapsuleFwVersion;
  Status = gRT->SetVariable (
    SRE_FMP_LAS_VARIABLE_NAME,
    &gOdpPkgTokenSpaceGuid,
    EFI_VARIABLE_NON_VOLATILE | EFI_VARIABLE_BOOTSERVICE_ACCESS,
    sizeof (LasData),
    &LasData);
  if (EFI_ERROR (Status)) {
    DEBUG ((DEBUG_ERROR, "[SRE %a] - failed to set last attempt status: %r\n", __FUNCTION__, Status));
  }

  // Return the appropriate status based on the last attempt
  Status = (*LastAttemptStatus == LAST_ATTEMPT_STATUS_SUCCESS) ? EFI_SUCCESS : EFI_ABORTED;
  DEBUG ((EFI_ERROR (Status) ? DEBUG_ERROR : DEBUG_INFO, "[SRE %a] Image write status - %r\n", __FUNCTION__, Status));
  DEBUG ((EFI_ERROR (Status) ? DEBUG_ERROR : DEBUG_INFO, "    Last Attempt Status: 0x%08x\n", *LastAttemptStatus));
  DEBUG ((EFI_ERROR (Status) ? DEBUG_ERROR : DEBUG_INFO, "    Image Version: 0x%08x\n", CapsuleFwVersion));
  return Status;
}

/**
  Apply the SRE image (without LastAttemptStatus).

  @param[in]  Image            Descriptor image buffer.
  @param[in]  ImageSize        Size of Image.
  @param[in]  VendorCode       Optional vendor-specific data.
  @param[in]  Progress         Optional progress callback.
  @param[in]  CapsuleFwVersion Firmware version from the capsule header.
  @param[out] AbortReason      Unused abort reason string.
**/
EFI_STATUS
EFIAPI
FmpDeviceSetImage (
  IN  CONST VOID                                     *Image,
  IN  UINTN                                          ImageSize,
  IN  CONST VOID                                     *VendorCode        OPTIONAL,
  IN  EFI_FIRMWARE_MANAGEMENT_UPDATE_IMAGE_PROGRESS  Progress           OPTIONAL,
  IN  UINT32                                         CapsuleFwVersion,
  OUT CHAR16                                         **AbortReason
  )
{
  UINT32  LastAttemptStatus;
  return FmpDeviceSetImageWithStatus (Image, ImageSize, VendorCode, Progress, CapsuleFwVersion, AbortReason, &LastAttemptStatus);
}

/**
  Lock the SRE boot partition.
**/
EFI_STATUS
EFIAPI
FmpDeviceLock (
  VOID
  )
{
  EFI_STATUS  Status0;
  EFI_STATUS  Status1;

  Status0 = SreStorageLock (SrePartition_A);
  DEBUG (((EFI_ERROR (Status0)) ? DEBUG_ERROR : DEBUG_INFO, "[SRE] boot partition A lock status - %r\n", Status0));

  Status1 = SreStorageLock (SrePartition_B);
  DEBUG (((EFI_ERROR (Status1)) ? DEBUG_ERROR : DEBUG_INFO, "[SRE] boot partition B lock status - %r\n", Status1));

  return (EFI_ERROR (Status0)) ? Status0 : Status1;
}
