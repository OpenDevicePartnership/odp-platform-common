# @file BuildSreCapsule.py
#
# Builds a UEFI FMP capsule to support SRE (system recovery environment) that contains TWO payloads:
#
#   payload[0] = the SRE_WIM_DESCRIPTOR
#   payload[1] = the recovery .wim image
#
# Only payload[0] (the descriptor) is wrapped in an FMP payload header and signed with an
# EFI_FIRMWARE_IMAGE_AUTHENTICATION (PKCS#7) envelope.  It contains the data hash to authenticate
# payload[1] (the WIM image) which is carried raw (unsigned).
#
# Signer hook contract (identical to a typical platform batch signer):
#
#     signer(payload_list, signing_type) -> (retcode, cert, signature_list)
#
#       payload_list   : list of byte buffers to sign
#       signing_type   : "FW" -> detached PKCS#7 over each payload (capsule leaf signing,
#                                 chained to the firmware's trusted FMP root)
#                        "OS" -> Authenticode-signed catalog bytes for each input .cat file
#       returns        : (retcode, cert_or_None, signature_list) where signature_list is ordered
#                        to match payload_list; retcode 0 == success
#
# Library use (host build passes its own signer, e.g. PlatformSignHelper):
#
#   import BuildSreCapsule
#   BuildSreCapsule.build_sre_capsule(
#       wim_path="D:/images/sre.wim", version=0x00010000, out_dir="D:/out",
#       signer=my_platform_signer, esrt_guid="9777ff5d-...", emit_cat=True)
#
# Standalone use (self-contained built-in signer, for local layout/round-trip testing):
#
#   python BuildSreCapsule.py \
#       --wim D:\images\sre.wim \
#       --version 0x00010000 \
#       --builtin-signer signtool \
#       -ds key_file="C:\keys\fw_signer.pfx" \
#       -ds key_pass="<password>" \
#       D:\out
#
# The final positional argument is an output directory; the tool writes SreRecovery.cap (the capsule)
# and SreRecovery.inf (and, with --emit-cat + a platform signer, SreRecovery.cat) into it.
#
# Both payloads are tightly coupled, so the input version is used for the version of the capsule, the
# ESRT entry, and the FMP image.
#
# Copyright (c) Microsoft Corporation
#
# SPDX-License-Identifier: BSD-2-Clause-Patent
##

import argparse
import datetime
import glob
import hashlib
import os
import shutil
import struct
import subprocess
import sys
import tempfile
import uuid
from typing import Callable, List, Optional, Sequence, Tuple

from edk2toolext.capsule import capsule_helper, signing_helper
from edk2toollib.uefi.edk2.fmp_payload_header import FmpPayloadHeaderClass
from edk2toollib.uefi.fmp_auth_header import FmpAuthHeaderClass
from edk2toollib.windows.capsule import inf_generator2

# Signer hook: (payload_list, signing_type) -> (retcode, cert, signature_list)
Signer = Callable[[Sequence[bytes], str], Tuple[int, Optional[bytes], Optional[List[bytes]]]]

# PKCS#7 SignedData OID (see capsule_helper).
PKCS7_SIGNED_DATA_OID = "1.2.840.113549.1.7.2"

# Fixed anti-rollback counter used for the descriptor's signature envelope.
MONOTONIC_COUNT = 1

# UpdateImageTypeId carried by the raw WIM payload (payload[1]).  Must match the firmware
# peeler's staging GUID.  No FmpDxe instance claims this GUID.
SRE_WIM_STAGING_GUID = uuid.UUID("d31b249c-bc66-4cdc-be21-30b1b6566b9c")

# Defaults for the standalone CLI / library callers.
DEFAULT_ESRT_GUID = "9777ff5d-3c53-4316-9af3-b99fb9c60fb5"
DEFAULT_PROVIDER = "ODP (Open Device Partnership)"
DEFAULT_ARCH = "amd64"
DEFAULT_FW_NAME = "SreRecovery"
DEFAULT_FW_DESCRIPTION = "Secure Recovery Environment"


