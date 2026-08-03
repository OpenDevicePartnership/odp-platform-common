#!/usr/bin/env python3
#
# Build + sign the single-payload SRE recovery capsule
#
import argparse
import datetime
from itertools import product
import os
import struct
import subprocess
import uuid
import zipfile

from edk2toollib.uefi.edk2.fmp_payload_header import FmpPayloadHeaderClass
from edk2toollib.uefi.fmp_auth_header import FmpAuthHeaderClass
from edk2toollib.windows.capsule import inf_generator2
from edk2toolext.capsule import capsule_helper

# EFI_FMP image header v3 ImageCapsuleSupport bit
CAPSULE_SUPPORT_AUTHENTICATION = 0x0000000000000001

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
    p = argparse.ArgumentParser()
    p.add_argument("--wim-path")                                   # input WIM path
    p.add_argument("--signer-path")                                # AutosignConsole.exe path
    p.add_argument("--capsule-version", type=lambda v: int(v, 0))  # FMP / ESRT / capsule version
    p.add_argument("--lsv", type=lambda v: int(v, 0))              # lowest supported version
    p.add_argument("--monotonic-count", type=int)                  # anti-rollback counter
    p.add_argument("--esrt-guid", type=uuid.UUID)                  # FMP payload type (ESRT)
    return p.parse_args()

def create_payload(args, build_artifacts_dir):

    # Wrap the raw WIM in an FMP payload header
    with open(args.wim_path, "rb") as f:
        wim_data = f.read()

    fmp_payload = FmpPayloadHeaderClass()
    fmp_payload.FwVersion = args.capsule_version
    fmp_payload.LowestSupportedVersion = args.lsv
    fmp_payload.Payload = wim_data
    payload_data = fmp_payload.Encode() + struct.pack("<Q", args.monotonic_count)

    # Drop the unsigned payload to disk and return its path
    payload_path = os.path.join(build_artifacts_dir, "payload.bin")
    with open(payload_path, "wb") as f:
        f.write(payload_data)
    return payload_path

def sign_payload(args, payload_path):

    build_artifacts_dir = os.path.dirname(payload_path)
    signed_path = os.path.join(build_artifacts_dir, "payload.signed.bin")

    result_file = os.path.join(build_artifacts_dir, "ResultFile.txt")
    with open(result_file, "w") as f:
        f.write("SRE capsule signing request.")

    signer_args = [
        args.signer_path, "/s",                 # submit and wait for approval
        "-s", r"fareast\frxu",                  # Autosigner service account
        "-t", "DO_NOT_TRUST_TEST_900_MAA",      # selects the DEV/test signing key
        "-p", "FUTURE PRODUCT",                 # Ambiguous product name
        "-b", "SRE capsule FW (leaf) signing",  # Justification for the signing request
        "-r", result_file,                      # Where results are stored
        "-f", payload_path,                     # Input payload to sign
        "-v", "0.0.0.1",                        # Signing request version label only
        "-d", "Dev Test SRE Image",             # Description of the firmware being signed
        "-path", signed_path,                   # Output path
    ]

    print("Waiting for signing process...")
    if subprocess.run(signer_args).returncode != 0:
        raise RuntimeError("signer.exe failed")

    return signed_path

def create_signed_payload(args, signed_path):
    build_artifacts_dir = os.path.dirname(signed_path)

    with open(os.path.join(build_artifacts_dir, "payload.bin"), "rb") as f:
        payload_data = f.read()
    with open(signed_path, "rb") as f:
        signed_payload_data = f.read()

    # Rebuild the payload header (needed by the auth wrapper) from the signed data
    fmp_payload = FmpPayloadHeaderClass()
    fmp_payload.Decode(payload_data[:-8])

    fmp_auth = FmpAuthHeaderClass()
    fmp_auth.MonotonicCount = args.monotonic_count
    fmp_auth.FmpPayloadHeader = fmp_payload
    fmp_auth.AuthInfo.cert_data = signed_payload_data

    # Drop the signed FMP payload to disk and return its path
    payload_path = os.path.join(build_artifacts_dir, "payload.payload.bin")
    with open(payload_path, "wb") as f:
        f.write(fmp_auth.Encode())
    return payload_path

def create_image(args, payload_path):
    build_artifacts_dir = os.path.dirname(payload_path)
    image_path = os.path.join(build_artifacts_dir, "image_0.bin")
    payload_size = os.path.getsize(payload_path)

    image0_header = pack_image_header_v3(args.esrt_guid, 1, payload_size, CAPSULE_SUPPORT_AUTHENTICATION)
    with open(image_path, "wb") as out, open(payload_path, "rb") as f:
        out.write(image0_header)
        for chunk in iter(lambda: f.read(4096), b""):
            out.write(chunk)

    return image_path

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
        str(args.esrt_guid),            # EsrtGuid
        str(version),                   # VersionInt
        "SreRecovery.cap",              # FirmwareFile
    )
    inf_path = os.path.join(build_dir, "SreRecovery.inf")
    with open(inf_path, "w") as f:
        f.write(str(inf_file))
    return inf_path

