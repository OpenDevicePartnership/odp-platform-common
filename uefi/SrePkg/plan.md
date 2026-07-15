# Specification: Capsule Updates for the Secure Recovery Environment (SRE)

## 1. Problem Statement

We need to deliver an **SRE image** in capsule form that contains a `.wim` of ~0.5 GB+. Because UEFI after a reboot
uses a single contiguous buffer to load the image into RAM and measure during the PEI or early boot phase, using a full
capsule is not practical.  We need a solution that allows the large image to be staged by Windows, persist across
a reboot, and handed to the FMP to process in small manageable chunks.

## 2. Constraints

- No modifications to common EDK II / Mu code. All new logic lives in platform libraries/drivers and host tooling.
- No new signing keys or in-firmware key infrastructure; reuse the platform's existing FMP signing key
- The OS delivers the capsule as a single in-memory buffer to `UpdateCapsule()`
- Platform or family specific libraries can be re-written or modified as needed
- The SRE image must never be fully resident in RAM post reboot
- This spec describes the design in generic EDK II terms; platform- and vendor-specific implementation notes live in the code
- Test signatures will be used for all testing

## 3. Solution Summary

The single capsule the OS delivers carries **two payloads**: a small signed **descriptor** and the large **SRE image**.
Runtime services places the two payloads into the staging area, reboots, then update path carries only the small
descriptor through the boot/measure process, leaving the SRE image for the FMP driver to apply:

1. **OS Delivery** - The OS sends the multi-payload capsule (descriptor + SRE image) to UEFI as a single in-memory buffer.
2. **Runtime Services** - The `UpdateCapsule` handler separates the two payloads, persists the descriptor as an independent
   capsule, writes the SRE image as a plain file in the staging area, then reboots.
3. **Early Boot** - The capsule path coalesces, measures, and applies the descriptor capsule as usual. The SRE image is
   a plain file the capsule path never references, so it is never read into memory.
4. **FMP Driver** - The descriptor capsule reaches the SRE FMP driver, which then streams the SRE image from the staging
   area to its final destination in bounded chunks.

## 4. Interfaces

### 4.1 SRE update capsule layout

The SRE update capsule is a **multi-payload** FMP capsule laid out as follows:

```c
SRE_UPDATE_CAPSULE_LAYOUT {
    EFI_CAPSULE_HEADER {
        EFI_GUID  CapsuleGuid;            // gEfiFmpCapsuleGuid (this is an FMP capsule)
        UINT32    HeaderSize;             // sizeof (EFI_CAPSULE_HEADER)
        UINT32    Flags;                  // CAPSULE_FLAGS_PERSIST_ACROSS_RESET | CAPSULE_FLAGS_INITIATE_RESET
        UINT32    CapsuleImageSize;       // total size of everything in this layout
    }
    EFI_FIRMWARE_MANAGEMENT_CAPSULE_HEADER {
        UINT32    Version;                // 1
        UINT16    EmbeddedDriverCount;    // 0  (no embedded drivers)
        UINT16    PayloadItemCount;       // 2  (Payload 0 = descriptor, Payload 1 = SRE image)
        UINT64    ItemOffsetList[2];      // [0] -> byte offset of Payload 0 (descriptor)
                                          // [1] -> byte offset of Payload 1 (SRE image)
    }

    // ===== Payload 0: Signed descriptor ====================
    PAYLOAD_0_DESCRIPTOR {
        EFI_FIRMWARE_MANAGEMENT_CAPSULE_IMAGE_HEADER {
            UINT32    Version;                // 3
            EFI_GUID  UpdateImageTypeId;      // <platform FMP GUID> (claimed by the SRE FmpDxe)
            UINT8     UpdateImageIndex;       // 1
            UINT8     Reserved[3];            // padding
            UINT32    UpdateImageSize;        // size of the signed image below (a few KB)
            UINT32    UpdateVendorCodeSize;   // 0
            UINT64    UpdateHardwareInstance; // 0
            UINT64    ImageCapsuleSupport;    // CAPSULE_SUPPORT_AUTHENTICATION (0x01)
        }
        EFI_FIRMWARE_IMAGE_AUTHENTICATION {
            UINT64                     MonotonicCount; // anti-rollback counter
            WIN_CERTIFICATE_UEFI_GUID  AuthInfo;       // PKCS#7 signature over
                                                       //   (descriptor bytes || MonotonicCount)
        }
        FMP_PAYLOAD_HEADER {
            UINT32  Signature;                // 'MSS1' (stock FMP payload header)
            UINT32  HeaderSize;
            UINT32  FwVersion;
            UINT32  LowestSupportedVersion;
        }
        SRE_WIM_DESCRIPTOR  Descriptor;       // Data to describe the SRE Image
    }

    // ===== Payload 1: SRE WIM image ===============
    PAYLOAD_1_SRE_IMAGE {

        EFI_FIRMWARE_MANAGEMENT_CAPSULE_IMAGE_HEADER {
            UINT32    Version;                // 3
            EFI_GUID  UpdateImageTypeId;      // d31b249c-bc66-4cdc-be21-30b1b6566b9c indicates SRE payload
            UINT8     UpdateImageIndex;       // 1
            UINT8     Reserved[3];            // padding
            UINT32    UpdateImageSize;        // Size of raw WIM
            UINT32    UpdateVendorCodeSize;   // 0
            UINT64    UpdateHardwareInstance; // 0
            UINT64    ImageCapsuleSupport;    // 0  (unsigned -> data is confirmed using hash in descriptor)
        }
        // NO EFI_FIRMWARE_IMAGE_AUTHENTICATION
        // No FMP_PAYLOAD_HEADER
        UINT8  RawSreImage[ /* WimSize */ ];  // raw SRE WIM image
    }
}
```

