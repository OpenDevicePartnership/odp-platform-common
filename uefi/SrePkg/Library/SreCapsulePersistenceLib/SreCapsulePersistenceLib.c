/** @file
  Platform CapsulePersistenceLib that "peels" the SRE WIM payload out of an
  incoming multi-payload FMP capsule before the capsule is persisted across
  reset.

  This is a platform override of MsCorePkg/DxeCapsulePersistenceLib. It reuses
  that library's disk engine verbatim (CapsulePersistence.c / .h, copied into
  this instance) and replaces ONLY the thin public shim so the SRE peel can be
  injected at PersistCapsuleImageAcrossReset() time. Because the override lives
  entirely in the persistence library, the platform consumes the stock,
  unmodified MsCorePkg/CapsuleServiceProtocolDxe driver.

  The staged WIM bytes are written to the EFI System Partition at the path given
  by PcdSreStagingPath, creating intermediate directories as needed. The
  post-reset SRE FmpDeviceLib reads (and later deletes) the same file using that
  same PCD, so both legs target one staged file.

  Peel behavior (only for the SRE capsule; every other capsule is persisted
  unchanged):

    1. The SRE capsule is a stock FMP capsule (gEfiFmpCapsuleGuid) with exactly
       two payloads and no embedded drivers: payload "descriptor" targets the
       platform FMP GUID, payload "WIM" targets the SRE staging GUID.
    2. The WIM payload bytes are written to the staged file on the ESP.
    3. A PayloadItemCount = 1 descriptor-only capsule is rebuilt (descriptor
       payload copied verbatim; CapsuleImageSize and ItemOffsetList recomputed,
       neither of which is covered by the descriptor's signature).
    4. ONLY that small descriptor-only capsule is handed to the stock disk
       engine to persist and queue.

  Copyright (c) Microsoft Corporation. All rights reserved.
  SPDX-License-Identifier: BSD-2-Clause-Patent

**/

#include <Uefi.h>

#include <Guid/FmpCapsule.h>

#include <Library/DebugLib.h>
#include <Library/BaseLib.h>
#include <Library/BaseMemoryLib.h>
#include <Library/MemoryAllocationLib.h>
#include <Library/PcdLib.h>
#include <Library/UefiBootServicesTableLib.h>
#include <Library/CapsulePersistenceLib.h>

#include <Protocol/SimpleFileSystem.h>
#include <Protocol/FirmwareManagement.h>

#include <SreFmpDeviceLib.h>

#include "CapsulePersistence.h"

//
// Staging UpdateImageTypeId carried by the WIM payload. No FmpDxe instance
// claims it; it exists only so the peeler can identify the WIM and so stock
// FmpDxe would harmlessly skip it if the peeler were ever bypassed.
// { d31b249c-bc66-4cdc-be21-30b1b6566b9c }
//
STATIC CONST EFI_GUID  mSreWimStagingGuid = {
  0xd31b249c, 0xbc66, 0x4cdc, { 0xbe, 0x21, 0x30, 0xb1, 0xb6, 0x56, 0x6b, 0x9c }
};

//
// Chunk size used to copy the WIM payload out to the ESP file. Bounds the size
// of each Simple File System write; the whole WIM is already resident in the
// OS-delivered DRAM buffer, so this only caps per-call transfer size.
//
#define SRE_PEEL_WRITE_CHUNK  SIZE_16MB

