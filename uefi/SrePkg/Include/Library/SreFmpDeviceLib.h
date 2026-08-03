/** @file
  Provides SRE FMP update specific information.

  This header defines the small "capsule descriptor" that the SRE Firmware
  Management Protocol (FMP) payload carries. The descriptor is what flows
  through the stock capsule path (Capsule-On-Disk -> PEI -> DxeCapsuleLib ->
  FmpDxe -> FmpDeviceLib::SetImage). It is only a few bytes: it records the
  size and hash of the large recovery WIM and where to find it. The ~1 GB WIM
  itself is NOT part of this descriptor and is never loaded into memory as a
  single buffer; it is staged separately and streamed in chunks by the
  FmpDeviceLib.

  See plan.md (repo root) for the full design rationale.

  Copyright (c) Microsoft Corporation.<BR>

  SPDX-License-Identifier: BSD-2-Clause-Patent

**/

#ifndef __SRE_FMP_DEVICE_LIB__
#define __SRE_FMP_DEVICE_LIB__

#include <Uefi.h>

//
// UEFI variable used to persist the SRE-specific last attempt status across
// the update flow.
//
#define SRE_FMP_LAS_VARIABLE_NAME  L"LastAttemptStatus"
typedef struct {
  UINT32 LastAttemptStatus;
  UINT32 LastAttemptVersion;
} SRE_FMP_LAS_VARIABLE_DATA;

//
// Descriptor signature: 'S','R','E','I' (little-endian 0x49455253).
//
#define SRE_IMAGE_INFO_SIG  SIGNATURE_32 ('S', 'R', 'E', 'I')

//
// Fixed byte offset of the SRE_IMAGE_INFO descriptor from the start of the boot
// partition. 0x00004400 == LBA 34, the first byte of the Microsoft Reserved
// (MSR) partition. It follows the GPT entry array and carries no filesystem, so
// the boot loader never reads it. This offset is intentionally NOT aligned to
// the storage block size; readers locate the block that spans the offset and
// index into it by (SRE_IMAGE_INFO_OFFSET % BlockSize).
//
#define SRE_IMAGE_INFO_OFFSET  0x00004400

//
// Current descriptor structure version.
//
#define SRE_IMAGE_INFO_STRUCT_VER  0x00000001

//
// Hash size carried by the descriptor (SHA-256 = 32 bytes).
//
#define SRE_WIM_HASH_SIZE  32

//
// Image Information
//
// This structure is stored at a fixed byte offset (SRE_IMAGE_INFO_OFFSET) from
// the start of the SRE image partition to record the version of the image
// currently installed to the partition.
//
#pragma pack (1)
typedef struct {
  UINT32  Signature;                    // Descriptor signature ("SREI")
  UINT32  StructVersion;                // Version of this structure (currently 0x00000001)
  UINT32  SreFwVersion;                 // Version of the image currently stored in this partition
  UINT32  Reserved[5];                  // Reserved for future use (ex: image hash)
} SRE_IMAGE_INFO;
#pragma pack ()

//
// Partitions A and B supported for SRE storage operations
//
typedef enum {
  SrePartition_A = 0,
  SrePartition_B = 1
} PARTITION_INDEX;

#endif
