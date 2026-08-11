#!/usr/bin/env python3
#
# Build + sign the single-payload SRE recovery capsule
#
# Capsule construction:
#   FAT32 partition image
#     -> FMP payload header and monotonic count
#     -> OEM payload signature
#     -> FMP authentication header
#     -> FMP image header
#     -> FMP capsule header
#     -> UEFI capsule header
#     -> Windows INF and signed catalog
#
import argparse
import datetime
import os
import struct
import uuid

from edk2toollib.uefi.edk2.fmp_payload_header import FmpPayloadHeaderClass
from edk2toollib.uefi.fmp_auth_header import FmpAuthHeaderClass
from edk2toollib.windows.capsule import inf_generator2
from edk2toolext.capsule import capsule_helper

# EFI_FMP image header v3 ImageCapsuleSupport bit
CAPSULE_SUPPORT_AUTHENTICATION = 0x0000000000000001

# Must match gSreEsrtGuid (SrePkg.dec) as injected into the FMP driver's PcdFmpDeviceImageTypeIdGuid and the
# ESRT/Windows update uses it to identify this capsule.
SRE_ESRT_GUID = uuid.UUID("9777ff5d-3c53-4316-9af3-b99fb9c60fb5")

# Image header packing for Version-3 EFI_FIRMWARE_MANAGEMENT_CAPSULE_IMAGE_HEADER is not yet supported by edk2toollib
def pack_image_header_v3(type_guid, image_index, payload_len, capsule_support):
    return struct.pack(
        "<I16sB3xIIQQ",
        3,                    # Version
        type_guid.bytes_le,   # UpdateImageTypeId
        image_index & 0xFF,   # UpdateImageIndex
        payload_len,          # UpdateImageSize
        0,                    # UpdateVendorCodeSize
        0,                    # UpdateHardwareInstance
        capsule_support,      # ImageCapsuleSupport
    )

def parse_args():
    parser = argparse.ArgumentParser(description="Build the SRE recovery capsule and Windows Update package.")
    parser.add_argument(
        "--partition-image-path",
        required=True,
        help="Path to the raw FAT32 boot-partition image",
    )
    parser.add_argument(
        "--capsule-version",
        required=True,
        type=lambda value: int(value, 0),
        help="FMP, ESRT, and capsule version as a decimal or 0x-prefixed value",
    )
    parser.add_argument(
        "--lsv",
        required=True,
        type=lambda value: int(value, 0),
        help="Lowest supported version as a decimal or 0x-prefixed value",
    )
    parser.add_argument(
        "--monotonic-count",
        required=True,
        type=int,
        help="Monotonic count used by FMP authentication",
    )
    return parser.parse_args()

def create_unsigned_fmp_payload(args, build_artifacts_dir):

    # Wrap the raw FAT32 partition image in an FMP payload header. The monotonic
    # count is appended because it is included in the bytes the OEM must sign.
    with open(args.partition_image_path, "rb") as partition_image_file:
        partition_image_data = partition_image_file.read()

    fmp_payload = FmpPayloadHeaderClass()
    fmp_payload.FwVersion = args.capsule_version
    fmp_payload.LowestSupportedVersion = args.lsv
    fmp_payload.Payload = partition_image_data
    payload_data = fmp_payload.Encode() + struct.pack("<Q", args.monotonic_count)

    # Drop the unsigned payload to disk and return its path
    unsigned_fmp_payload_path = os.path.join(build_artifacts_dir, "unsigned_fmp_payload.bin")
    with open(unsigned_fmp_payload_path, "wb") as unsigned_fmp_payload_file:
        unsigned_fmp_payload_file.write(payload_data)
    return unsigned_fmp_payload_path

