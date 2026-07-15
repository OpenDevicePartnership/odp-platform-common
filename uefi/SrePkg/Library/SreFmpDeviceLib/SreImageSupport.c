/** @file
  Private image-support functions for the SRE NVMe FMP Device Library.

  Groups the staged-WIM file access (locate the EFI System Partition, open, and
  delete the staged WIM) together with the WIM apply/verify path that streams
  the staged image to the NVMe boot partitions. These are the private helpers
  behind the public FmpDeviceLib entry points in SreFmpDeviceLib.c, kept in
  their own file so that the main library file shows only how it implements the
  public FMP API.

  Copyright (c) Microsoft Corporation. All rights reserved.

  SPDX-License-Identifier: BSD-2-Clause-Patent

**/
#include <PiDxe.h>
#include <Library/BaseCryptLib.h>
#include <Library/BaseMemoryLib.h>
#include <Library/DebugLib.h>
#include <Library/MemoryAllocationLib.h>
#include <Library/PcdLib.h>
#include <Library/UefiBootServicesTableLib.h>
#include <Protocol/SimpleFileSystem.h>
#include <Library/FmpDeviceLib.h>
#include <SreFmpDeviceLib.h>
#include "SreImageSupport.h"
#include <Library/SreStorage.h>

// NVME Boot Partition indexes for the primary and backup partitions
#define SRE_PRIMARY_BOOT_PARTITION  0
#define SRE_BACKUP_BOOT_PARTITION   1

/**
  Validate that an SRE_WIM_DESCRIPTOR holds a recognized header. Confirms the
  descriptor signature and structure version match the values this library
  produces. This guards against reading uninitialized boot-partition contents
  (for example, a partition that has never had an image set) as if it were a
  real descriptor.

  @param[in]  Descriptor  Pointer to the descriptor to validate. May be NULL.

  @retval TRUE   The descriptor signature and structure version are valid.
  @retval FALSE  The descriptor is NULL or carries an unrecognized header.
**/
BOOLEAN
SreIsDescriptorValid (
  IN CONST SRE_WIM_DESCRIPTOR  *Descriptor
  )
{
  if (Descriptor == NULL) {
    return FALSE;
  }

  if (Descriptor->Signature != SRE_WIM_IMAGE_SIG) {
    return FALSE;
  }

  if (Descriptor->StructVersion != SRE_WIM_DESCRIPTOR_VERSION) {
    return FALSE;
  }

  return TRUE;
}

/**
  Locate the EFI System Partition's Simple File System protocol: the first SFS
  handle that also exposes gEfiPartTypeSystemPartGuid is the ESP. The pre-reset
  peeler selects the same volume the same way so both target one staged file.

  @param[out] OutSfs  On success, the ESP Simple File System protocol.
**/
STATIC
EFI_STATUS
SreLocateEspSfs (
  OUT EFI_SIMPLE_FILE_SYSTEM_PROTOCOL  **OutSfs
  )
{
  EFI_STATUS  Status;
  EFI_HANDLE  *Handles;
  UINTN       Count;
  UINTN       Index;
  VOID        *Dummy;

  *OutSfs = NULL;
  Handles = NULL;
  Count   = 0;

  Status = gBS->LocateHandleBuffer (ByProtocol, &gEfiSimpleFileSystemProtocolGuid, NULL, &Count, &Handles);
  if (EFI_ERROR (Status)) {
    return Status;
  }

  Status = EFI_NOT_FOUND;
  for (Index = 0; Index < Count; Index++) {
    if (EFI_ERROR (gBS->HandleProtocol (Handles[Index], &gEfiPartTypeSystemPartGuid, &Dummy))) {
      continue;
    }

    Status = gBS->HandleProtocol (Handles[Index], &gEfiSimpleFileSystemProtocolGuid, (VOID **)OutSfs);
    if (!EFI_ERROR (Status)) {
      break;
    }
  }

  FreePool (Handles);
  return Status;
}

