//
// Secure Recovery Environment (SRE) storage support
//
// Copyright (c) Microsoft Corporation. All rights reserved.
//
// MIT License
//
#ifndef _SRE_STORAGE_H_
#define _SRE_STORAGE_H_


//
// Return the geometry of the SRE storage area.  If the device is not present or
// supported this returns EFI_UNSUPPORTED and any other call will also fail.
//
// BlockCount           - Number of blocks in the storage area
// BlockSize            - Size of a block in bytes
// BlockBufferAlignment - Required address alignment (in bytes) for buffers passed
//                        to SreStorageRead and SreStorageWriteBlock
//
// EFI_INVALID_PARAMETER - If any output pointer is NULL
// EFI_UNSUPPORTED       - If the SRE storage device is not present or supported
//
EFI_STATUS
EFIAPI
SreStorageInfo (
  OUT UINTN           *BlockCount,
  OUT UINTN           *BlockSize,
  OUT UINTN           *BlockBufferAlignment
  );

//
// Read a region of the storage partition
//
// PartitionIndex - Target storage partition index
// BlockIndex     - Index of the block to read
// BlockBuffer    - Buffer to receive the block data (must be BlockSize bytes and aligned to
//                  BlockBufferAlignment as reported by SreStorageInfo)
//
// EFI_INVALID_PARAMETER - BlockBuffer is NULL, misaligned, or if Partition or Block Indexes are out of range
// EFI_UNSUPPORTED       - The SRE storage device is not present or supported
// EFI_ABORTED           - A write session is currently open
// EFI_NOT_READY         - The underlying HW protocol is not ready
// EFI_PROTOCOL_ERROR    - The underlying HW protocol reported an error
//
EFI_STATUS
EFIAPI
SreStorageRead (
  IN  PARTITION_INDEX PartitionIndex,
  IN  UINTN           BlockIndex,
  OUT VOID            *BlockBuffer
  );

//
// Prepare the storage area for update.  Internally an index is set to block 0 and a write must
// be called BlockCount times for a close to succeed.
//
// PartitionIndex  - Target storage partition index
//
// EFI_INVALID_PARAMETER - PartitionIndex is out of range
// EFI_UNSUPPORTED       - The SRE storage device is not present or supported
// EFI_ABORTED           - A write session is currently open
// EFI_NOT_READY         - The underlying HW protocol is not ready
// EFI_PROTOCOL_ERROR    - The underlying HW protocol reported an error
//
EFI_STATUS
EFIAPI
SreStorageWriteOpen (
  IN  PARTITION_INDEX PartitionIndex
  );

//
// Write the input data to the currently indexed block and move the internal block pointer to the
// next block
//
// BlockBuffer - Buffer containing data to write to the storage area, all bytes from the block are written.
//               Must be BlockSize bytes and aligned to BlockBufferAlignment as reported by SreStorageInfo.
//
// EFI_INVALID_PARAMETER - BlockBuffer is NULL or misaligned
// EFI_END_OF_MEDIA      - The internal indexed block is at end of storage area
// EFI_UNSUPPORTED       - The SRE storage device is not present or supported
// EFI_NOT_READY         - No write session is open, or the underlying HW protocol is not ready
// EFI_PROTOCOL_ERROR    - The underlying HW protocol reported an error
//
EFI_STATUS
EFIAPI
SreStorageWriteBlock (
  IN  VOID *BlockBuffer
  );

//
// Close and flush a write session
//
// EFI_UNSUPPORTED    - The SRE storage device is not present or supported
// EFI_ABORTED        - The internal block pointer is not currently at the end of the storage area
// EFI_NOT_READY      - No write session is open, or the underlying HW protocol is not ready
// EFI_PROTOCOL_ERROR - The underlying HW protocol reported an error
//
EFI_STATUS
EFIAPI
SreStorageWriteClose (
  VOID
  );

//
// Abort an open write session without committing
//
// EFI_UNSUPPORTED    - The SRE storage device is not present or supported
// EFI_NOT_READY      - No write session is open, or the underlying HW protocol is not ready
// EFI_PROTOCOL_ERROR - The underlying HW protocol reported an error
//
EFI_STATUS
EFIAPI
SreStorageWriteAbort (
  VOID
  );

//
// Perform the pre-OS handoff lock that requires a power reset to unlock.
//
// EFI_INVALID_PARAMETER - PartitionIndex is out of range
// EFI_UNSUPPORTED       - The SRE storage device is not present or supported
// EFI_ABORTED           - A write session is currently open
// EFI_NOT_READY         - The underlying HW protocol is not ready
// EFI_PROTOCOL_ERROR    - The underlying HW protocol reported an error
//
EFI_STATUS
EFIAPI
SreStorageLock (
  IN  PARTITION_INDEX PartitionIndex
  );

#endif // _SRE_STORAGE_H_
