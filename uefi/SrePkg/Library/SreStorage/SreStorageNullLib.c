//
// Secure Recovery Environment (SRE) Storage Library - NULL instance
//
// Copyright (c) Microsoft Corporation. All rights reserved.
// License: MIT
//

#include <PiDxe.h>

#include <Library/SreStorage.h>
#include <Library/SreFmpDeviceLib.h>

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

  //
  // TODO: Connect the controller and locate needed protocols, save handles, etc.
  //

  // Always return success unless you want the DXE core to ASSERT and halt execution.
  return EFI_SUCCESS;
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
  //
  // TODO
  //
  
  return EFI_UNSUPPORTED;
}

// Read a single block from the boot partition
EFI_STATUS
EFIAPI
SreStorageRead (
  IN  PARTITION_INDEX PartitionIndex,
  IN  UINTN           BlockIndex,
  OUT VOID            *BlockBuffer
  )
{
  //
  // TODO
  //
  
  return EFI_UNSUPPORTED;
}

// Open a write session to the target boot partition.
EFI_STATUS
EFIAPI
SreStorageWriteOpen (
  IN  PARTITION_INDEX PartitionIndex
  )
{
  
  //
  // TODO
  //
  
  return EFI_UNSUPPORTED;
}

// Write the next block via Firmware Image Download.
EFI_STATUS
EFIAPI
SreStorageWriteBlock (
  IN  VOID  *BlockBuffer
  )
{

  //
  // TODO
  //
  
  return EFI_UNSUPPORTED;
}

// Commit the write session
EFI_STATUS
EFIAPI
SreStorageWriteClose (
  VOID
  )
{

  //
  // TODO
  //
  
  return EFI_UNSUPPORTED;
}

// Abort an open write session without committing
EFI_STATUS
EFIAPI
SreStorageWriteAbort (
  VOID
  )
{

  //
  // TODO
  //
  
  return EFI_UNSUPPORTED;
}

// SRE Storage lock mechanism that requires a power reset to unlock
EFI_STATUS
EFIAPI
SreStorageLock (
  IN  PARTITION_INDEX PartitionIndex
  )
{
  //
  // TODO
  //
  
  return EFI_UNSUPPORTED;
}

