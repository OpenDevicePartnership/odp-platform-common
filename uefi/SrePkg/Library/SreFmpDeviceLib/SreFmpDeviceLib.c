/** @file
  Secure Recovery Environment (SRE) Firmware Management Protocol (FMP) Device Library.  No ASSERTs are being used
  due to it being an FMP and we want to handle all errors without halting.

  Copyright (c) Microsoft Corporation. All rights reserved.

  SPDX-License-Identifier: BSD-2-Clause-Patent

**/
#include <PiDxe.h>
#include <Guid/SystemResourceTable.h>
#include <Library/DebugLib.h>
#include <Library/FmpDeviceLib.h>
#include <Library/MemoryAllocationLib.h>
#include <Library/PrintLib.h>
#include <Library/UefiRuntimeServicesTableLib.h>
#include <Protocol/SimpleFileSystem.h>
#include <SreFmpDeviceLib.h>
#include "SreImageSupport.h"
#include <Library/SreStorage.h>

// Global to keep Register FmpInstaller and FmpUninstaller having the same return values
EFI_STATUS  mFmpRegisterStatus = EFI_SUCCESS;

// Using a macro to guarantee we don't use an ASSERT or other method to halt execution when validating input
// parameters due to this being an FMP driver that can not halt boot execution.
#define INPUT_PARAM_CHECK(param_checks)                                       \
  if (param_checks) {                                                         \
    DEBUG ((DEBUG_ERROR, "[SRE %a] - invalid parameter\n", __FUNCTION__));    \
    return EFI_INVALID_PARAMETER;                                             \
  }


//
// Public FMP library functions
//

EFI_STATUS
EFIAPI
RegisterFmpInstaller (
  IN FMP_DEVICE_LIB_REGISTER_FMP_INSTALLER  Function)
{
  // Returning EFI_UNSUPPORTED will cause the FMP framework to install a single FMP instance on the ImageHandle.
  // Returning EFI_SUCCESS without registering an installer will result in this driver never binding to a device and properly not installing an ESRT entry.
  mFmpRegisterStatus = IsSupported () ? EFI_UNSUPPORTED : EFI_SUCCESS;
  return mFmpRegisterStatus;
}

EFI_STATUS
EFIAPI
RegisterFmpUninstaller (
  IN FMP_DEVICE_LIB_REGISTER_FMP_UNINSTALLER  Function)
{
  return mFmpRegisterStatus;
}

EFI_STATUS
EFIAPI
FmpDeviceSetContext (
  IN EFI_HANDLE  Handle,
  IN OUT VOID    **Context)
{
  // Single FMP instance for platform, no context is needed.
  return EFI_UNSUPPORTED;
}

EFI_STATUS
EFIAPI
FmpDeviceGetSize (
  OUT UINTN  *Size
  )
{
  EFI_STATUS  Status;
  INPUT_PARAM_CHECK(Size == NULL);

  Status = SreStorageSize (Size);
  if (EFI_ERROR (Status)) {
    DEBUG ((DEBUG_ERROR, "[SRE %a] failed to read boot partition size - %r\n", __FUNCTION__, Status));
  }
  else {
    DEBUG ((DEBUG_INFO, "[SRE %a] Size = 0x%08X_%08X - success\n", __FUNCTION__, (*Size >> 32), (*Size & 0xFFFFFFFF)));
  } 
  return Status;
}

EFI_STATUS
EFIAPI
FmpDeviceGetImageTypeIdGuidPtr (
  OUT EFI_GUID  **Guid
  )
{
  // Return EFI_UNSUPPORTED to indicate gFmpDevicePkgTokenSpaceGuid.PcdFmpDeviceImageTypeIdGuid should be used.
  return EFI_UNSUPPORTED;
}

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

EFI_STATUS
EFIAPI
FmpDeviceGetVersion (
  OUT UINT32  *Version
  )
{
  EFI_STATUS          Status;
  SRE_WIM_DESCRIPTOR  Descriptor;
  UINTN Size = sizeof(SRE_WIM_DESCRIPTOR);

  INPUT_PARAM_CHECK(Version == NULL);

  // The Get/Set functions pass the ImageDescriptor instead of the raw WIM file
  Status = FmpDeviceGetImage ((VOID *)&Descriptor, &Size);
  if (Status == EFI_NOT_FOUND) {
    *Version = 0x00000000;
    DEBUG ((DEBUG_WARN, "[SRE %a] No valid image found, forcing version 0x00000000\n", __FUNCTION__));
    Status = EFI_SUCCESS;
  } else if (EFI_ERROR (Status)) {
    return Status;
  } else {
    *Version = Descriptor.WimVersion;
  }

  DEBUG ((DEBUG_INFO, "[SRE %a] Version = 0x%08x - success\n", __FUNCTION__, *Version));
  return Status;
}

