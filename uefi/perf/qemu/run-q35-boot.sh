#!/usr/bin/env bash
#
# Boots the Patina Q35 firmware under QEMU and reports whether it reaches BDS.
#
# Exit codes:
#   0  the BDS entry marker appeared on the debug console
#   1  the firmware did not reach BDS, either because the timeout expired or
#      because it reset or shut down first
#   2  a setup problem: bad arguments, a missing file or tool, an output
#      directory that cannot be prepared, or QEMU itself failing to run
#
# The firmware keeps running after BDS is reached when no boot device is
# attached, so this stops QEMU as soon as the marker is seen rather than
# waiting for an exit.
#
# Copyright (c) Microsoft Corporation.
#
# SPDX-License-Identifier: MIT
#

set -euo pipefail

# Fixed by the platform, not tunable. The firmware's logger writes to the debug
# console, which is where this script looks for the BDS marker. The debug exit
# device is part of the standard Q35 invocation; a plain boot to BDS never
# writes to it.
readonly DEBUGCON_IO_PORT=0x402
readonly DEBUG_EXIT_IO_PORT=0xf4
readonly DEBUG_EXIT_IO_SIZE=0x04

# Emitted by BdsDxe when the boot device selection phase begins.
readonly BDS_READY_MARKER='[Bds] Entry...'

# Q35 firmware is built as a pair of flash images: unit 0 is execute-only code,
# unit 1 is the writable variable store.
readonly FLASH_UNIT_CODE=0
readonly FLASH_UNIT_VARS=1

readonly GUEST_MEMORY_MB=2048
readonly GUEST_CPU_COUNT=4
readonly POLL_INTERVAL_SECONDS=2

# Generous enough for a cold TCG boot on a loaded CI runner; a healthy boot
# reaches BDS in a few seconds.
readonly DEFAULT_TIMEOUT_SECONDS=180

# Usage and environment problems exit with this, so they stay distinct from
# exit 1, which means the firmware itself did not reach BDS. Every setup check
# below uses it for that reason.
readonly EXIT_USAGE=2

readonly POSITIVE_INTEGER_PATTERN='^[1-9][0-9]*$'

readonly QEMU_COMMAND=qemu-system-x86_64

usage() {
  cat <<'EOF'
Usage: run-q35-boot.sh --firmware-dir DIR [--timeout SECONDS] [--out-dir DIR]

  --firmware-dir  Directory holding QEMUQ35_CODE.fd and QEMUQ35_VARS.fd.
  --timeout       Seconds to wait for BDS before failing.
  --out-dir       Directory for the boot log and the writable variable store.
EOF
}

firmware_dir=""
out_dir=""
timeout_seconds="${DEFAULT_TIMEOUT_SECONDS}"

# Reading "$2" for a flag given without a value trips 'set -u', which exits 1
# through a bash error. Check for the value first.
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
    --firmware-dir) require_value "$1" "${2:-}"; firmware_dir="$2"; shift 2 ;;
    --timeout) require_value "$1" "${2:-}"; timeout_seconds="$2"; shift 2 ;;
    --out-dir) require_value "$1" "${2:-}"; out_dir="$2"; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) echo "unknown argument: $1" >&2; usage >&2; exit "$EXIT_USAGE" ;;
  esac
done

if [ -z "$firmware_dir" ]; then
  echo "--firmware-dir is required" >&2
  usage >&2
  exit "$EXIT_USAGE"
fi

# Validated here because the timeout is not used until after QEMU has started.
if ! [[ "$timeout_seconds" =~ $POSITIVE_INTEGER_PATTERN ]]; then
  echo "--timeout must be a positive whole number of seconds: ${timeout_seconds}" >&2
  exit "$EXIT_USAGE"
fi

code_fd="${firmware_dir}/QEMUQ35_CODE.fd"
vars_fd_source="${firmware_dir}/QEMUQ35_VARS.fd"

if ! command -v "$QEMU_COMMAND" >/dev/null 2>&1; then
  echo "required command not found: ${QEMU_COMMAND}" >&2
  exit "$EXIT_USAGE"
fi

for image in "$code_fd" "$vars_fd_source"; do
  if [ ! -f "$image" ]; then
    echo "firmware image not found: $image" >&2
    exit "$EXIT_USAGE"
  fi
done