/**
  Locate the EFI System Partition's Simple File System protocol: the first SFS
  handle that also exposes gEfiPartTypeSystemPartGuid is the ESP. The post-reset
  FmpDeviceLib selects the same volume the same way so both target one file.

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
  Open (creating as needed) the file named by the absolute path FullPath on the
  already-open volume Root, creating every intermediate directory component.

  @param[in]  Root      The opened volume root.
  @param[in]  FullPath  Absolute path from the volume root (e.g. L"\\ODP\\SRE\\OsImage.wim").
  @param[out] OutFile   On success, the opened (read/write/create) leaf file.
**/
STATIC
EFI_STATUS
SreCreatePathFile (
  IN  EFI_FILE_PROTOCOL  *Root,
  IN  CONST CHAR16       *FullPath,
  OUT EFI_FILE_PROTOCOL  **OutFile
  )
{
  EFI_STATUS         Status;
  CHAR16             *PathCopy;
  UINTN              PathLen;
  EFI_FILE_PROTOCOL  *Current;
  EFI_FILE_PROTOCOL  *Next;
  CHAR16             *Component;
  CHAR16             *Cursor;

  *OutFile = NULL;
  Current  = Root;

  PathLen  = StrLen (FullPath);
  PathCopy = AllocateCopyPool ((PathLen + 1) * sizeof (CHAR16), FullPath);
  if (PathCopy == NULL) {
    return EFI_OUT_OF_RESOURCES;
  }

  //
  // Walk each '\'-separated component. Every component except the final one is
  // opened/created as a directory; the final component is the leaf file.
  //
  Cursor = PathCopy;
  while (*Cursor == L'\\') {
    Cursor++;
  }

  Status = EFI_INVALID_PARAMETER;
  while (*Cursor != L'\0') {
    Component = Cursor;
    while ((*Cursor != L'\\') && (*Cursor != L'\0')) {
      Cursor++;
    }

    if (*Cursor == L'\\') {
      //
      // Intermediate directory component.
      //
      *Cursor = L'\0';
      Cursor++;
      while (*Cursor == L'\\') {
        Cursor++;
      }

      Status = Current->Open (
                          Current,
                          &Next,
                          Component,
                          EFI_FILE_MODE_READ | EFI_FILE_MODE_WRITE | EFI_FILE_MODE_CREATE,
                          EFI_FILE_DIRECTORY
                          );
      if (Current != Root) {
        Current->Close (Current);
      }

      if (EFI_ERROR (Status)) {
        DEBUG ((DEBUG_ERROR, "[SRE %a] create dir %s - %r\n", __FUNCTION__, Component, Status));
        goto Done;
      }

      Current = Next;
    } else {
      //
      // Final component: the leaf file. Delete any stale copy, then create
      // fresh so the new write starts clean.
      //
      Status = Current->Open (Current, &Next, Component, EFI_FILE_MODE_READ | EFI_FILE_MODE_WRITE, 0);
      if (!EFI_ERROR (Status)) {
        Next->Delete (Next);
      }

      Status = Current->Open (
                          Current,
                          &Next,
                          Component,
                          EFI_FILE_MODE_READ | EFI_FILE_MODE_WRITE | EFI_FILE_MODE_CREATE,
                          0
                          );
      if (Current != Root) {
        Current->Close (Current);
      }

      if (EFI_ERROR (Status)) {
        DEBUG ((DEBUG_ERROR, "[SRE %a] create file %s - %r\n", __FUNCTION__, Component, Status));
        goto Done;
      }

      *OutFile = Next;
      Status   = EFI_SUCCESS;
      goto Done;
    }
  }

Done:
  if (EFI_ERROR (Status) && (Current != Root) && (Current != NULL)) {
    Current->Close (Current);
  }

  FreePool (PathCopy);
  return Status;
}