def build_descriptor(version_int: int, wim_size: int, wim_hash: bytes) -> bytes:
    """Pack the SRE_WIM_DESCRIPTOR v1 (52 bytes, #pragma pack(1)).

    struct layout:
        UINT32 Signature       ('SWim')
        UINT32 StructVersion   (1)
        UINT32 WimVersion
        UINT64 WimSize
        UINT8  WimHash[32]     (SHA-256)
    """
    return struct.pack(
        "<IIIQ32s",
        struct.unpack("<I", b"SWim")[0],  # SIGNATURE_32('S','W','i','m')
        0x00000001,
        version_int,
        wim_size,
        wim_hash,
    )


def build_signed_descriptor_payload(
    descriptor: bytes, version_int: int, lsv_int: int, signer: Signer
) -> bytes:
    """Wrap the descriptor in an FMP_PAYLOAD_HEADER and a signed
    EFI_FIRMWARE_IMAGE_AUTHENTICATION envelope, using the injected signer hook.

    The data covered by the signature is exactly (FMP_PAYLOAD_HEADER || MonotonicCount),
    matching the on-device FmpDxe verification contract.
    """
    fmp_payload_header = FmpPayloadHeaderClass()
    fmp_payload_header.FwVersion = version_int
    fmp_payload_header.LowestSupportedVersion = lsv_int
    fmp_payload_header.Payload = descriptor

    data_to_sign = fmp_payload_header.Encode() + struct.pack("<Q", MONOTONIC_COUNT)

    retcode, _cert, signatures = signer([data_to_sign], "FW")
    if retcode != 0 or not signatures:
        raise RuntimeError("FW (capsule leaf) signing failed (retcode={0})".format(retcode))

    fmp_auth_header = FmpAuthHeaderClass()
    fmp_auth_header.MonotonicCount = MONOTONIC_COUNT
    fmp_auth_header.FmpPayloadHeader = fmp_payload_header
    fmp_auth_header.AuthInfo.cert_data = signatures[0]

    return fmp_auth_header.Encode()


def build_capsule_bytes(
    wim_bytes: bytes, version_int: int, esrt_guid, lsv_int: int, signer: Signer
) -> bytes:
    """Assemble the full two-payload SRE FMP capsule as a byte buffer."""



    # #######################################################################
    # ###  TEST-ONLY HACK -- FORCE DESCRIPTOR VERSION OLDER THAN THE INF  ###
    # #######################################################################
    descriptor_version = version_int and 0xFFFF0000
    if descriptor_version == version_int:
        raise RuntimeError("descriptor_version not older than version_int")
    # #######################################################################



    wim_hash = hashlib.sha256(wim_bytes).digest()
    descriptor = build_descriptor(descriptor_version, len(wim_bytes), wim_hash)
    descriptor_payload = build_signed_descriptor_payload(descriptor, version_int, lsv_int, signer)

    esrt_uuid = esrt_guid if isinstance(esrt_guid, uuid.UUID) else uuid.UUID(str(esrt_guid))
    return assemble_capsule(
        descriptor_payload=descriptor_payload,
        descriptor_type=esrt_uuid,
        descriptor_index=1,
        wim_payload=wim_bytes,
        wim_type=SRE_WIM_STAGING_GUID,
    )