EFI_STATUS
EFIAPI
FmpDeviceGetHardwareInstance (
  OUT UINT64  *HardwareInstance
  )
{
  // HW instances not supported
  return EFI_UNSUPPORTED;
}

EFI_STATUS
EFIAPI
FmpDeviceGetImage (
  OUT    VOID   *Image,
  IN OUT UINTN  *ImageSize
  )
{
  EFI_STATUS          Status;
  UINTN               PartitionSize;

  INPUT_PARAM_CHECK(Image == NULL || ImageSize == NULL);

  // The image descriptor is what this FMP passes around which resides at the end of the boot partition region.
  Status = SreStorageSize (&PartitionSize);
  if (EFI_ERROR (Status)) {
    DEBUG ((DEBUG_ERROR, "[SRE %a] failed to read boot partition size - %r\n", __FUNCTION__, Status));
    return Status;
  }
  if (*ImageSize < sizeof(SRE_WIM_DESCRIPTOR)) {
    Status = EFI_BUFFER_TOO_SMALL;
    DEBUG ((DEBUG_ERROR, "[SRE %a] input buffer too small to read the boot partition descriptor - %r\n", __FUNCTION__, Status));
    *ImageSize = sizeof(SRE_WIM_DESCRIPTOR);
    return Status;
  }

  Status = SreStorageRead (0, PartitionSize - sizeof(SRE_WIM_DESCRIPTOR), Image, sizeof(SRE_WIM_DESCRIPTOR));
  if (EFI_ERROR(Status))
  {
    DEBUG ((DEBUG_ERROR, "[SRE %a] failed to read boot partition descriptor - %r\n", __FUNCTION__, Status));
    return Status;
  }

  *ImageSize = sizeof(SRE_WIM_DESCRIPTOR);

  if (!SreIsDescriptorValid ((CONST SRE_WIM_DESCRIPTOR *)Image)) {
    DEBUG ((DEBUG_WARN, "[SRE %a] - no valid image descriptor found\n", __FUNCTION__));
    return EFI_NOT_FOUND;
  }

  DEBUG ((DEBUG_INFO, "[SRE %a] - success\n", __FUNCTION__));
  return EFI_SUCCESS;
}

