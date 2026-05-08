# OdpPkg / Include

This directory holds the **public C header files** exported by `OdpPkg` to
modules built against the package.

In a standard EDK II package, `Include/` is the conventional location for:

- **`Library/`** — header files that declare *library class* APIs. Modules
  that use a library class via `[LibraryClasses]` in their INF compile against
  the matching header here.
- **`Protocol/`** — UEFI Protocol header definitions and GUIDs.
- **`Ppi/`** — PEI-to-PEI Interface (PPI) header definitions and GUIDs.
- **`Guid/`** — standalone GUID definitions (e.g., HOB, variable, file GUIDs).
- **`IndustryStandard/`** — headers for industry-standard structures
  (ACPI tables, SMBIOS, PCI, USB, etc.) that are not specific to any one
  protocol or library class.

The `Includes` paths exposed by this package are declared in the
`[Includes]` section of [`../OdpPkg.dec`](../OdpPkg.dec); consumers add
`OdpPkg/OdpPkg.dec` to the `[Packages]` section of their INF to gain
access to these headers.