# 'set -e' would exit 1 on these, so route them through EXIT_USAGE instead.
setup_failed() {
  echo "$1" >&2
  exit "$EXIT_USAGE"
}

if [ -z "$out_dir" ]; then
  out_dir="$(mktemp -d)" || setup_failed "could not create a temporary output directory"
fi
mkdir -p "$out_dir" || setup_failed "could not create output directory: ${out_dir}"

boot_log="${out_dir}/boot-debugcon.log"

# QEMU announces on stderr that it was signalled, which happens whenever this
# script has to stop it. Capturing stderr keeps that notice out of the output;
# it is printed below if QEMU failed to run at all.
qemu_stderr_log="${out_dir}/qemu-stderr.log"

# The variable store is written during boot, so run against a copy to keep the
# extracted firmware directory reusable across runs.
vars_fd="${out_dir}/QEMUQ35_VARS.writable.fd"
cp "$vars_fd_source" "$vars_fd" || setup_failed "could not copy the variable store into ${out_dir}"
chmod u+w "$vars_fd" || setup_failed "could not make the variable store writable: ${vars_fd}"

: > "$boot_log" || setup_failed "could not create boot log: ${boot_log}"
: > "$qemu_stderr_log" || setup_failed "could not create QEMU log: ${qemu_stderr_log}"

"$QEMU_COMMAND" \
  -debugcon "file:${boot_log}" \
  -global "isa-debugcon.iobase=${DEBUGCON_IO_PORT}" \
  -global ICH9-LPC.disable_s3=1 \
  -device "isa-debug-exit,iobase=${DEBUG_EXIT_IO_PORT},iosize=${DEBUG_EXIT_IO_SIZE}" \
  -machine q35,smm=on,accel=tcg \
  -global driver=cfi.pflash01,property=secure,value=on \
  -cpu qemu64,+rdrand,+umip,+smep,+pdpe1gb,+popcnt,+sse,+sse2,+sse3,+ssse3,+sse4.2,+sse4.1 \
  -smp "${GUEST_CPU_COUNT}" \
  -m "${GUEST_MEMORY_MB}" \
  -drive "if=pflash,format=raw,unit=${FLASH_UNIT_CODE},file=${code_fd},readonly=on" \
  -drive "if=pflash,format=raw,unit=${FLASH_UNIT_VARS},file=${vars_fd}" \
  -display none \
  -no-reboot \
  2>"$qemu_stderr_log" &
qemu_pid=$!

# Signalling a pid that has already been reaped could reach an unrelated
# process that inherited the number, so the child is only ever signalled while
# this script still knows it is running.
qemu_reaped=false
stop_qemu() {
  if [ "$qemu_reaped" = true ]; then
    return
  fi
  qemu_reaped=true
  kill "$qemu_pid" 2>/dev/null || true
  wait "$qemu_pid" 2>/dev/null || true
}
trap stop_qemu EXIT

deadline=$((SECONDS + timeout_seconds))
reached_bds=false
qemu_exited=false
qemu_status=0

while [ "$SECONDS" -lt "$deadline" ]; do
  if grep -qF "$BDS_READY_MARKER" "$boot_log" 2>/dev/null; then
    reached_bds=true
    break
  fi
  # QEMU ending on its own is either the guest resetting or shutting down under
  # -no-reboot, which is a firmware failure, or QEMU refusing to run at all.
  # Its exit status tells the two apart: clean means the guest ended it.
  if ! kill -0 "$qemu_pid" 2>/dev/null; then
    qemu_exited=true
    qemu_reaped=true
    wait "$qemu_pid" 2>/dev/null || qemu_status=$?
    break
  fi
  sleep "$POLL_INTERVAL_SECONDS"
done

stop_qemu

echo "boot log: ${boot_log}"

if [ "$qemu_exited" = true ]; then
  if [ "$qemu_status" -eq 0 ]; then
    echo "FAIL: the firmware reset or shut down before reaching BDS" >&2
    exit 1
  fi
  echo "QEMU exited with status ${qemu_status} before the guest could boot; check the invocation and ${boot_log}" >&2
  cat "$qemu_stderr_log" >&2
  exit "$EXIT_USAGE"
fi

if [ "$reached_bds" != true ]; then
  echo "FAIL: did not reach BDS within ${timeout_seconds}s" >&2
  exit 1
fi

echo "PASS: reached BDS"
grep -m1 -nF "$BDS_READY_MARKER" "$boot_log"