EFI_STATUS
EFIAPI
FmpDeviceCheckImageWithStatus (
  IN  CONST VOID  *Image,
  IN  UINTN       ImageSize,
  OUT UINT32      *ImageUpdatable,
  OUT UINT32      *LastAttemptStatus
  )
{
  CONST SRE_WIM_DESCRIPTOR  *Descriptor;
  EFI_STATUS                Status;
  EFI_FILE_PROTOCOL         *File;
  UINTN                     BpSize;

  // Per spec, return EFI_SUCCESS for all checks unless an underlying UEFI based call fails.  The caller is expected
  // to check the ImageUpdatable and LastAttemptStatus variables for information.

  INPUT_PARAM_CHECK(Image == NULL || ImageSize == 0 || ImageUpdatable == NULL || LastAttemptStatus == NULL);
  Descriptor = (CONST SRE_WIM_DESCRIPTOR *)Image;
  if (ImageSize < sizeof (SRE_WIM_DESCRIPTOR)) {
    DEBUG ((DEBUG_ERROR, "[SRE %a] ERROR: invalid input image size of %d, expected %d\n", __FUNCTION__, ImageSize, sizeof (SRE_WIM_DESCRIPTOR)));
    return EFI_SUCCESS;
  }

  DEBUG ((DEBUG_INFO, "[SRE %a] SRE_WIM_DESCRIPTOR:\n", __FUNCTION__));
  DEBUG ((DEBUG_INFO, "    Signature = %c%c%c%c\n", Descriptor->Signature & 0xFF, (Descriptor->Signature >> 8) & 0xFF, (Descriptor->Signature >> 16) & 0xFF, (Descriptor->Signature >> 24) & 0xFF));
  DEBUG ((DEBUG_INFO, "    StructVersion = %u\n", Descriptor->StructVersion));
  DEBUG ((DEBUG_INFO, "    WimVersion  = %u\n", Descriptor->WimVersion));
  DEBUG ((DEBUG_INFO, "    WimSize       = 0x%lx bytes\n", Descriptor->WimSize));
  DEBUG ((DEBUG_INFO, "    WimHash       ="));
  for (UINTN i = 0; i < sizeof (Descriptor->WimHash); i++) {
    DEBUG ((DEBUG_INFO, " %02x", Descriptor->WimHash[i]));
  }
  DEBUG ((DEBUG_INFO, "\n"));

  *ImageUpdatable    = IMAGE_UPDATABLE_INVALID;
  *LastAttemptStatus = LAST_ATTEMPT_STATUS_ERROR_INVALID_FORMAT;

  // Check that the input descriptor is valid
  if (!SreIsDescriptorValid (Descriptor)) {
    return EFI_SUCCESS;
  }

  // Check that the staged WIM will fit into the SRE boot partition
  Status = SreStorageSize (&BpSize);
  if (EFI_ERROR (Status)) {
    DEBUG ((DEBUG_ERROR, "[SRE %a] failed to read boot partition size - %r\n", __FUNCTION__, Status));
    return Status;
  }
  if ((Descriptor->WimSize + sizeof(SRE_WIM_DESCRIPTOR)) > BpSize)
  {
    DEBUG ((DEBUG_ERROR, "[SRE %a] ERROR: staged WIM will not fit into the SRE boot partition\n", __FUNCTION__));
    return EFI_SUCCESS;
  }

  // Check that the staged WIM is accessible and readable
  File = NULL;
  Status = SreOpenStagedWim (EFI_FILE_MODE_READ, &File);
  if (EFI_ERROR (Status)) {
    *LastAttemptStatus = LAST_ATTEMPT_STATUS_ERROR_INSUFFICIENT_RESOURCES;
    DEBUG ((DEBUG_ERROR, "[SRE %a] ERROR: staged WIM is not accessible - %r\n", __FUNCTION__, Status));
    return EFI_SUCCESS;
  }
  File->Close (File);

  // Success
  *ImageUpdatable    = IMAGE_UPDATABLE_VALID;
  *LastAttemptStatus = LAST_ATTEMPT_STATUS_SUCCESS;
  return EFI_SUCCESS;
}

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
  CONST SRE_WIM_DESCRIPTOR  *Descriptor;
  SRE_FMP_LAS_VARIABLE_DATA LasData;
  UINT32                    ImageUpdatable;
 
  // Input checks
  INPUT_PARAM_CHECK(Image == NULL || ImageSize == 0 || LastAttemptStatus == NULL || AbortReason == NULL);
  Descriptor = (CONST SRE_WIM_DESCRIPTOR *)Image;

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
    Status = ApplyWimToSreStorage (0, Descriptor, Progress);
    if (EFI_ERROR(Status)) {
      DEBUG ((DEBUG_ERROR, "[SRE %a] failed to apply WIM to partition 0 - %r\n", __FUNCTION__, Status));
      *LastAttemptStatus = LAST_ATTEMPT_STATUS_ERROR_UNSUCCESSFUL;
    }
    Status = ApplyWimToSreStorage (1, Descriptor, Progress);
    if (EFI_ERROR(Status)) {
      DEBUG ((DEBUG_ERROR, "[SRE %a] failed to apply WIM to partition 1 - %r\n", __FUNCTION__, Status));
      *LastAttemptStatus = LAST_ATTEMPT_STATUS_ERROR_UNSUCCESSFUL;
    }
  }

  // The staged WIM has been committed and verified on both boot partitions;
  // delete the ESP staging file so it is not left behind. Best-effort: a delete
  // failure does not invalidate the already-committed image.
  if (*LastAttemptStatus == LAST_ATTEMPT_STATUS_SUCCESS) {
    Status = SreDeleteStagedWim ();
    if (EFI_ERROR (Status)) {
      DEBUG ((DEBUG_WARN, "[SRE %a] failed to delete staged WIM after commit - %r\n", __FUNCTION__, Status));
    }
  }

  // Store last attempt status and version for use in ESRT table on next boot
  LasData.LastAttemptStatus = *LastAttemptStatus;
  LasData.LastAttemptVersion = Descriptor->WimVersion;
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
  DEBUG ((EFI_ERROR (Status) ? DEBUG_ERROR : DEBUG_INFO, "    Image Version: 0x%08x\n", Descriptor->WimVersion));
  return Status;
}

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

EFI_STATUS
EFIAPI
FmpDeviceLock (
  VOID
  )
{
  EFI_STATUS  Status;

  Status = SreStorageLock ();
  DEBUG (((EFI_ERROR (Status)) ? DEBUG_ERROR : DEBUG_INFO, "[SRE] boot partition lock status - %r\n", Status));

  return Status;
}

