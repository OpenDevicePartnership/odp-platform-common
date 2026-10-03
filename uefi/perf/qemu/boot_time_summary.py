"""Prints ms-resolution boot milestones from a parsed FBPT XML report.

The capture harness is expected to report a boot time for each QEMU
invocation, so every run prints a short summary instead of leaving the numbers
to manual post-processing of the generated reports.

Only the ACPI-defined basic boot performance record is summarized here. The
parser also emits per-phase records, but their reported millisecond values are
not all on one time base in this firmware -- PEI end, for example, comes out as
264303577 ms, roughly 73 hours -- so folding them into a summary would report
figures that look authoritative and are not. Per-phase detail remains available
in the generated text and XML reports for anyone who needs it.

License
-------

Copyright (c) Microsoft Corporation.

SPDX-License-Identifier: MIT
"""

import sys
import xml.etree.ElementTree as ET

BASIC_BOOT_RECORD = "FirmwareBasicBootPerformanceEvent"
MILLISECONDS_ATTRIBUTE = "ValueInMilliseconds"

# Reset is the zero point of the record, and the OS loader milestones bracket
# the handoff out of firmware, so the start of the loader image is the boot
# time this harness reports.
RESET_MILESTONE = "ResetEnd"
OS_LOADER_HANDOFF_MILESTONE = "OSLoaderStartImageStart"

REPORTED_MILESTONES = (
    RESET_MILESTONE,
    "OSLoaderLoadImageStart",
    OS_LOADER_HANDOFF_MILESTONE,
    "ExitBootServicesEntry",
    "ExitBootServicesExit",
)

# Milestones the firmware never reached are left at zero. Reset legitimately
# sits at zero, so it is always reported; the rest are only shown once they
# hold a real measurement.
UNREACHED_MILESTONE_MS = 0.0

EXIT_USAGE = 2
EXIT_NO_DATA = 1


def _milliseconds(element):
    raw = element.get(MILLISECONDS_ATTRIBUTE)
    return None if raw is None else float(raw)


def summarize(xml_path):
    """Prints the boot milestones, returning a process exit code."""
    record = ET.parse(xml_path).getroot().find(f".//{BASIC_BOOT_RECORD}")
    if record is None:
        print(f"no {BASIC_BOOT_RECORD} in {xml_path}", file=sys.stderr)
        return EXIT_NO_DATA

    handoff = record.find(OS_LOADER_HANDOFF_MILESTONE)
    handoff_ms = None if handoff is None else _milliseconds(handoff)
    if handoff_ms is None or handoff_ms == UNREACHED_MILESTONE_MS:
        print(
            f"no {OS_LOADER_HANDOFF_MILESTONE} timestamp; the guest did not "
            "reach the OS loader",
            file=sys.stderr,
        )
        return EXIT_NO_DATA

    print(f"boot time (reset to OS loader handoff): {handoff_ms:.3f} ms")

    for name in REPORTED_MILESTONES:
        element = record.find(name)
        if element is None:
            continue
        value = _milliseconds(element)
        if value is None:
            continue
        if value == UNREACHED_MILESTONE_MS and name != RESET_MILESTONE:
            continue
        print(f"  {name:<26}{value:>12.3f} ms")

    return 0


def main(argv):
    if len(argv) != 2:
        print(f"usage: {argv[0]} FBPT_XML", file=sys.stderr)
        return EXIT_USAGE
    return summarize(argv[1])


if __name__ == "__main__":
    sys.exit(main(sys.argv))
