/** @file
  Declarations for the SRE image-support helpers (implemented in
  SreImageSupport.c): staged-WIM file access and the WIM apply/verify path.

  This header intentionally includes no other headers. Each consuming source
  file provides the required types (Uefi.h, Library/FmpDeviceLib.h,
  Protocol/SimpleFileSystem.h, and the SreFmpDeviceLib descriptor header)
  before including it.

  Copyright (c) Microsoft Corporation. All rights reserved.

  SPDX-License-Identifier: BSD-2-Clause-Patent

**/
#ifndef __SRE_IMAGE_SUPPORT_H__
#define __SRE_IMAGE_SUPPORT_H__

BOOLEAN
SreIsDescriptorValid (
  IN CONST SRE_WIM_DESCRIPTOR  *Descriptor
  );

EFI_STATUS
SreOpenStagedWim (
  IN  UINT64             OpenMode,
  OUT EFI_FILE_PROTOCOL  **OutFile
  );

EFI_STATUS
SreDeleteStagedWim (
  VOID
  );

EFI_STATUS
ApplyWimToSreStorage (
  IN  UINT32                                         PartitionIndex,
  IN  CONST SRE_WIM_DESCRIPTOR                       *Descriptor,
  IN  EFI_FIRMWARE_MANAGEMENT_UPDATE_IMAGE_PROGRESS  Progress      OPTIONAL
  );


#endif // __SRE_IMAGE_SUPPORT_H__
