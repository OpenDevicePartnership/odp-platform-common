# SRE Recovery Capsule — Design Notes

The Secure Recovery Environment (SRE) ships as a **standard single-payload FMP capsule** which is installed by a
standard single instance FmpDeviceLib.  This note only describes what is novel about this implementation.

## SRE Capsule Payload

The capsule payload is the physical **~1 GiB raw FAT32 boot-partition image** that is placed in memory, converted to
a RAM drive, then mounted from the BDS to launch the SRE image.  This payload binary image is built using the
`Application\BpRecoveryLoader\BuildBpFatImage.ps1` tool located in this directory.  Run everything below from the
`SrePkg` directory; the FAT image build needs an elevated shell with the Hyper-V Management Tools feature enabled:

  ```terminal
  .\Application\BpRecoveryLoader\BuildBpFatImage.ps1 -WimFile .\ValidationOS.wim -OutImage .\ValidationFat32Partition.img -SizeBytes 1GB
  ```

And the capsule is created from that image by the builder in the `capsules` sub-directory by running the
"capsules\SreCapsuleBuilder.cmd" script:

  ```terminal
  .\capsules\SreCapsuleBuilder.cmd
  ```

NOTE: `SreCapsuleBuilder.cmd` resolves its Python script, the `ValidationFat32Partition.img` payload, and the
      `AutosignConsole.exe` signer relative to its own location, so it runs from any current directory.  It expects the
      FAT image next to the WIM in the `SrePkg` directory, and the SRE image WIM named `ValidationOS.wim`.  The capsule
      version, LSV, monotonic count, and ESRT GUID are baked into the .cmd.

## ESRT Versioning

When the capsule is applied, the FMP driver reads the version from the capsule header and creates an SRE_IMAGE_INFO
structure that is used to overwrite unused GPT LBA 34 in the partition image before it is written to the NVME SRE storage
area.  This structure is then used on the next boot to populte the ESRT table with the currently running version
of the image.

```c
typedef struct {
  UINT32  Signature;      // 'SREI' (SRE_IMAGE_INFO_SIG)
  UINT32  StructVersion;  // Structure version (currently 1)
  UINT32  SreFwVersion;   // Version of the image installed to the partition
  UINT32  Reserved[5];    // Reserved for future use (e.g. a 32-byte image hash)
} SRE_IMAGE_INFO;         // stamped at SRE_IMAGE_INFO_OFFSET (0x4400)
```

Implications of this solution are:

- `0x4400` sits in unused MSR space between the GPT entry array (ends at `0x4400`) and the FAT32 volume (starts at
  `0x1000000`), so the descriptor is invisible to the boot path and cannot corrupt GPT or filesystem data. This assumes
  512-byte logical sectors and that the FAT32 partition starts above the descriptor window — both are asserted by
  `BuildBpFatImage.ps1`.
- The offset is intentionally not block-aligned; the driver locates the storage block that spans it and indexes by
  `SRE_IMAGE_INFO_OFFSET % BlockSize`.
- `Signature` and `StructVersion` are stable across future revisions to allow field changes going forward.
- `FmpDeviceGetVersion` reads the descriptor from Partition A and returns `0x00000000` if it is not present/valid.

Next iteration idea is to directly write the .wim into the capsule and NVME partition along with a small Win loader
that can directly launch a WIM.  This would compress the capsule dramatically since WIMs are compressed, and allow
the IMAGE_INFO structure to be a trailer at the end of the partition and not rely on a repurposed region of a partition.
