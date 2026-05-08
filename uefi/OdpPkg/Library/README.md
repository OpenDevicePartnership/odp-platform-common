# OdpPkg / Library

This directory holds the **library module instances** provided by `OdpPkg`.

In a standard EDK II package, `Library/` contains one subdirectory per
*library instance* — a concrete implementation of a library class declared
in the package's `.dec` file. Each instance has its own `.inf` describing
its sources, dependencies, and the library class it produces.

Typical contents:

- One subdirectory per library instance (e.g., `MyFooLibDxe/`,
  `MyFooLibPei/`, `MyFooLibNull/`), each containing:
  - The library's `.inf` (LIBRARY_CLASS, sources, dependencies).
  - One or more `.c` / `.h` source files implementing the class.

The library *classes* themselves (the API contracts) are declared in the
`[LibraryClasses]` section of [`../OdpPkg.dec`](../OdpPkg.dec) and their
public headers live under [`../Include/Library/`](../Include/). Platforms
choose which instance to consume by mapping the class to an `.inf` here in
their DSC's `[LibraryClasses]` section.