def write_inf(
    out_dir: str,
    version_int: int,
    esrt_guid,
    provider: str = DEFAULT_PROVIDER,
    capsule_name: str = DEFAULT_FW_NAME + ".cap",
    fw_name: str = DEFAULT_FW_NAME,
    description: str = DEFAULT_FW_DESCRIPTION,
    arch: str = DEFAULT_ARCH,
) -> str:
    """Write the Windows Update .inf that references the capsule by its ESRT GUID."""
    version_string = capsule_helper.get_normalized_version_string(
        "{0}.{1}.{2}".format((version_int >> 24) & 0xFF, (version_int >> 8) & 0xFFFF, version_int & 0xFF)
    )
    inf_file = inf_generator2.InfFile(
        fw_name,
        version_string,
        datetime.date.today().strftime("%m/%d/%Y"),
        provider,  # Provider
        provider,  # Manufacturer
        arch,      # Architecture
        TargetOsVersion=None,
    )
    inf_file.AddFirmware(
        "Firmware",
        description,
        str(esrt_guid),
        str(version_int),
        capsule_name,
    )
    inf_path = os.path.join(out_dir, fw_name + ".inf")
    with open(inf_path, "w") as handle:
        handle.write(str(inf_file))
    return inf_path


def write_cat(
    out_dir: str,
    signer: Signer,
    fw_name: str = DEFAULT_FW_NAME,
    arch: str = DEFAULT_ARCH,
    os_string: Optional[str] = None,
) -> str:
    """Generate the driver catalog (.cat) with the WDK's Inf2Cat and sign it via the 'OS' hook.

    Requires a .inf already present in out_dir and the WDK's Inf2Cat.exe on the machine.
    The signer must implement the 'OS' signing type. Both a platform-provided signer and
    the built-in CLI 'signtool' signer (with a PFX) satisfy this.
    """
    cat_options = {"fw_name": fw_name, "arch": arch}
    if os_string is not None:
        cat_options["os_string"] = os_string

    cat_path = capsule_helper.create_cat_file(cat_options, out_dir)  # unsigned, via Inf2Cat
    with open(cat_path, "rb") as fp:
        cat_bytes = fp.read()

    retcode, _cert, signatures = signer([cat_bytes], "OS")
    if retcode != 0 or not signatures:
        raise RuntimeError("OS (catalog) signing failed (retcode={0})".format(retcode))

    with open(cat_path, "wb") as fp:
        fp.write(signatures[0])
    return cat_path


def build_sre_capsule(
    wim_path: str,
    version,
    out_dir: str,
    signer: Signer,
    esrt_guid: str = DEFAULT_ESRT_GUID,
    lsv: int = 0,
    provider: str = DEFAULT_PROVIDER,
    emit_cat: bool = False,
    arch: str = DEFAULT_ARCH,
    os_string: Optional[str] = None,
) -> Tuple[str, str, Optional[str]]:
    """Build the SRE capsule + .inf (+ optional signed .cat) into out_dir.

    signer is the caller-provided hook (see module docstring). A host build passes its own
    in-house signer; the standalone CLI passes a built-in signer. Returns
    (capsule_path, inf_path, cat_path_or_None).
    """
    if not os.path.isfile(wim_path):
        raise FileNotFoundError("--wim path not found: {0}".format(wim_path))

    version_int = version if isinstance(version, int) else int(version, 0)
    lsv_int = lsv if isinstance(lsv, int) else int(lsv, 0)

    out_dir = os.path.abspath(out_dir)
    os.makedirs(out_dir, exist_ok=True)

    print("Reading WIM: {0}".format(wim_path))
    with open(wim_path, "rb") as wim_file:
        wim_bytes = wim_file.read()
    print("  size  = {0} bytes".format(len(wim_bytes)))
    print("  sha256= {0}".format(hashlib.sha256(wim_bytes).hexdigest()))

    print("Signing payload[0] (descriptor) via injected signer hook ...")
    capsule_bytes = build_capsule_bytes(wim_bytes, version_int, esrt_guid, lsv_int, signer)

    capsule_name = DEFAULT_FW_NAME + ".cap"
    capsule_path = os.path.join(out_dir, capsule_name)
    with open(capsule_path, "wb") as out_file:
        out_file.write(capsule_bytes)
    print("Wrote capsule ({0} bytes) -> {1}".format(len(capsule_bytes), capsule_path))

    inf_path = write_inf(out_dir, version_int, esrt_guid, provider, capsule_name=capsule_name, arch=arch)
    print("Wrote INF -> {0}".format(inf_path))

    cat_path = None
    if emit_cat:
        cat_path = write_cat(out_dir, signer, arch=arch, os_string=os_string)
        print("Wrote signed CAT -> {0}".format(cat_path))

    return capsule_path, inf_path, cat_path


