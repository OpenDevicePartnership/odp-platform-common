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
// Descriptor signature: 'S','W','i','m' (little-endian 0x6D695753).
//
#define SRE_WIM_IMAGE_SIG  SIGNATURE_32 ('S', 'W', 'i', 'm')

//
// Current descriptor structure version.
//
#define SRE_WIM_DESCRIPTOR_VERSION  0x00000001

//
// Hash size carried by the descriptor (SHA-256 = 32 bytes).
//
#define SRE_WIM_HASH_SIZE  32

//
// SRE_WIM_DESCRIPTOR
//
// This is the entire FMP payload the SRE capsule delivers (a few bytes). It
// is authenticated by the stock FMP PKCS#7 envelope, so every field below is
// signed. The large WIM is NOT appended to the capsule: the runtime peeler
// splits it out and writes it to the EFI System Partition at the fixed
// build-time path given by PcdSreStagingPath. The blob
// is bound to this descriptor by WimSize + WimHash; the FmpDeviceLib opens
// that file, re-computes the hash over the bytes it actually streams, and
// compares against WimHash, so an attacker cannot substitute a different blob
// without breaking the signature over this descriptor.
//
#pragma pack (1)
typedef struct {
  UINT32    Signature;                       // SRE_WIM_IMAGE_SIG ('SWim')
  UINT32    StructVersion;                   // SRE_WIM_DESCRIPTOR_VERSION
  UINT32    WimVersion;                      // WIM firmware version, u8.u16.u8
                                             // (matches FmpDeviceGetVersion format)
  UINT64    WimSize;                         // Exact size of the WIM, in bytes.
  UINT8     WimHash[SRE_WIM_HASH_SIZE];      // SHA-256 over the WIM bytes.
} SRE_WIM_DESCRIPTOR;
#pragma pack ()

#endif