/**
  Open the staged WIM file (PcdSreStagingPath) on the EFI System Partition with
  the requested mode. The returned handle stays valid after the volume root is
  closed; the caller closes (or deletes) it.

  @param[in]  OpenMode  EFI_FILE_MODE_* flags to open the staged file with.
  @param[out] OutFile   On success, the open staged WIM file handle.
**/
EFI_STATUS
SreOpenStagedWim (
  IN  UINT64             OpenMode,
  OUT EFI_FILE_PROTOCOL  **OutFile
  )
{
  EFI_STATUS                       Status;
  EFI_SIMPLE_FILE_SYSTEM_PROTOCOL  *Sfs;
  EFI_FILE_PROTOCOL                *Root;
  EFI_FILE_PROTOCOL                *File;
  CONST CHAR16                     *StagePath;

  if (OutFile == NULL) {
    return EFI_INVALID_PARAMETER;
  }

  *OutFile  = NULL;
  Root      = NULL;
  StagePath = (CONST CHAR16 *)PcdGetPtr (PcdSreStagingPath);

  Status = SreLocateEspSfs (&Sfs);
  if (EFI_ERROR (Status)) {
    DEBUG ((DEBUG_ERROR, "[SRE %a] ESP not found - %r\n", __FUNCTION__, Status));
    return Status;
  }

  Status = Sfs->OpenVolume (Sfs, &Root);
  if (EFI_ERROR (Status)) {
    DEBUG ((DEBUG_ERROR, "[SRE %a] OpenVolume - %r\n", __FUNCTION__, Status));
    return Status;
  }

  File   = NULL;
  Status = Root->Open (Root, &File, (CHAR16 *)StagePath, OpenMode, 0);
  Root->Close (Root);
  if (EFI_ERROR (Status)) {
    return Status;
  }

  *OutFile = File;
  return EFI_SUCCESS;
}

/**
  Delete the staged WIM file from the EFI System Partition. Treats a missing
  file as success (best-effort post-commit cleanup).
**/
EFI_STATUS
SreDeleteStagedWim (
  VOID
  )
{
  EFI_STATUS         Status;
  EFI_FILE_PROTOCOL  *File;

  File   = NULL;
  Status = SreOpenStagedWim (EFI_FILE_MODE_READ | EFI_FILE_MODE_WRITE, &File);
  if (Status == EFI_NOT_FOUND) {
    return EFI_SUCCESS;
  }

  if (EFI_ERROR (Status)) {
    return Status;
  }

  //
  // EFI_FILE_PROTOCOL.Delete closes the handle regardless of the result, so the
  // handle must not be closed again here.
  //
  Status = File->Delete (File);
  if (EFI_ERROR (Status)) {
    DEBUG ((DEBUG_ERROR, "[SRE %a] delete staged WIM - %r\n", __FUNCTION__, Status));
    return Status;
  }

  DEBUG ((DEBUG_INFO, "[SRE %a] deleted staged WIM\n", __FUNCTION__));
  return EFI_SUCCESS;
}