def sign_payload(args, unsigned_fmp_payload_path):

    # TODO: The OEM must sign all bytes at unsigned_fmp_payload_path with its
    # private key. Those bytes contain the FMP payload header, the raw FAT32
    # partition image, and the monotonic count. The implementation may use args
    # for OEM-specific configuration and must return the path to a raw PKCS#7
    # detached signature (the WIN_CERTIFICATE body) over those bytes, matching
    # what FmpAuthHeaderClass.AuthInfo.cert_data expects and what the FMP driver
    # verifies against its trust anchor.

    raise NotImplementedError("OEM payload signing is not implemented")

def create_authenticated_fmp_payload(args, unsigned_fmp_payload_path, signature_path):
    build_artifacts_dir = os.path.dirname(unsigned_fmp_payload_path)

    with open(unsigned_fmp_payload_path, "rb") as unsigned_fmp_payload_file:
        unsigned_fmp_payload_data = unsigned_fmp_payload_file.read()
    with open(signature_path, "rb") as signature_file:
        signature_data = signature_file.read()

    # The final eight bytes are the monotonic count, not part of the encoded FMP
    # payload header reconstructed for the authentication wrapper.
    fmp_payload = FmpPayloadHeaderClass()
    fmp_payload.Decode(unsigned_fmp_payload_data[:-8])

    fmp_auth = FmpAuthHeaderClass()
    fmp_auth.MonotonicCount = args.monotonic_count
    fmp_auth.FmpPayloadHeader = fmp_payload
    fmp_auth.AuthInfo.cert_data = signature_data

    authenticated_fmp_payload_path = os.path.join(build_artifacts_dir, "authenticated_fmp_payload.bin")
    with open(authenticated_fmp_payload_path, "wb") as authenticated_fmp_payload_file:
        authenticated_fmp_payload_file.write(fmp_auth.Encode())
    return authenticated_fmp_payload_path

def create_fmp_capsule_image(args, authenticated_fmp_payload_path):
    build_artifacts_dir = os.path.dirname(authenticated_fmp_payload_path)
    fmp_capsule_image_path = os.path.join(build_artifacts_dir, "fmp_capsule_image.bin")
    payload_size = os.path.getsize(authenticated_fmp_payload_path)

    fmp_image_header = pack_image_header_v3(SRE_ESRT_GUID, 1, payload_size, CAPSULE_SUPPORT_AUTHENTICATION)
    with open(fmp_capsule_image_path, "wb") as fmp_capsule_image_file, \
         open(authenticated_fmp_payload_path, "rb") as authenticated_fmp_payload_file:
        fmp_capsule_image_file.write(fmp_image_header)
        for chunk in iter(lambda: authenticated_fmp_payload_file.read(4096), b""):
            fmp_capsule_image_file.write(chunk)

    return fmp_capsule_image_path

def create_inf(args, build_dir):
    version = args.capsule_version
    version_string = capsule_helper.get_normalized_version_string(
        "{0}.{1}.{2}".format((version >> 24) & 0xFF, (version >> 8) & 0xFFFF, version & 0xFF)
    )
    inf_file = inf_generator2.InfFile(
        "SreRecovery",                                # Name
        version_string,                               # VersionStr
        datetime.date.today().strftime("%m/%d/%Y"),   # CreationDate
        "ODP (Open Device Partnership)",              # Provider
        "ODP (Open Device Partnership)",              # ManufacturerName
        "amd64",                                      # Arch
    )
    inf_file.AddFirmware(
        "Firmware",                     # Tag
        "Secure Recovery Environment",  # Description
        str(SRE_ESRT_GUID),             # EsrtGuid
        str(version),                   # VersionInt
        "SreRecovery.cap",              # FirmwareFile
    )
    inf_path = os.path.join(build_dir, "SreRecovery.inf")
    with open(inf_path, "w") as f:
        f.write(str(inf_file))
    return inf_path

def sign_catalog(args, catalog_path):

    # TODO: The OEM must implement its Windows catalog-signing process. The
    # implementation may use args for OEM-specific configuration and must return
    # the path to an Authenticode-signed .cat (same path in place, or a new path;
    # create_cat() replaces catalog_path with whichever path is returned).

    raise NotImplementedError("OEM catalog signing is not implemented")

