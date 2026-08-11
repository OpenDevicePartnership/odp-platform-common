/**
  SRE FMP update specific information.

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
// the boot loader never reads it.
//
#define SRE_IMAGE_INFO_OFFSET  0x00004400

//
// SRE Image reporting structure layout version
//
#define SRE_IMAGE_INFO_STRUCT_VER  0x00000001

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