/**
  Write a buffer to the staged WIM file (PcdSreStagingPath) on the EFI System
  Partition, creating intermediate directories and replacing any existing file.

  @param[in] Buffer  Source bytes (the WIM payload, resident in DRAM).
  @param[in] Size    Number of bytes to write.
**/
STATIC
EFI_STATUS
SreWriteStagedWim (
  IN VOID    *Buffer,
  IN UINT64  Size
  )
{
  EFI_STATUS                       Status;
  EFI_SIMPLE_FILE_SYSTEM_PROTOCOL  *Sfs;
  EFI_FILE_PROTOCOL                *Root;
  EFI_FILE_PROTOCOL                *File;
  CONST CHAR16                     *StagePath;
  UINT64                           Offset;
  UINT8                            *Cursor;

  if (Buffer == NULL) {
    return EFI_INVALID_PARAMETER;
  }

  Root      = NULL;
  File      = NULL;
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

  Status = SreCreatePathFile (Root, StagePath, &File);
  if (EFI_ERROR (Status)) {
    goto Done;
  }

  Offset = 0;
  Cursor = (UINT8 *)Buffer;
  while (Offset < Size) {
    UINTN  ThisWrite;

    ThisWrite = SRE_PEEL_WRITE_CHUNK;
    if ((UINT64)ThisWrite > (Size - Offset)) {
      ThisWrite = (UINTN)(Size - Offset);
    }

    Status = File->Write (File, &ThisWrite, Cursor);
    if (EFI_ERROR (Status)) {
      DEBUG ((DEBUG_ERROR, "[SRE %a] write at 0x%lx - %r\n", __FUNCTION__, Offset, Status));
      goto Done;
    }

    Offset += ThisWrite;
    Cursor += ThisWrite;
  }

  Status = File->Flush (File);
  if (EFI_ERROR (Status)) {
    DEBUG ((DEBUG_ERROR, "[SRE %a] flush - %r\n", __FUNCTION__, Status));
    goto Done;
  }

  DEBUG ((DEBUG_INFO, "[SRE %a] staged %lu bytes to %s\n", __FUNCTION__, Size, StagePath));

Done:
  if (File != NULL) {
    File->Close (File);
  }

  if (Root != NULL) {
    Root->Close (Root);
  }

  return Status;
}

//
// Stock FMP payload header (FmpPayloadHeaderLib) prepended to the signed image
// before signing. The peeler must skip it to reach the SRE descriptor, since it
// parses the raw capsule (post-reset FmpDxe strips it on the apply path).
//
#define SRE_FMP_PAYLOAD_HEADER_SIGNATURE  SIGNATURE_32 ('M', 'S', 'S', '1')

typedef struct {
  UINT32    Signature;
  UINT32    HeaderSize;
  UINT32    FwVersion;
  UINT32    LowestSupportedVersion;
} SRE_FMP_PAYLOAD_HEADER;

/**
  Given an SRE descriptor FMP payload image, return a pointer to the embedded
  SRE_WIM_DESCRIPTOR, skipping the FMP authentication header and FMP payload
  header when present.

  @param[in]  ImageHeader  The descriptor payload's image header.
  @param[out] OutDesc      On success, pointer to the SRE_WIM_DESCRIPTOR.
**/
STATIC
EFI_STATUS
SreGetDescriptor (
  IN  EFI_FIRMWARE_MANAGEMENT_CAPSULE_IMAGE_HEADER  *ImageHeader,
  OUT CONST SRE_WIM_DESCRIPTOR                      **OutDesc
  )
{
  UINT8                              *Image;
  UINTN                              ImageSize;
  UINTN                              AuthSize;
  CONST SRE_WIM_DESCRIPTOR           *Desc;
  EFI_FIRMWARE_IMAGE_AUTHENTICATION  *Auth;

  *OutDesc  = NULL;
  Image     = (UINT8 *)(ImageHeader + 1);
  ImageSize = ImageHeader->UpdateImageSize;
  AuthSize  = 0;

  if ((ImageHeader->ImageCapsuleSupport & CAPSULE_SUPPORT_AUTHENTICATION) != 0) {
    if (ImageSize < sizeof (EFI_FIRMWARE_IMAGE_AUTHENTICATION)) {
      return EFI_INVALID_PARAMETER;
    }

    Auth     = (EFI_FIRMWARE_IMAGE_AUTHENTICATION *)Image;
    AuthSize = sizeof (Auth->MonotonicCount) + Auth->AuthInfo.Hdr.dwLength;

    if ((AuthSize <= sizeof (Auth->MonotonicCount)) || (AuthSize > ImageSize)) {
      return EFI_INVALID_PARAMETER;
    }
  }

  //
  // The signed image is wrapped in an FMP payload header (FmpPayloadHeaderLib
  // prepends one before signing). Post-reset FmpDxe strips it, but the peeler
  // parses the raw capsule, so skip it here to reach the descriptor. AuthSize
  // accumulates the total bytes skipped before the descriptor.
  //
  if ((ImageSize - AuthSize) >= sizeof (SRE_FMP_PAYLOAD_HEADER)) {
    CONST SRE_FMP_PAYLOAD_HEADER  *Fph;

    Fph = (CONST SRE_FMP_PAYLOAD_HEADER *)(Image + AuthSize);
    if (Fph->Signature == SRE_FMP_PAYLOAD_HEADER_SIGNATURE) {
      if ((Fph->HeaderSize < sizeof (SRE_FMP_PAYLOAD_HEADER)) ||
          (Fph->HeaderSize > (ImageSize - AuthSize)))
      {
        return EFI_INVALID_PARAMETER;
      }

      AuthSize += Fph->HeaderSize;
    }
  }

  if ((ImageSize - AuthSize) < sizeof (SRE_WIM_DESCRIPTOR)) {
    return EFI_INVALID_PARAMETER;
  }

  Desc = (CONST SRE_WIM_DESCRIPTOR *)(Image + AuthSize);
  if (Desc->Signature != SRE_WIM_IMAGE_SIG) {
    return EFI_INVALID_PARAMETER;
  }

  if (Desc->StructVersion != SRE_WIM_DESCRIPTOR_VERSION) {
    return EFI_INVALID_PARAMETER;
  }

  *OutDesc = Desc;
  return EFI_SUCCESS;
}