def create_cat(args, build_dir):
    # Generate the unsigned catalog from the .inf + .cap using the WDK's Inf2Cat (via edk2toolext)
    cat_path = capsule_helper.create_cat_file({"fw_name": "SreRecovery", "arch": "amd64"}, build_dir)
    signed_cat_path = sign_catalog(args, cat_path)
    if signed_cat_path != cat_path:
        os.replace(signed_cat_path, cat_path)
    return cat_path

def main():
    args = parse_args()
    script_dir = os.path.dirname(os.path.abspath(__file__))
    build_dir = os.path.join(script_dir, "Build", "SreCapsule")
    build_artifacts_dir = os.path.join(build_dir, "Artifacts")
    os.makedirs(build_artifacts_dir, exist_ok=True)

    # Step 1: Add an FMP payload header and monotonic count to the partition image.
    unsigned_fmp_payload_path = create_unsigned_fmp_payload(args, build_artifacts_dir)

    # Step 2: Sign the complete unsigned FMP payload using the OEM implementation.
    signature_path = sign_payload(args, unsigned_fmp_payload_path)

    # Step 3: Add the FMP authentication header containing the OEM signature.
    authenticated_fmp_payload_path = create_authenticated_fmp_payload(
        args,
        unsigned_fmp_payload_path,
        signature_path,
    )

    # Step 4: Add the FMP image header used to identify this payload to the FMP driver.
    fmp_capsule_image_path = create_fmp_capsule_image(args, authenticated_fmp_payload_path)
    fmp_capsule_image_size = os.path.getsize(fmp_capsule_image_path)

    # Step 5: Add the FMP capsule header around the image.
    fmp_capsule_header_format = "<IHHQ"                                  # Format: U32, U16, U16, U64
    fmp_capsule_header_size = struct.calcsize(fmp_capsule_header_format)
    fmp_capsule_header = struct.pack(fmp_capsule_header_format,
        1,                                         # (UINT32) Version
        0,                                         # (UINT16) Embedded Driver Count
        1,                                         # (UINT16) Payload Count
        fmp_capsule_header_size,                   # (UINT64) Offset to payload[0]
    )
    fmp_capsule_size = len(fmp_capsule_header) + fmp_capsule_image_size

    # Step 6: Add the outer UEFI capsule header.
    uefi_capsule_header_format = "<16sIII4B"                       # Format: GUID, U32, U32, U32, 4 padding bytes
    uefi_capsule_header_size = struct.calcsize(uefi_capsule_header_format)
    uefi_capsule_header = struct.pack(uefi_capsule_header_format,
        uuid.UUID("6dcbd5ed-e82d-4c44-bda1-7194199ad92a").bytes_le,  # (EFI_GUID) CapsuleGuid = EFI_FMP_CAPSULE_ID_GUID
        uefi_capsule_header_size,                       # (UINT32) HeaderSize = 32
        0x00010000 | 0x00040000,                        # (UINT32) Flags = PERSIST_ACROSS_RESET | INITIATE_RESET
        uefi_capsule_header_size + fmp_capsule_size,    # (UINT32) CapsuleImageSize = size of entire capsule
        0, 0, 0, 0)                                     # 4B -> pad to 32-byte HeaderSize

    # Write the complete capsule in outermost-to-innermost header order.
    with open(os.path.join(build_dir, "SreRecovery.cap"), "wb") as out:
        out.write(uefi_capsule_header)                  # UEFI capsule header
        out.write(fmp_capsule_header)                   # FMP capsule header
        with open(fmp_capsule_image_path, "rb") as f:   # FMP image containing the authenticated partition image
            for chunk in iter(lambda: f.read(4096), b""):
                out.write(chunk)
    print("Capsule created")

    # Step 7: Create the Windows Update INF and signed catalog.
    create_inf(args, build_dir)
    print("INF created")

    # Signed .cat catalog so Windows accepts the driver package
    create_cat(args, build_dir)
    print("CAT created")

if __name__ == "__main__":
    main()