def _find_signtool() -> str:
    """Locate signtool.exe (PATH first, then the Windows SDK/WDK bin folders)."""
    found = shutil.which("signtool")
    if found:
        return found

    candidates: List[str] = []
    for program_files in (os.getenv("ProgramFiles(x86)"), os.getenv("ProgramFiles")):
        if not program_files:
            continue
        kits_bin = os.path.join(program_files, "Windows Kits", "10", "bin")
        candidates.extend(glob.glob(os.path.join(kits_bin, "*", "x64", "signtool.exe")))
        candidates.extend(glob.glob(os.path.join(kits_bin, "x64", "signtool.exe")))

    if not candidates:
        raise FileNotFoundError(
            "signtool.exe not found. Install the Windows SDK/WDK or add signtool.exe to PATH."
        )

    # Prefer the highest SDK version (paths sort lexically with the version directory).
    candidates.sort()
    return candidates[-1]


def _signtool_sign_catalog(cat_bytes: bytes, key_file: str, key_pass: Optional[str]) -> bytes:
    """Authenticode-sign catalog bytes in place with signtool and return the signed bytes."""
    signtool = _find_signtool()
    with tempfile.TemporaryDirectory() as tmpdir:
        cat_path = os.path.join(tmpdir, "SreRecovery.cat")
        with open(cat_path, "wb") as handle:
            handle.write(cat_bytes)

        cmd = [signtool, "sign", "/fd", "sha256", "/f", key_file]
        if key_pass:
            cmd += ["/p", key_pass]
        cmd.append(cat_path)

        result = subprocess.run(cmd, capture_output=True, text=True)
        if result.returncode != 0:
            raise RuntimeError(
                "signtool failed to sign the catalog (exit {0}):\n{1}\n{2}".format(
                    result.returncode, result.stdout, result.stderr
                )
            )

        with open(cat_path, "rb") as handle:
            return handle.read()


def default_cli_signer(builtin_signer_name: str, signer_options: dict) -> Signer:
    """Build a standalone signer hook from the open-source built-in signers.

    Conforms to the same signer contract as a platform signer. Implements 'FW' (capsule
    leaf) signing via the selected built-in signer, and 'OS' (driver catalog) signing
    locally via signtool when a PFX is supplied (-ds key_file=<pfx> [-ds key_pass=<pw>]).
    Catalog signing requires the 'signtool' path (a .pfx); the keyless pyopenssl test
    signer cannot Authenticode-sign a catalog.
    """
    signer_module = signing_helper.get_signer(builtin_signer_name)

    def _sign(payload_list: Sequence[bytes], signing_type: str):
        if signing_type == "FW":
            signature_options = {"sign_alg": "pkcs12", "hash_alg": "sha256"}
            signatures = []
            for payload in payload_list:
                options = dict(signer_options)
                options["oid"] = PKCS7_SIGNED_DATA_OID
                signatures.append(signer_module.sign(payload, signature_options, options))
            return (0, None, signatures)
        if signing_type == "OS":
            key_file = signer_options.get("key_file")
            if not key_file:
                raise NotImplementedError(
                    "catalog ('OS') signing needs a PFX; pass '--builtin-signer signtool "
                    "-ds key_file=<pfx> -ds key_pass=<pw>'. The keyless pyopenssl test "
                    "signer cannot Authenticode-sign a catalog."
                )
            key_pass = signer_options.get("key_pass")
            signatures = [
                _signtool_sign_catalog(payload, key_file, key_pass) for payload in payload_list
            ]
            return (0, None, signatures)
        raise NotImplementedError(
            "the built-in CLI signer supports only 'FW' and 'OS' signing types"
        )

    return _sign