/**
  If CapsuleHeader is an SRE multi-payload FMP capsule (descriptor + WIM), peel
  the WIM payload out to a separate ESP file and build a descriptor-only capsule
  to persist across reset in its place.

  @param[in]  CapsuleHeader  The OS-delivered capsule.
  @param[out] OutCapsule     On success, a newly-allocated descriptor-only
                             capsule the caller must FreePool after persisting.

  @retval EFI_SUCCESS        The WIM was staged and *OutCapsule was built.
  @retval EFI_NOT_FOUND      Not an SRE capsule; caller persists CapsuleHeader as-is.
  @retval Other              A parse, stage, or allocation error occurred.
**/
STATIC
EFI_STATUS
SrePeelWimFromCapsule (
  IN  EFI_CAPSULE_HEADER  *CapsuleHeader,
  OUT EFI_CAPSULE_HEADER  **OutCapsule
  )
{
  EFI_FIRMWARE_MANAGEMENT_CAPSULE_HEADER        *FmpHeader;
  EFI_FIRMWARE_MANAGEMENT_CAPSULE_IMAGE_HEADER  *WimImage;
  EFI_FIRMWARE_MANAGEMENT_CAPSULE_IMAGE_HEADER  *DescImage;
  EFI_FIRMWARE_MANAGEMENT_CAPSULE_IMAGE_HEADER  *ItemHeader;
  UINT64                                        *ItemOffsetList;
  UINTN                                         ItemCount;
  UINTN                                         Index;
  CONST SRE_WIM_DESCRIPTOR                      *Desc;
  EFI_STATUS                                    Status;
  UINTN                                         DescChunkSize;
  UINTN                                         FmpToPayload;
  UINTN                                         NewImageSize;
  EFI_CAPSULE_HEADER                            *NewCapsule;
  EFI_FIRMWARE_MANAGEMENT_CAPSULE_HEADER        *NewFmpHeader;
  UINT64                                        *NewOffsetList;
  UINT8                                         *NewPayload;

  *OutCapsule = NULL;

  //
  // Only stock FMP capsules can be SRE capsules.
  //
  if (!CompareGuid (&CapsuleHeader->CapsuleGuid, &gEfiFmpCapsuleGuid)) {
    return EFI_NOT_FOUND;
  }

  FmpHeader = (EFI_FIRMWARE_MANAGEMENT_CAPSULE_HEADER *)((UINT8 *)CapsuleHeader + CapsuleHeader->HeaderSize);
  if (FmpHeader->Version != EFI_FIRMWARE_MANAGEMENT_CAPSULE_HEADER_INIT_VERSION) {
    return EFI_NOT_FOUND;
  }

  //
  // The SRE capsule has exactly two payloads and no embedded drivers. Anything
  // else is not an SRE capsule; let the stock path handle it unchanged.
  //
  if ((FmpHeader->EmbeddedDriverCount != 0) || (FmpHeader->PayloadItemCount != 2)) {
    return EFI_NOT_FOUND;
  }

  ItemOffsetList = (UINT64 *)(FmpHeader + 1);
  ItemCount      = (UINTN)FmpHeader->EmbeddedDriverCount + (UINTN)FmpHeader->PayloadItemCount;

  //
  // Locate the WIM payload (staging GUID) and the descriptor payload.
  //
  WimImage  = NULL;
  DescImage = NULL;
  for (Index = 0; Index < ItemCount; Index++) {
    ItemHeader = (EFI_FIRMWARE_MANAGEMENT_CAPSULE_IMAGE_HEADER *)((UINT8 *)FmpHeader + ItemOffsetList[Index]);
    if (CompareGuid (&ItemHeader->UpdateImageTypeId, &mSreWimStagingGuid)) {
      WimImage = ItemHeader;
    } else {
      DescImage = ItemHeader;
    }
  }

  if ((WimImage == NULL) || (DescImage == NULL)) {
    return EFI_NOT_FOUND;
  }

  //
  // Confirm payload 0 really is a signed SRE descriptor before peeling.
  //
  Status = SreGetDescriptor (DescImage, &Desc);
  if (EFI_ERROR (Status)) {
    DEBUG ((DEBUG_ERROR, "[SRE %a] descriptor parse failed - %r\n", __FUNCTION__, Status));
    return Status;
  }

  //
  // Stage the WIM payload bytes to the ESP at the shared compile-time path.
  //
  Status = SreWriteStagedWim ((UINT8 *)(WimImage + 1), WimImage->UpdateImageSize);
  if (EFI_ERROR (Status)) {
    DEBUG ((DEBUG_ERROR, "[SRE %a] stage WIM failed - %r\n", __FUNCTION__, Status));
    return Status;
  }

  //
  // Build a descriptor-only (PayloadItemCount = 1) capsule. ItemOffsetList[0]
  // is measured from the start of the FMP header.
  //
  FmpToPayload  = sizeof (EFI_FIRMWARE_MANAGEMENT_CAPSULE_HEADER) + sizeof (UINT64);
  DescChunkSize = sizeof (EFI_FIRMWARE_MANAGEMENT_CAPSULE_IMAGE_HEADER)
                  + DescImage->UpdateImageSize
                  + DescImage->UpdateVendorCodeSize;
  NewImageSize = CapsuleHeader->HeaderSize + FmpToPayload + DescChunkSize;

  NewCapsule = AllocateZeroPool (NewImageSize);
  if (NewCapsule == NULL) {
    return EFI_OUT_OF_RESOURCES;
  }

  //
  // Copy the original capsule header (including any bytes up to HeaderSize) and
  // fix up the total image size.
  //
  CopyMem (NewCapsule, CapsuleHeader, CapsuleHeader->HeaderSize);
  NewCapsule->CapsuleImageSize = (UINT32)NewImageSize;

  NewFmpHeader                      = (EFI_FIRMWARE_MANAGEMENT_CAPSULE_HEADER *)((UINT8 *)NewCapsule + CapsuleHeader->HeaderSize);
  NewFmpHeader->Version             = FmpHeader->Version;
  NewFmpHeader->EmbeddedDriverCount = 0;
  NewFmpHeader->PayloadItemCount    = 1;

  NewOffsetList    = (UINT64 *)(NewFmpHeader + 1);
  NewOffsetList[0] = FmpToPayload;

  NewPayload = (UINT8 *)NewFmpHeader + FmpToPayload;
  CopyMem (NewPayload, DescImage, DescChunkSize);

  *OutCapsule = NewCapsule;
  DEBUG ((DEBUG_INFO, "[SRE %a] built descriptor-only capsule (0x%x bytes)\n", __FUNCTION__, (UINT32)NewImageSize));
  return EFI_SUCCESS;
}