### 4.2 Descriptor structure

The descriptor structure that makes up the data portion of the first image in the capsule contains information the firmware
management protocol will need in order to stream the raw SRE image to its final destination.  The first 2 fields (sig and
version) will always be present in every future release of this structure.  The following fields and offsets are dictated
by the version number.

SRE WIM Descriptor **v1**:

```c
#pragma pack(1)
SRE_WIM_DESCRIPTOR {
  UINT32   Signature;     // Structure signature string ('SWim')
  UINT32   StructVersion; // Version of structure
  // Fields defined by v1 (52 bytes total including sig and ver)
  UINT32   WimVersion;    // Should match payload 0 FMP_PAYLOAD_HEADER.FwVersion
  UINT64   WimSize;       // Exact byte size of the WIM binary
  UINT8    WimHash[32];   // Hash over the WIM binary (SHA-256)
}
#pragma pack()
```

Note that this structure is also used as a trailer when the WIM is written.  So the destination region must be at least
`(WimSize + sizeof(SRE_WIM_DESCRIPTOR))` bytes.

### 4.3 FmpDeviceLib::SetImage contract

The firmware management protocol driver's `FmpDeviceLib::SetImage` call receives a buffer containing the SRE_WIM_DESCRIPTOR
structure in RAM. Its job is then to open the staging area, initialize the mass‑storage device for update, stream and
hash the WIM data from the staging area to the destination, verify the hash, then commit the final image. How much memory
the transfer uses is left to the platform FMP driver, given the large image size.

Since this capsule's 2nd payload is a Windows image, security for it is handled through secure-boot where the UEFI loader
is expected to validate the signature.  The hash is intended to only verify data is valid through the entire process.  It
is recommended to hash each packet streamed to the primary destination area, verify the write hash, re-read and hash all
packets, verify the read hash, then do the same update to the backup partition to allow A/B recovery support.

Once the image has been uploaded, a copy of the SRE_WIM_DESCRIPTOR will be appended to the end of the SRE area as a method
to read the image manifest during boot.  If the descriptor structure size plus the image size is too large for the destination,
the update will fail.

## 5. What Changes and How

Summary of the items touched when implementing this feature.

### 5.1 `BuildSreCapsule.py` (new build-host tool)

A Python build-host tool assembles the single FMP capsule from a recovery `.wim`. It is self-contained, depending only on
the open-source TianoCore EDK II PyTool libraries (`edk2-pytool-extensions`, `edk2-pytool-library`) and their built-in
signers — no vendor-specific tooling.

What it does:

1. Reads the `.wim` once; records its exact byte size and a SHA-256 over those bytes.
2. Builds the `SRE_WIM_DESCRIPTOR` (§4.2) for payload 0 — filling `WimSize`, `WimHash`, and `WimVersion`.
3. Emits one multi-payload FMP capsule (`PayloadItemCount = 2`): payload 0 = descriptor, payload 1 = the raw `.wim`.
   Payload 0 is wrapped in a stock `FMP_PAYLOAD_HEADER` and PKCS#7-signed with the platform's FMP signing key; payload 1
   is left **unsigned** and carries no `FMP_PAYLOAD_HEADER` — its integrity is bound to the signed descriptor by `WimSize`
   + `WimHash` (§4.2). Both payloads use Version-3 (48-byte) `EFI_FIRMWARE_MANAGEMENT_CAPSULE_IMAGE_HEADER`s, with
   `ImageCapsuleSupport = CAPSULE_SUPPORT_AUTHENTICATION` on payload 0 and `0` on payload 1.
