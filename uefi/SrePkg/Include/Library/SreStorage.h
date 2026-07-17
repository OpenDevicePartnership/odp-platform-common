//
// Secure Recovery Environment (SRE) storage support for the NVME drive
//
// Copyright (c) Microsoft Corporation. All rights reserved.
//
// MIT License
//
#ifndef _SRE_STORAGE_H_
#define _SRE_STORAGE_H_

//
// Partitions A and B supported for SRE storage operations
//
typedef enum {
  SrePartition_A = 0,
  SrePartition_B = 1
} PARTITION_INDEX;

//
// Return TRUE if the SRE storage device is present and supported.  If FALSE, any other call will return EFI_UNSUPPORTED.
//
BOOLEAN
EFIAPI
IsSupported (
  VOID
  );

//
// Return the size of the storage partition. 
//
EFI_STATUS
EFIAPI
SreStorageSize (
  OUT UINTN  *Size
  );

//
// Read a region of the storage partition. Offset and length must be DWORD aligned.
//
EFI_STATUS
EFIAPI
SreStorageRead (
  IN  PARTITION_INDEX PartitionIndex,
  IN  UINT64          OffsetBytes,
  OUT VOID            *Buffer,
  IN  UINTN           Length
  );

//
// Perform the pre-OS handoff lock that requires a power reset to unlock.
//
EFI_STATUS
EFIAPI
SreStorageLock (
  VOID
  );

//
// Open a write session.  Each block must be written in sequence and no other commands may be called before closing.
// The BlockBuffer must be freed by the caller after use.
//
EFI_STATUS
EFIAPI
SreStorageWriteOpen (
  IN  PARTITION_INDEX PartitionIndex,
  OUT UINTN           *BlockCount,
  OUT UINTN           *BlockSize,
  OUT VOID            **BlockBuffer
  );

//
// Write a block of data to the storage partition.  BlockBuffer must point to an allocated block of the size reported
// by SreStorageWriteOpen and each write indexes an internal pointer to the next block.
//
EFI_STATUS
EFIAPI
SreStorageWriteBlock (
  IN  VOID  *BlockBuffer
  );

//
// Close and flush a write session. Will fail if SreStorageWriteBlock has not been called the number of times indicated
// by BlockCount.
//
EFI_STATUS
EFIAPI
SreStorageWriteClose (
  VOID
  );


#endif // _SRE_STORAGE_H_