def get_cli_options(args=None) -> argparse.Namespace:
    
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )

    parser.add_argument(
        "--wim",
        required=True,
        help="filesystem path to the recovery .wim used as payload[1]"
    )
    parser.add_argument(
        "--version",
        required=True,
        type=lambda v: int(v, 0),
        help="Firmware version (hex, e.g. 0x00010000)",
    )
    parser.add_argument(
        "--esrt_guid",
        default="9777ff5d-3c53-4316-9af3-b99fb9c60fb5",
        help="Capsule ESRT GUID",
    )
    parser.add_argument(
        "--lsv_version",
        default=0,
        type=lambda v: int(v, 0),
        help="Lowest supported version (hex or decimal, default: 0).",
    )
    parser.add_argument(
        "--builtin-signer",
        default=signing_helper.PYOPENSSL_SIGNER,
        choices=[signing_helper.PYOPENSSL_SIGNER, signing_helper.SIGNTOOL_SIGNER],
        help="Signer tool to use ('signtool' for PKCS#7, defaults to test signatures with 'pyopenssl')",
    )
    parser.add_argument(
        "-ds",
        action="append",
        dest="signer_options",
        default=[],
        help="signer option in key=value form (e.g. key_file=...). Repeatable.",
    )
    parser.add_argument(
        "--provider",
        default=DEFAULT_PROVIDER,
        help="INF provider/manufacturer name (default: %(default)s).",
    )
    parser.add_argument(
        "--emit-cat",
        action="store_true",
        help="also generate a signed driver catalog (.cat). Requires the WDK Inf2Cat.exe. "
             "With the built-in CLI, catalog signing uses signtool and needs a PFX "
             "('--builtin-signer signtool -ds key_file=<pfx> -ds key_pass=<pw>').",
    )
    parser.add_argument(
        "output", help="filesystem directory for output files"
    )

    return parser.parse_args(args=args)


def main() -> int:
    args = get_cli_options()

    # Parse the signer's 'key=value' options.
    signer_options = {}
    for item in args.signer_options:
        if "=" not in item:
            raise ValueError("signer option '{0}' must be in key=value form".format(item))
        key, value = item.split("=", 1)
        signer_options[key] = value

    # Both signers require a key. For the pyopenssl test signer, auto-generate an
    # ephemeral self-signed key so keyless builds produce a test-signed capsule.
    # The real signtool signer still requires a caller-supplied certificate.
    if "key_file" not in signer_options and "key_data" not in signer_options:
        if args.builtin_signer == signing_helper.PYOPENSSL_SIGNER:
            print("No signing key provided; generating an ephemeral test key (pyopenssl) ...")
            signer_options["key_data"] = _make_test_pkcs12()
            signer_options["key_file_format"] = "pkcs12"
        else:
            raise ValueError(
                "no signing key provided for the '{0}' signer; pass '-ds key_file=<path>'".format(
                    args.builtin_signer
                )
            )

    signer = default_cli_signer(args.builtin_signer, signer_options)

    build_sre_capsule(
        wim_path=args.wim,
        version=args.version,
        out_dir=args.output,
        signer=signer,
        esrt_guid=args.esrt_guid,
        lsv=args.lsv_version,
        provider=args.provider,
        emit_cat=args.emit_cat,
    )

    return 0