EFI_STATUS
ApplyWimToSreStorage(
  IN  UINT32  PartitionIndex,
  IN CONST SRE_WIM_DESCRIPTOR                       *Descriptor,
  IN  EFI_FIRMWARE_MANAGEMENT_UPDATE_IMAGE_PROGRESS Progress OPTIONAL)
{
  EFI_STATUS         Status;
  UINTN              BlockCount;
  UINTN              BlockSize;
  UINTN              BlockIndex;
  VOID               *Chunk;
  EFI_FILE_PROTOCOL  *File;
  UINTN              Percent;
  UINTN              LastPercent;
  VOID               *HashCtx;
  UINT64             RemainingToHash;
  UINT8              ComputedHash[SHA256_DIGEST_SIZE];

  // Items the Done: tag examines to free resources
  Chunk       = NULL;
  File        = NULL;
  HashCtx     = NULL;
  LastPercent = (PartitionIndex == 0) ? 0 : 50;

  // Open a write session to the target partition. This reports the storage
  // block geometry (count and size); no storage-specific (NVMe) knowledge is
  // needed here beyond streaming that many blocks of that size.
  Status = SreStorageWriteOpen (PartitionIndex, &BlockCount, &BlockSize, &Chunk);
  if (EFI_ERROR (Status)) {
    DEBUG ((DEBUG_ERROR, "[SRE %a] failed to open write session - %r\n", __FUNCTION__, Status));
    return Status;
  }

  if (Chunk == NULL) {
    DEBUG ((DEBUG_ERROR, "[SRE %a] failed to allocate block buffer\n", __FUNCTION__));
    Status = EFI_OUT_OF_RESOURCES;
    goto Done;
  }

  Status = SreOpenStagedWim (EFI_FILE_MODE_READ, &File);
  if (EFI_ERROR (Status)) {
    DEBUG ((DEBUG_ERROR, "[SRE %a] failed to open staged WIM - %r\n", __FUNCTION__, Status));
    goto Done;
  }

  // Initialize the running SHA-256 over the WIM bytes so the data committed to
  // the boot partition is verified against the signed descriptor hash.
  HashCtx = AllocatePool (Sha256GetContextSize ());
  if ((HashCtx == NULL) || !Sha256Init (HashCtx)) {
    DEBUG ((DEBUG_ERROR, "[SRE %a] failed to initialize SHA-256 context\n", __FUNCTION__));
    Status = EFI_OUT_OF_RESOURCES;
    goto Done;
  }
  RemainingToHash = Descriptor->WimSize;

  // Write the staged WIM one block at a time, zero-padding the tail.
  if (Progress != NULL) {
    Progress (LastPercent);
  }
  for (BlockIndex = 0; BlockIndex < BlockCount; BlockIndex++) {
    UINTN ReadAmt = BlockSize;

    Status = File->Read (File, &ReadAmt, Chunk);
    if (EFI_ERROR (Status)) {
      DEBUG ((DEBUG_ERROR, "[SRE %a] failed to read staged WIM - %r\n", __FUNCTION__, Status));
      goto Done;
    }

    // Hash only the real WIM bytes (exclude zero padding and the appended
    // descriptor) so the digest matches the signed descriptor's WimHash.
    if (RemainingToHash > 0) {
      UINTN HashAmt = (ReadAmt < RemainingToHash) ? ReadAmt : (UINTN)RemainingToHash;
      if (!Sha256Update (HashCtx, Chunk, HashAmt)) {
        DEBUG ((DEBUG_ERROR, "[SRE %a] SHA-256 update failed\n", __FUNCTION__));
        Status = EFI_DEVICE_ERROR;
        goto Done;
      }
      RemainingToHash -= HashAmt;
    }

    // If the WIM has no more data, pad the rest of the buffer with zeros and add descriptor
    if (ReadAmt < BlockSize) {
      ZeroMem(&((UINT8*)Chunk)[ReadAmt], BlockSize - ReadAmt);
      CopyMem(&((UINT8*)Chunk)[BlockSize - sizeof (SRE_WIM_DESCRIPTOR)], Descriptor, sizeof (SRE_WIM_DESCRIPTOR));
    }

    Status = SreStorageWriteBlock (Chunk);
    if (EFI_ERROR (Status)) {
      DEBUG ((DEBUG_ERROR, "[SRE %a] failed to write block - %r\n", __FUNCTION__, Status));
      goto Done;
    }

    if (Progress != NULL) {
      Percent = ((BlockIndex * 100) / BlockCount) + ((PartitionIndex == 0) ? 0 : 50);
      if (Percent != LastPercent) {
        Progress (Percent);
        LastPercent = Percent;
      }
    }
  }

  // Verify the streamed WIM matches the signed descriptor hash before close.
  // A short file (RemainingToHash != 0) or a mismatch aborts before close so
  // unverified data is never made the active boot partition.
  if ((RemainingToHash != 0) || !Sha256Final (HashCtx, ComputedHash)) {
    DEBUG ((DEBUG_ERROR, "[SRE %a] staged WIM shorter than WimSize or hash finalize failed\n", __FUNCTION__));
    Status = EFI_SECURITY_VIOLATION;
    goto Done;
  }
  if (CompareMem (ComputedHash, Descriptor->WimHash, SHA256_DIGEST_SIZE) != 0) {
    DEBUG ((DEBUG_ERROR, "[SRE %a] staged WIM hash mismatch - refusing to commit\n", __FUNCTION__));
    Status = EFI_SECURITY_VIOLATION;
    goto Done;
  }

  // Close the write session, flushing and committing the data to the partition
  // indicated when the session was opened.
  Status = SreStorageWriteClose ();
  if (EFI_ERROR (Status)) {
    DEBUG ((DEBUG_ERROR, "[SRE %a] failed to close write session for partition %d - %r\n", __FUNCTION__, PartitionIndex, Status));
    goto Done;
  }

Done:
  if (HashCtx != NULL) {
    FreePool (HashCtx);
  }
  if (Chunk != NULL) {
    FreeAlignedPages (Chunk, EFI_SIZE_TO_PAGES (BlockSize));
  }
  if (File != NULL) {
    File->Close (File);
  }
  if (Progress != NULL && LastPercent != 100) {
    Progress (100);
  }
  return Status;
}