/**
  Persists a Capsule across reset and adds it to the queue.

  SRE override: if CapsuleHeader is an SRE multi-payload capsule, the WIM
  payload is peeled out to a separate ESP file and only the descriptor-only
  capsule is persisted. Every other capsule is persisted unchanged.

  @param[in]      CapsuleHeader           EFI_CAPSULE_HEADER pointing to Capsule Image to persist.
  @param[out]     CapsuleIdentifier       Data for the capsule that has been persisted (optional).

  @retval         EFI_SUCCESS             Capsule was successfully persisted.
  @retval         Other                   See InternalPersistCapsuleImageAcrossReset.
**/
EFI_STATUS
EFIAPI
PersistCapsuleImageAcrossReset (
  IN  EFI_CAPSULE_HEADER            *CapsuleHeader,
  OUT CAPSULE_PERSISTED_IDENTIFIER  *CapsuleIdentifier    OPTIONAL
  )
{
  EFI_STATUS          Status;
  EFI_CAPSULE_HEADER  *PeeledCapsule;

  if (CapsuleHeader == NULL) {
    return EFI_INVALID_PARAMETER;
  }

  //
  // EFI_NOT_FOUND means this is an ordinary capsule; persist it unchanged.
  //
  PeeledCapsule = NULL;
  Status        = SrePeelWimFromCapsule (CapsuleHeader, &PeeledCapsule);
  if (Status == EFI_NOT_FOUND) {
    PeeledCapsule = NULL;
  } else if (EFI_ERROR (Status)) {
    DEBUG ((DEBUG_ERROR, "[SRE %a] peel failed - %r\n", __FUNCTION__, Status));
    return Status;
  }

  Status = InternalPersistCapsuleImageAcrossReset (
             (PeeledCapsule != NULL) ? PeeledCapsule : CapsuleHeader,
             CapsuleIdentifier
             );

  if (PeeledCapsule != NULL) {
    FreePool (PeeledCapsule);
  }

  return Status;
}

