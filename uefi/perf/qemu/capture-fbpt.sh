#!/usr/bin/env bash
#
# Captures firmware performance data from a Patina Q35 firmware under QEMU.
#
# Boots the firmware with the dump disk from make-fbpt-disk.sh attached. The
# guest writes the firmware basic boot performance table (FBPT) back to that
# disk and powers itself off, so this waits for QEMU to exit rather than
# watching for a console marker. The table is then copied out and parsed.
#
# Exit codes:
#   0  the table was captured and parsed
#   1  the capture failed: measurement is disabled in the firmware, the guest
#      never powered off, the dump log could not be read back, or no table was
#      written
#   2  a setup problem: bad arguments, a missing file or tool, an output
#      directory that cannot be prepared, or QEMU itself failing to run
#
# Copyright (c) Microsoft Corporation.
#
# SPDX-License-Identifier: MIT
#

set -euo pipefail

# QEMU wiring that the Patina Q35 firmware expects; fixed by the platform.
readonly DEBUGCON_IO_PORT=0x402
readonly DEBUG_EXIT_IO_PORT=0xf4
readonly DEBUG_EXIT_IO_SIZE=0x04

# Flash unit 0 is execute-only code, unit 1 is the writable variable store.
readonly FLASH_UNIT_CODE=0
readonly FLASH_UNIT_VARS=1

readonly GUEST_MEMORY_MB=2048
readonly GUEST_CPU_COUNT=4

# The guest powers itself off once the dump completes, so this only bounds a
# hang. A healthy capture finishes in well under a minute.
readonly DEFAULT_TIMEOUT_SECONDS=300
readonly POLL_INTERVAL_SECONDS=2

# Written by the dump application; the name carries the model and firmware
# version, which vary, so it is matched by prefix.
readonly FBPT_FILE_PREFIX=FBPT
readonly SHELL_LOG_NAME=dumplog.txt

# Reported by the dump application on success.
readonly DUMP_SUCCESS_MARKER='Wrote to file'

# Confirms the firmware was built with performance tracing selected. Without
# it the boot succeeds but carries no measurements.
readonly PERFORMANCE_ENABLED_MARKER='Patina Performance Config HOB: Enabled=1'

readonly EXIT_USAGE=2

readonly POSITIVE_INTEGER_PATTERN='^[1-9][0-9]*$'

readonly QEMU_COMMAND=qemu-system-x86_64

# The parser wrapper imports this; checking for it up front turns a missing
# dependency into a setup error rather than a failure after a full boot.
readonly PARSER_MODULE=edk2toolext.perf.fpdt_parser

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
readonly PARSER_WRAPPER="${script_dir}/fpdt_parser_any_platform.py"
readonly BOOT_TIME_SUMMARY="${script_dir}/boot_time_summary.py"

usage() {
  cat <<'EOF'
Usage: capture-fbpt.sh --firmware-dir DIR --disk IMAGE --out-dir DIR
                       [--timeout SECONDS] [--python PYTHON]

  --firmware-dir  Directory holding QEMUQ35_CODE.fd and QEMUQ35_VARS.fd.
  --disk          Dump disk image produced by make-fbpt-disk.sh.
  --out-dir       Directory for the captured table, logs and parsed output.
  --timeout       Seconds to wait for the guest to power itself off.
  --python        Interpreter with edk2-pytool-extensions installed.

The firmware must be built with 'BLD_*_PERF_TRACE_ENABLE=TRUE'; see README.md.
EOF
}

firmware_dir=""
disk_image=""
out_dir=""
timeout_seconds="${DEFAULT_TIMEOUT_SECONDS}"
python_bin="python3"

# Reading "$2" for a flag given without a value trips 'set -u', which reports a
# bash error and exits 1 -- the code that means the capture itself failed.
# Check for the value first so misuse stays distinguishable.
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
    --disk) require_value "$1" "${2:-}"; disk_image="$2"; shift 2 ;;
    --out-dir) require_value "$1" "${2:-}"; out_dir="$2"; shift 2 ;;
    --timeout) require_value "$1" "${2:-}"; timeout_seconds="$2"; shift 2 ;;
    --python) require_value "$1" "${2:-}"; python_bin="$2"; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) echo "unknown argument: $1" >&2; usage >&2; exit "$EXIT_USAGE" ;;
  esac
done

if [ -z "$firmware_dir" ] || [ -z "$disk_image" ] || [ -z "$out_dir" ]; then
  echo "--firmware-dir, --disk and --out-dir are all required" >&2
  usage >&2
  exit "$EXIT_USAGE"
fi

# The timeout is only used in arithmetic after QEMU has started, so a bad value
# would otherwise surface as a failed capture rather than as misuse.
if ! [[ "$timeout_seconds" =~ $POSITIVE_INTEGER_PATTERN ]]; then
  echo "--timeout must be a positive whole number of seconds: ${timeout_seconds}" >&2
  exit "$EXIT_USAGE"
fi

code_fd="${firmware_dir}/QEMUQ35_CODE.fd"
vars_fd_source="${firmware_dir}/QEMUQ35_VARS.fd"

for image in "$code_fd" "$vars_fd_source" "$disk_image"; do
  if [ ! -f "$image" ]; then
    echo "image not found: $image" >&2
    exit "$EXIT_USAGE"
  fi
done

# Checked up front so a missing tool does not surface as a generic 127 partway
# through a capture, which the caller cannot tell apart from a firmware that
# produced no performance data.
for tool in "$QEMU_COMMAND" mtype mdir mcopy awk; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "required tool not found: $tool (install the qemu-system-x86, mtools and gawk packages)" >&2
    exit "$EXIT_USAGE"
  fi
done