def create_cat(args, build_dir):
    # Generate the unsigned catalog from the .inf + .cap using the WDK's Inf2Cat (via edk2toolext)
    cat_path = capsule_helper.create_cat_file({"fw_name": "SreRecovery", "arch": "amd64"}, build_dir)

    # Sign the catalog (Authenticode) with the OS driver signing key
    signed_cat_path = os.path.join(build_dir, "SreRecovery.signed.cat")
    result_file = os.path.join(build_dir, "CatResultFile.txt")
    with open(result_file, "w") as f:
        f.write("SRE catalog signing request.")

    signer_args = [
        args.signer_path, "/s",                          # submit and wait for approval
        "-s", r"fareast\frxu",                           # Autosigner service account
        "-t", "DO_NOT_TRUST_TEST_ESRP_OS_DRIVER_SIGNER", # OS (catalog) signing key
        "-p", "FUTURE PRODUCT",                          # Ambiguous product name
        "-b", "SRE capsule catalog (OS) signing",        # Justification for the signing request
        "-r", result_file,                               # Where results are stored
        "-f", cat_path,                                  # Input catalog to sign
        "-v", "0.0.0.1",                                 # Signing request version label only
        "-d", "Dev Test SRE Catalog",                    # Description of the catalog being signed
        "-path", signed_cat_path,                        # Output path
    ]

    print("Waiting for catalog signing process...")
    if subprocess.run(signer_args).returncode != 0:
        raise RuntimeError("catalog signer.exe failed")

    # AutosignConsole wraps OS-signed output in a ZIP; extract the actual .cat
    if zipfile.is_zipfile(signed_cat_path):
        with zipfile.ZipFile(signed_cat_path, "r") as zf:
            cat_name = next(n for n in zf.namelist() if n.endswith(".cat"))
            zf.extract(cat_name, build_dir)
            extracted = os.path.join(build_dir, cat_name)
            if extracted != cat_path:
                os.replace(extracted, cat_path)
        os.remove(signed_cat_path)
    else:
        # Replace the unsigned catalog so the INF's CatalogFile=SreRecovery.cat matches the signed file
        os.replace(signed_cat_path, cat_path)
    return cat_path

def main():
    args = parse_args()
    repo_root = os.path.dirname(os.path.abspath(__file__))
    build_dir = os.path.join(repo_root, "Build", "SreCapsule")
    build_artifacts_dir = os.path.join(build_dir, "Artifacts")
    os.makedirs(build_artifacts_dir, exist_ok=True)

    # Image[0], signed WIM payload
    payload_path = create_payload(args, build_artifacts_dir)
    signed_payload_path = sign_payload(args, payload_path)
    signed_image_payload_path = create_signed_payload(args, signed_payload_path)
    image_path = create_image(args, signed_image_payload_path)
    image_size = os.path.getsize(image_path)

    # EFI_FIRMWARE_MANAGEMENT_CAPSULE_HEADER - Wraps images indicating they are for FW Management drivers
    fmt_fwmgmt_capsule_header = "<IHHQ"                             # Format of the structure (U32, U16, U16, U64)
    fwmgmt_header_size = struct.calcsize(fmt_fwmgmt_capsule_header) # Size of the structure
    efi_fwm_capsule_header = struct.pack(fmt_fwmgmt_capsule_header,
        1,                                         # (UINT32) Version
        0,                                         # (UINT16) Embedded Driver Count
        1,                                         # (UINT16) Payload Count
        fwmgmt_header_size,                        # (UINT64) Offset to payload[0]
    )
    efi_fwm_capsule_size = len(efi_fwm_capsule_header) + image_size

    # EFI_CAPSULE_HEADER - Overall header for entire capsule
    fmt_capsule_header = "<16sIII4B"                          # Format of the structure (GUID, U32, U32, U32, 4 bytes padding)
    capsule_header_size = struct.calcsize(fmt_capsule_header) # Size of the structure
    efi_header = struct.pack(fmt_capsule_header,
        uuid.UUID("6dcbd5ed-e82d-4c44-bda1-7194199ad92a").bytes_le,  # (EFI_GUID) CapsuleGuid = EFI_FMP_CAPSULE_ID_GUID
        capsule_header_size,                            # (UINT32) HeaderSize = 32
        0x00010000 | 0x00040000,                        # (UINT32) Flags = PERSIST_ACROSS_RESET | INITIATE_RESET
        capsule_header_size + efi_fwm_capsule_size,     # (UINT32) CapsuleImageSize = size of entire capsule
        0, 0, 0, 0)                                     # 4B -> pad to 32-byte HeaderSize

    # Create the new capsule
    with open(os.path.join(build_dir, "SreRecovery.cap"), "wb") as out:
        out.write(efi_header)                           # Capsule header
        out.write(efi_fwm_capsule_header)               # FMP capsule header
        with open(image_path, "rb") as f:               # Image[0] - signed WIM payload
            for chunk in iter(lambda: f.read(4096), b""):
                out.write(chunk)
    print(f"Capsule created")

    # Windows Update .inf that references the capsule by its ESRT GUID
    create_inf(args, build_dir)
    print(f"INF created")

    # Signed .cat catalog so Windows accepts the driver package
    create_cat(args, build_dir)
    print(f"CAT created")

if __name__ == "__main__":
    main()