4. A single `--version` is applied in lock-step everywhere it must match: the capsule/ESRT version, each payload's FMP
   `FwVersion`, and the descriptor's `WimVersion`. `StructVersion` is fixed at v1.
5. Optionally writes the matching standalone Windows Update `.inf` (and, with the WDK, a `.cat`) that references the
   capsule by payload 0's ESRT GUID.

Signing uses the built-in `signtool` signer for a real detached PKCS#7 (on-device verifiable); a `pyopenssl` mode exists
for local tool round-trip testing only.

### 5.2 Platform pre-reset WIM peeler (platform `CapsulePersistenceLib` override)

The peel is implemented as a platform **`CapsulePersistenceLib` override** (`SreCapsulePersistenceLib`) that the
stock, unmodified `MsCorePkg/CapsuleServiceProtocolDxe` consumes. The driver is not forked and there is no separate
runtime handler: the platform DSC maps `CapsulePersistenceLib` to this instance for that driver, so the split runs
inside `PersistCapsuleImageAcrossReset()` — the single point where the stock driver hands each capsule to the disk
engine — **before** the capsule is persisted, so the large WIM never enters the capsule persist/queue/apply path. The
override reuses MU's disk engine (`CapsulePersistence.c`) verbatim and only replaces the thin public shim.

What it does:

1. Recognizes the SRE capsule by parsing the FMP `ItemOffsetList` (payload 0 = descriptor for the SRE FMP GUID;
   payload 1 = WIM for the SRE staging GUID), then validates payload 0 is a well-formed signed SRE descriptor (it skips
   the `EFI_FIRMWARE_IMAGE_AUTHENTICATION` and `FMP_PAYLOAD_HEADER` to reach and check the `SRE_WIM_DESCRIPTOR`).
2. Writes payload 1's WIM bytes to a separate file in the staging area at the fixed build-time path `PcdSreStagingPath`
   (a private, non-OEM-tunable PCD shared with the FMP), overwriting any stale copy (only one SRE update is ever in
   flight). If the staging volume lacks room for the WIM, the update fails pre-reset.
3. Rebuilds a `PayloadItemCount = 1`, descriptor-only capsule (payload 0 copied verbatim; recomputes `CapsuleImageSize`
   and `ItemOffsetList` — neither is covered by payload 0's signature, so it still verifies).
4. Delegates that descriptor-only capsule to the underlying persist + queue path; reset then proceeds normally.

The split runs at runtime DXE, where full system DRAM is available, so briefly holding the OS-delivered two-payload
buffer to copy the WIM out is acceptable — the post-reset apply path never sees the WIM.

### 5.3 Platform `FmpDeviceLib` (new FMP support)

The platform `FmpDeviceLib` provides the SRE implementation of the firmware management protocol support.

What it does:

1. Validates the received descriptor (signature, version, sizes).
2. Opens the staged WIM file the peeler wrote, pinned to the staging volume so a same-named file elsewhere can't be
   substituted.
3. Streams the WIM to the storage device's staging area in bounded, page-aligned chunks (chunk size taken from the
   device's reported update granularity), feeding each chunk to a stock incremental SHA as it goes — nothing is committed
   yet.
4. After the full image streams, compares the running hash to the descriptor's `WimSize`/`WimHash`. **Only on a match**
   does it commit, making the staged data the live image; a short read or mismatch returns a security violation and never
   commits.
5. Appends a copy of the descriptor as a **trailer** after the image (outside the hashed region) so later boots can read
   the installed version/manifest
6. Deletes the staged WIM file after a successful commit.
7. Records and reports the last-attempt status.

The library installs a **single FMP instance** connected to the device named by the device-path PCD (`PcdSreDevicePathString`)
and checks that it exposes the NVMe boot-partition feature. `RegisterFmpInstaller` reads that result and returns `EFI_UNSUPPORTED`
— which directs the FMP framework to install a single FMP instance (and thus publish the ESRT entry) — only when the configured
device is present and supported; otherwise it returns `EFI_SUCCESS` without registering an installer, so no FMP instance binds
and no ESRT entry is produced.
