#!/usr/bin/env bash
#
# Builds a FAT disk image that boots to the UEFI Shell, writes the firmware
# basic boot performance table (FBPT) to the same image, and powers the guest
# off.
#
# The shell and the dump application both come from a patina-qemu build, so the
# dump matches the firmware under test.
#
# Copyright (c) Microsoft Corporation.
#
# SPDX-License-Identifier: MIT
#

set -euo pipefail

# Comfortably fits the shell, the dump application and a captured table, which
# runs to tens of kilobytes.
readonly DISK_IMAGE_SIZE_MB=64

# Removable-media boot path that firmware boot device selection looks for when
# no explicit boot option refers to the disk.
readonly REMOVABLE_BOOT_DIR='::/EFI/BOOT'
readonly REMOVABLE_BOOT_FILE='::/EFI/BOOT/BOOTX64.EFI'

readonly SHELL_BINARY_NAME=Shell.efi
readonly DUMP_BINARY_NAME=FbptDump.efi

# The dump application reports through the shell's standard output rather than
# the debug console, so the startup script redirects it onto the disk. The
# shell writes UCS-2 by default; '>a' selects ASCII so the host can read it
# without transcoding.
readonly SHELL_LOG_NAME=dumplog.txt

# Exit code for usage and environment problems, kept distinct from a capture
# that ran but produced nothing.
readonly EXIT_USAGE=2

usage() {
  cat <<'EOF'
Usage: make-fbpt-disk.sh --build-dir DIR --out IMAGE

  --build-dir  patina-qemu build output holding Shell.efi and FbptDump.efi,
               for example Build/QemuQ35Pkg/DEBUG_CLANGPDB/X64.
  --out        Path of the disk image to create, overwritten if present.

FbptDump.efi is not part of a default build. Add
UefiTestingPkg/PerfTests/FbptDump/FbptDump.inf to the platform description
before building.
EOF
}

build_dir=""
disk_image=""

# Reading "$2" for a flag given without a value trips 'set -u', which reports a
# bash error and exits 1 instead of the usage exit code.
require_value() {
  local flag="$1" value="${2:-}"
  if [ -z "$value" ]; then
    echo "missing value for $flag" >&2
    usage >&2
    exit "$EXIT_USAGE"
  fi
}

while [ $# -gt 0 ]; do
  case "$1" in
    --build-dir) require_value "$1" "${2:-}"; build_dir="$2"; shift 2 ;;
    --out) require_value "$1" "${2:-}"; disk_image="$2"; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) echo "unknown argument: $1" >&2; usage >&2; exit "$EXIT_USAGE" ;;
  esac
done

if [ -z "$build_dir" ] || [ -z "$disk_image" ]; then
  echo "--build-dir and --out are both required" >&2
  usage >&2
  exit "$EXIT_USAGE"
fi

shell_efi="${build_dir}/${SHELL_BINARY_NAME}"
dump_efi="${build_dir}/${DUMP_BINARY_NAME}"

for binary in "$shell_efi" "$dump_efi"; do
  if [ ! -f "$binary" ]; then
    echo "missing build output: $binary" >&2
    exit "$EXIT_USAGE"
  fi
done

for tool in dd mkfs.vfat mmd mcopy mdir; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "required tool not found: $tool (install dosfstools and mtools)" >&2
    exit "$EXIT_USAGE"
  fi
done

startup_script="$(mktemp)"
trap 'rm -f "$startup_script"' EXIT

# 'set -e' would surface each tool's own status here, so route them through
# EXIT_USAGE instead: building the image is setup, not a failed dump.
setup_failed() {
  echo "$1" >&2
  exit "$EXIT_USAGE"
}

# The shell runs startup.nsh from its own volume on entry. Dump the table, then
# power off so the host can read the image back without racing the guest.
cat >"$startup_script" <<NSH || setup_failed "could not write the startup script"
@echo -off
fs0:
cd fs0:\\
${DUMP_BINARY_NAME} >a fs0:\\${SHELL_LOG_NAME}
reset -s
NSH

rm -f "$disk_image" || setup_failed "could not replace ${disk_image}"
dd if=/dev/zero of="$disk_image" bs=1M count="$DISK_IMAGE_SIZE_MB" status=none \
  || setup_failed "could not allocate ${disk_image}"
mkfs.vfat -F 32 "$disk_image" >/dev/null \
  || setup_failed "could not format ${disk_image}"

# mtools refuses images whose geometry it cannot infer from a partition table.
export MTOOLS_SKIP_CHECK=1

mmd -i "$disk_image" ::/EFI \
  || setup_failed "could not create ::/EFI in ${disk_image}"
mmd -i "$disk_image" "$REMOVABLE_BOOT_DIR" \
  || setup_failed "could not create ${REMOVABLE_BOOT_DIR} in ${disk_image}"
mcopy -i "$disk_image" "$shell_efi" "$REMOVABLE_BOOT_FILE" \
  || setup_failed "could not copy the UEFI Shell into ${disk_image}"
mcopy -i "$disk_image" "$dump_efi" "::/${DUMP_BINARY_NAME}" \
  || setup_failed "could not copy ${DUMP_BINARY_NAME} into ${disk_image}"
mcopy -i "$disk_image" "$startup_script" ::/startup.nsh \
  || setup_failed "could not copy startup.nsh into ${disk_image}"

echo "created ${disk_image}"
mdir -i "$disk_image" ::/ || setup_failed "could not list ${disk_image}"