/**
  Returns a pointer to a specific capsule.

  @param[in]    CapsuleIdentifier     Pointer to the identifier for the capsule.
  @param[out]   CapsuleData           Pointer to a buffer to hold the capsule (OPTIONAL).
  @param[out]   CapsuleDataSize       On input, size of CapsuleData allocation; on output, the
                                      size of the persisted capsule.

  @retval       EFI_SUCCESS           Capsule was found and output data is valid.
  @retval       Other                 See InternalGetPersistedCapsuleData.
**/
EFI_STATUS
EFIAPI
GrabPersistedCapsuleByIdentifier (
  IN  CAPSULE_PERSISTED_IDENTIFIER  *CapsuleIdentifier,
  OUT EFI_CAPSULE_HEADER            *CapsuleData OPTIONAL,
  OUT UINTN                         *CapsuleDataSize
  )
{
  if (CapsuleIdentifier == NULL) {
    return EFI_INVALID_PARAMETER;
  }

  return InternalGetPersistedCapsuleData (CapsuleIdentifier->CapsuleId, CapsuleIdentifier->CapsuleHash, CapsuleData, CapsuleDataSize);
}

/**
  Deletes a capsule by id.

  @param[in]  CapsuleId           The ID of the capsule to delete.

  @retval     EFI_SUCCESS         Capsule was found and deleted.
  @retval     Other               See InternalDeletePersistedCapsuleData.
**/
EFI_STATUS
EFIAPI
DeletePersistedCapsuleById (
  IN  UINT32  CapsuleId
  )
{
  return InternalDeletePersistedCapsuleData (CapsuleId);
}

/**
  Deletes all capsules stored on the medium of persistence.

  @retval   EFI_SUCCESS         Capsules were deleted (or none were present).
  @retval   Other               See InternalDeleteAllCapsulesOnFileSystem.
**/
EFI_STATUS
EFIAPI
DeleteAllPersistedCapsules (
  VOID
  )
{
  return InternalDeleteAllCapsulesOnFileSystem ();
}