def _make_test_pkcs12() -> bytes:
    """Generate an ephemeral self-signed PKCS#12 for test signing (pyopenssl only).

    Used when no key is supplied so a keyless build still produces a test-signed
    capsule. This is NOT a trusted certificate and must never be used for release.
    """
    from cryptography import x509
    from cryptography.hazmat.primitives import hashes, serialization
    from cryptography.hazmat.primitives.asymmetric import rsa
    from cryptography.hazmat.primitives.serialization import pkcs12
    from cryptography.x509.oid import NameOID

    key = rsa.generate_private_key(public_exponent=65537, key_size=2048)
    name = x509.Name([x509.NameAttribute(NameOID.COMMON_NAME, "SRE Test Signer")])
    now = datetime.datetime.now(datetime.timezone.utc)
    cert = (
        x509.CertificateBuilder()
        .subject_name(name)
        .issuer_name(name)
        .public_key(key.public_key())
        .serial_number(x509.random_serial_number())
        .not_valid_before(now - datetime.timedelta(days=1))
        .not_valid_after(now + datetime.timedelta(days=3650))
        .sign(key, hashes.SHA256())
    )
    return pkcs12.serialize_key_and_certificates(
        name=b"sretest",
        key=key,
        cert=cert,
        cas=None,
        encryption_algorithm=serialization.NoEncryption(),
    )


#########################################################################################
# The edk2-pytool-library does not support the image header version 3, so the following
# functions are used to create the necessary headers manually.
#

def _pack_image_header_v3(
    type_guid: uuid.UUID, image_index: int, payload_len: int, capsule_support: int
) -> bytes:
    """Pack a 48-byte Version-3 EFI_FIRMWARE_MANAGEMENT_CAPSULE_IMAGE_HEADER."""
    return struct.pack(
        "<I16sB3xIIQQ",
        3,                      # Version
        type_guid.bytes_le,     # UpdateImageTypeId
        image_index & 0xFF,     # UpdateImageIndex
        payload_len,            # UpdateImageSize
        0,                      # UpdateVendorCodeSize
        0,                      # UpdateHardwareInstance
        capsule_support,        # ImageCapsuleSupport
    )

CAPSULE_SUPPORT_AUTHENTICATION = 0x0000000000000001
CAPSULE_FLAGS_PERSIST_ACROSS_RESET = 0x00010000
CAPSULE_FLAGS_INITIATE_RESET = 0x00040000

def assemble_capsule(
    descriptor_payload: bytes,
    descriptor_type: uuid.UUID,
    descriptor_index: int,
    wim_payload: bytes,
    wim_type: uuid.UUID,
) -> bytes:

    image0 = _pack_image_header_v3(descriptor_type, descriptor_index, len(descriptor_payload), CAPSULE_SUPPORT_AUTHENTICATION) + descriptor_payload
    image1 = _pack_image_header_v3(wim_type, 1, len(wim_payload), 0) + wim_payload

    # EFI_FIRMWARE_MANAGEMENT_CAPSULE_HEADER:
    #     Version (u32),
    #     EmbeddedDriverCount (u16) = 0,
    #     PayloadItemCount (u16) = 2,
    #     UINT64 ItemOffsetList[2] measured from the start of this header.
    fmp_fixed = struct.pack("<IHH", 1, 0, 2)
    fmp_header_size = len(fmp_fixed) + 2 * 8
    offset0 = fmp_header_size
    offset1 = fmp_header_size + len(image0)
    fmp_body = fmp_fixed + struct.pack("<QQ", offset0, offset1) + image0 + image1

    # Padding EFI_CAPSULE_HEADER to keep the FMP body 8-byte aligned
    efi_capsule_header_size = 32
    total_size = efi_capsule_header_size + len(fmp_body)
    flags = CAPSULE_FLAGS_PERSIST_ACROSS_RESET | CAPSULE_FLAGS_INITIATE_RESET
    efi_header = (
        uuid.UUID("6dcbd5ed-e82d-4c44-bda1-7194199ad92a").bytes_le
        + struct.pack("<III", efi_capsule_header_size, flags, total_size)
        + b"\x00\x00\x00\x00"  # pad to HeaderSize (32)
    )
    return efi_header + fmp_body

if __name__ == "__main__":
    sys.exit(main())