if ! command -v "$python_bin" >/dev/null 2>&1; then
  echo "python interpreter not found: ${python_bin}" >&2
  exit "$EXIT_USAGE"
fi

# The parser is only invoked after a full boot, so an uninstalled dependency
# would otherwise waste the whole capture before failing. Locate the module
# without importing it: on Linux, executing it raises ImportError on ctypes
# names that only exist on Windows, which is the very thing the wrapper exists
# to paper over.
if ! "$python_bin" -c \
  "import importlib.util as u, sys; sys.exit(0 if u.find_spec('${PARSER_MODULE}') else 1)" \
  >/dev/null 2>&1; then
  echo "${python_bin} cannot find ${PARSER_MODULE}; install edk2-pytool-extensions" >&2
  exit "$EXIT_USAGE"
fi

# 'set -e' would exit 1 on these, so route them through EXIT_USAGE instead.
setup_failed() {
  echo "$1" >&2
  exit "$EXIT_USAGE"
}

mkdir -p "$out_dir" || setup_failed "could not create output directory: ${out_dir}"
boot_log="${out_dir}/capture-debugcon.log"

# QEMU announces on stderr that it was signalled, which happens whenever this
# script has to stop it. Capturing stderr keeps that notice out of the output;
# it is printed below if QEMU failed to run at all.
qemu_stderr_log="${out_dir}/qemu-stderr.log"

# The guest writes to both the variable store and the dump disk, so run against
# copies to keep the inputs reusable across captures.
vars_fd="${out_dir}/QEMUQ35_VARS.writable.fd"
capture_disk="${out_dir}/fbpt-disk.img"
cp "$vars_fd_source" "$vars_fd" || setup_failed "could not copy the variable store into ${out_dir}"
cp "$disk_image" "$capture_disk" || setup_failed "could not copy the dump disk into ${out_dir}"
chmod u+w "$vars_fd" "$capture_disk" || setup_failed "could not make the capture inputs writable"

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
  -device ich9-ahci,id=ahci \
  -drive "id=fbptdisk,if=none,format=raw,file=${capture_disk}" \
  -device ide-hd,bus=ahci.0,drive=fbptdisk \
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
guest_powered_off=false
qemu_status=0

while [ "$SECONDS" -lt "$deadline" ]; do
  if ! kill -0 "$qemu_pid" 2>/dev/null; then
    guest_powered_off=true
    qemu_reaped=true
    wait "$qemu_pid" 2>/dev/null || qemu_status=$?
    break
  fi
  sleep "$POLL_INTERVAL_SECONDS"
done

stop_qemu
trap - EXIT

echo "boot log: ${boot_log}"

# A non-zero status means QEMU never got the guest running, so the checks below
# would otherwise blame the firmware for what is really a setup problem.
if [ "$qemu_status" -ne 0 ]; then
  echo "QEMU exited with status ${qemu_status} before the guest could boot; check the invocation and ${boot_log}" >&2
  cat "$qemu_stderr_log" >&2
  exit "$EXIT_USAGE"
fi

if ! grep -qF "$PERFORMANCE_ENABLED_MARKER" "$boot_log" 2>/dev/null; then
  echo "FAIL: firmware did not enable performance measurement" >&2
  echo "      rebuild with 'BLD_*_PERF_TRACE_ENABLE=TRUE'" >&2
  exit 1
fi

if [ "$guest_powered_off" != true ]; then
  echo "FAIL: guest did not power off within ${timeout_seconds}s" >&2
  exit 1
fi

export MTOOLS_SKIP_CHECK=1

# Redirecting straight onto the final path would create the log even when mtype
# fails, and the marker check below would then blame the dump application for a
# table it may well have written. Stage the extraction and install it only once
# mtype has actually succeeded, so the two failures stay distinguishable.
shell_log="${out_dir}/${SHELL_LOG_NAME}"
shell_log_staging="${shell_log}.partial"
if ! mtype -i "$capture_disk" "::/${SHELL_LOG_NAME}" >"$shell_log_staging" 2>/dev/null; then
  rm -f "$shell_log_staging"
  echo "FAIL: could not read ${SHELL_LOG_NAME} from the dump disk" >&2
  exit 1
fi
mv "$shell_log_staging" "$shell_log"
echo "dump application output: ${shell_log}"

if ! grep -qF "$DUMP_SUCCESS_MARKER" "$shell_log" 2>/dev/null; then
  echo "FAIL: the dump application did not report writing a table" >&2
  exit 1
fi

# The written name embeds the model and firmware version and contains spaces,
# so recover the short name from the directory listing rather than globbing.
short_name="$(mdir -i "$capture_disk" ::/ \
  | awk -v prefix="$FBPT_FILE_PREFIX" \
      '$1 ~ "^" prefix && $2 == "BIN" { print $1 "." $2; exit }')"

if [ -z "$short_name" ]; then
  echo "FAIL: no ${FBPT_FILE_PREFIX}*.bin on the dump disk" >&2
  exit 1
fi

fbpt_bin="${out_dir}/${FBPT_FILE_PREFIX}.bin"
mcopy -o -i "$capture_disk" "::/${short_name}" "$fbpt_bin"
echo "captured table: ${fbpt_bin} ($(wc -c <"$fbpt_bin") bytes)"

"$python_bin" "$PARSER_WRAPPER" \
  -b "$fbpt_bin" \
  -x "${out_dir}/fbpt.xml" \
  -t "${out_dir}/fbpt.txt"

echo "PASS: captured firmware performance data"
echo "parsed records: ${out_dir}/fbpt.xml, ${out_dir}/fbpt.txt"

# The summary only needs the standard library, so it runs under the same
# interpreter without requiring the parser package.
"$python_bin" "$BOOT_TIME_SUMMARY" "${out_dir}/fbpt.xml"
