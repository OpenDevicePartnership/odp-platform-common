"""Peeler/FMP layout simulation -- mirrors the C SreCapsulePersistenceLib peeler
and SreNvmeFmpDeviceLib descriptor read against a real built capsule.

Validates the on-disk contract the firmware depends on:
  * SRE capsule identification (FMP GUID, 2 payloads, staging-GUID payload)
  * Version-3 (48-byte) image headers
  * WIM staging from image-header+sizeof(IMAGE_HEADER) round-trips byte-for-byte
  * descriptor parse: skip auth header (when ImageCapsuleSupport AUTH set) AND
    the FMP_PAYLOAD_HEADER ('MSS1'), then read SRE_WIM_DESCRIPTOR
  * descriptor binds the WIM by WimSize + WimHash (SHA-256)
"""

import hashlib
import struct
import sys
import uuid

FMP_CAPSULE_GUID = uuid.UUID("6dcbd5ed-e82d-4c44-bda1-7194199ad92a")
STAGING_GUID = uuid.UUID("d31b249c-bc66-4cdc-be21-30b1b6566b9c")
SRE_WIM_IMAGE_SIG = struct.unpack("<I", b"SWim")[0]
FMP_PAYLOAD_HEADER_SIG = b"MSS1"
IMG_HDR_SIZE_V3 = 48  # sizeof(EFI_FIRMWARE_MANAGEMENT_CAPSULE_IMAGE_HEADER), Version 3
CAPSULE_SUPPORT_AUTHENTICATION = 0x1


def fail(msg):
    print("FAIL:", msg)
    sys.exit(1)


def main():
    cap = open(sys.argv[1], "rb").read()
    wim = open(sys.argv[2], "rb").read()

    # EFI_CAPSULE_HEADER
    if uuid.UUID(bytes_le=cap[0:16]) != FMP_CAPSULE_GUID:
        fail("capsule GUID is not the FMP capsule GUID")
    header_size, flags, image_size = struct.unpack_from("<III", cap, 16)
    if image_size != len(cap):
        fail("CapsuleImageSize {0} != buffer {1}".format(image_size, len(cap)))

    # EFI_FIRMWARE_MANAGEMENT_CAPSULE_HEADER
    fmp = header_size
    ver, embedded, payloads = struct.unpack_from("<IHH", cap, fmp)
    if ver != 1 or embedded != 0 or payloads != 2:
        fail("unexpected FMP header v{0} embedded={1} payloads={2}".format(ver, embedded, payloads))
    offsets = struct.unpack_from("<2Q", cap, fmp + 8)

    def parse_image(base):
        v = struct.unpack_from("<I", cap, base)[0]
        g = uuid.UUID(bytes_le=cap[base + 4 : base + 20])
        idx = cap[base + 20]
        usize, vsize = struct.unpack_from("<II", cap, base + 24)
        support = struct.unpack_from("<Q", cap, base + 40)[0] if v >= 3 else 0
        return v, g, idx, usize, vsize, support

    # Peeler identification: find WIM (staging GUID) and descriptor payloads.
    wim_img = desc_img = None
    for off in offsets:
        base = fmp + off
        info = parse_image(base)
        if info[0] != 3:
            fail("image header at FMP+0x{0:x} is Version {1}, expected 3".format(off, info[0]))
        if info[1] == STAGING_GUID:
            wim_img = (base, info)
        else:
            desc_img = (base, info)
    if wim_img is None or desc_img is None:
        fail("could not locate both WIM and descriptor payloads")

    # --- WIM staging (peeler SreWriteStagedWim source = (WimImage + 1)) ---
    wbase, (_, _, _, wsize, _, wsupport) = wim_img
    if wsupport & CAPSULE_SUPPORT_AUTHENTICATION:
        fail("WIM payload unexpectedly marked AUTH")
    staged = cap[wbase + IMG_HDR_SIZE_V3 : wbase + IMG_HDR_SIZE_V3 + wsize]
    if staged != wim:
        fail("staged WIM bytes do not match the source WIM (offset/size mismatch)")

    # --- descriptor parse (peeler SreGetDescriptor) ---
    dbase, (_, _, _, dsize, dvsize, dsupport) = desc_img
    image = dbase + IMG_HDR_SIZE_V3
    auth_size = 0
    if dsupport & CAPSULE_SUPPORT_AUTHENTICATION:
        dw_length = struct.unpack_from("<I", cap, image + 8)[0]  # WIN_CERTIFICATE.dwLength
        auth_size = 8 + dw_length  # sizeof(MonotonicCount) + dwLength
    cur = image + auth_size
    # Skip optional FMP_PAYLOAD_HEADER (Fix F2).
    if cap[cur : cur + 4] == FMP_PAYLOAD_HEADER_SIG:
        fph_size = struct.unpack_from("<I", cap, cur + 4)[0]
        cur += fph_size
    sig, sver, wver, wsz = struct.unpack_from("<IIIQ", cap, cur)
    whash = cap[cur + 20 : cur + 20 + 32]
    if sig != SRE_WIM_IMAGE_SIG:
        fail("descriptor signature mismatch (peeler would reject)")
    if sver != 1:
        fail("descriptor StructVersion {0} != 1".format(sver))
    if wsz != len(wim):
        fail("descriptor WimSize {0} != actual WIM size {1}".format(wsz, len(wim)))
    if whash != hashlib.sha256(wim).digest():
        fail("descriptor WimHash does not match SHA-256(WIM)")

    print("PEELER/FMP SIM: PASS")
    print("  identification : FMP GUID + 2 payloads + staging GUID OK")
    print("  image headers  : Version 3 (48-byte) OK")
    print("  WIM staging    : {0} bytes round-trip byte-for-byte OK".format(wsize))
    print("  descriptor     : SWim sig, v1, WimSize + WimHash bind OK")
    print("  payload[0] size: {0} bytes (auth+MSS1+descriptor)".format(dsize))


if __name__ == "__main__":
    main()
