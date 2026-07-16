// Secure Recovery Environment (SRE) Storage Library - NULL instance
//
// Copyright (c) Microsoft Corporation. All rights reserved.
//
// License: MIT
//
#include <PiDxe.h>
#include <Library/DebugLib.h>
#include <Library/DevicePathLib.h>
#include <Library/MemoryAllocationLib.h>
#include <Library/PcdLib.h>
#include <Library/UefiBootServicesTableLib.h>
#include <Protocol/DevicePath.h>
#include <Library/SreStorage.h>

//
// Private definitions
//

typedef struct {
  PARTITION_INDEX  PartitionIndex;
  UINTN            BlockSize;
  UINTN            BlockCount;
  UINTN            BlocksWritten;
} SRE_WRITE_SESSION;

//
// Private global variables
//

SRE_WRITE_SESSION  mSreWriteSession = {0, 0, 0, 0};
BOOLEAN mIsSupported = FALSE;

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
    Status = gBS->LocateDevicePath (&gEfiDevicePathProtocolGuid, &RemainingPath, *Handle);
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
  EFI_DEVICE_PATH_PROTOCOL  *TargetPath;

  // Convert the SRE Device Path PCD string to a DevicePathProtocol
  TargetPath = ConvertTextToDevicePath ((CONST CHAR16*) PcdGetPtr (PcdSreDevicePathString));
  Status = ConnectStorageDevice (TargetPath, &Handle);
  FreePool (TargetPath);
  if (EFI_ERROR(Status)) {
    DEBUG (((Status == EFI_NOT_FOUND) ? DEBUG_INFO : DEBUG_ERROR, "[SreStorageLib] ConnectStorageDevice - %r\n", Status));
    mIsSupported = FALSE;
  }
  else {
    mIsSupported = TRUE;
  }


  ///
  /// TODO:  This is where other device init can be done such as locating a protocol bound to the same device handle
  ///


  // Always return success unless you want the DXE core to ASSERT and halt execution.
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
  //
  // TODO
  //
  
  return EFI_UNSUPPORTED;
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
  //
  // TODO
  //
  
  return EFI_UNSUPPORTED;
}

EFI_STATUS
EFIAPI
SreStorageLock (
  VOID
  )
{
  //
  // TODO
  //
  
  return EFI_UNSUPPORTED;
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
  mSreWriteSession.PartitionIndex = PartitionIndex;
  mSreWriteSession.BlockSize      = 0;
  mSreWriteSession.BlockCount     = 0;
  mSreWriteSession.BlocksWritten  = 0;
  
  *BlockSize  = mSreWriteSession.BlockSize;
  *BlockCount = mSreWriteSession.BlockCount;
  *BlockBuffer = NULL;
  
  //
  // TODO
  //
  
  return EFI_UNSUPPORTED;
}

EFI_STATUS
EFIAPI
SreStorageWriteBlock (
  IN  VOID  *BlockBuffer
  )
{
  if (mSreWriteSession.BlocksWritten >= mSreWriteSession.BlockCount) {
    return EFI_OUT_OF_RESOURCES;
  }
  mSreWriteSession.BlocksWritten++;

  //
  // TODO
  //
  
  return EFI_UNSUPPORTED;
}

EFI_STATUS
EFIAPI
SreStorageWriteClose (
  VOID
  )
{
  if (mSreWriteSession.BlocksWritten != mSreWriteSession.BlockCount) {
    return EFI_NOT_READY;
  }

  //
  // TODO
  //
  
  return EFI_UNSUPPORTED;
}
