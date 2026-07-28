//! Optional per-phase performance instrumentation for [`crate::SreBootManager`].
//!
//! Locates the EDKII Performance Measurement Protocol (published by
//! `patina_performance`) and emits PerfCrossModule start/end records around
//! boot-manager phases, so per-phase durations land in the FPDT and are
//! visible to the standard tools (`dp`, WPA). The same start/end also brackets
//! a CPU-timestamp read so the phase duration in milliseconds is written to the
//! log directly — useful where the FPDT extended records are not conveniently
//! readable.
//!
//! Best-effort: if the protocol is absent (no performance component), every
//! operation is a no-op and boot is unaffected.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: MIT
//!
extern crate alloc;

use alloc::ffi::CString;
use core::ffi::{c_char, c_void};

use patina::boot_services::BootServices;
use r_efi::efi;

/// `EDKII_PERFORMANCE_MEASUREMENT_PROTOCOL_GUID`
/// (`C85D06BE-5F75-48CE-A80F-1236BA3B87B1`). `static` so a `'static`
/// reference is available for `locate_protocol_unchecked`.
static PERF_PROTOCOL_GUID: efi::Guid =
    efi::Guid::from_fields(0xC85D06BE, 0x5F75, 0x48CE, 0xA8, 0x0F, &[0x12, 0x36, 0xBA, 0x3B, 0x87, 0xB1]);

/// `PerfAttribute` discriminants (matches the EDKII protocol ABI).
const PERF_START_ENTRY: u32 = 0;
const PERF_END_ENTRY: u32 = 1;

/// `KnownPerfId::PerfCrossModuleStart` / `...End` — the IDs for a named
/// measurement that spans an arbitrary span of boot.
const PERF_CROSS_MODULE_START: u32 = 0x50;
const PERF_CROSS_MODULE_END: u32 = 0x51;

/// Caller GUID attributed to these records in the FPDT (SreBootManager).
static SRE_PERF_CALLER_GUID: efi::Guid = efi::Guid::from_fields(
    0x5be1c0de,
    0x0b0d,
    0x4a11,
    0x9c,
    0x3d,
    &[0x53, 0x52, 0x45, 0x50, 0x45, 0x52],
);

/// EDKII Performance Measurement Protocol create function (ABI).
type CreateMeasurementFn = unsafe extern "efiapi" fn(
    caller_identifier: *const c_void,
    guid: *const efi::Guid,
    string: *const c_char,
    ticker: u64,
    address: usize,
    identifier: u32,
    attribute: u32,
) -> efi::Status;

/// Read the CPU timestamp counter (millisecond deltas only; absolute value
/// is meaningless). Returns 0 on architectures without a cheap counter here,
/// which disables the millisecond log line but leaves the FPDT records intact.
#[cfg(target_arch = "x86_64")]
#[inline]
fn cpu_ticks() -> u64 {
    // SAFETY: rdtsc has no preconditions and no side effects.
    unsafe { core::arch::x86_64::_rdtsc() }
}

#[cfg(not(target_arch = "x86_64"))]
#[inline]
fn cpu_ticks() -> u64 {
    0
}

/// Calibrated millisecond timer, plus an optional handle to the
/// performance-measurement protocol for FPDT records. The millisecond log line
/// works from the CPU timestamp alone, so it is available even when the
/// protocol is absent.
pub(crate) struct Perf {
    create: Option<CreateMeasurementFn>,
    ticks_per_ms: u64,
}

impl Perf {
    /// Calibrate the timestamp counter against a short stall and (best-effort)
    /// locate the performance-measurement protocol. Always returns a value:
    /// the millisecond log line is produced from the CPU timestamp regardless
    /// of whether the protocol is present.
    pub(crate) fn locate<B: BootServices>(boot_services: &B) -> Option<Self> {
        // Calibrate ticks/ms with a 50 ms stall.
        let t0 = cpu_ticks();
        let _ = boot_services.stall(50_000);
        let t1 = cpu_ticks();
        let ticks_per_ms = t1.wrapping_sub(t0) / 50;

        // Best-effort: locate the protocol for FPDT records. Absence only
        // disables the FPDT emit, not the millisecond log line.
        // SAFETY: the returned interface is only used to read the leading
        // function-pointer field of the protocol struct.
        let create =
            match unsafe { boot_services.locate_protocol_unchecked(&PERF_PROTOCOL_GUID, core::ptr::null_mut()) } {
                Ok(iface) if !iface.is_null() => {
                    // SAFETY: iface points at a valid EdkiiPerformanceMeasurement
                    // instance; the create function is its first (and only) field.
                    Some(unsafe { *(iface as *const CreateMeasurementFn) })
                }
                _ => {
                    log::info!("SRE-PERF: performance-measurement protocol not found; FPDT records disabled");
                    None
                }
            };

        Some(Self { create, ticks_per_ms })
    }

    fn emit(&self, name: &str, id: u32, attr: u32) {
        let Some(create) = self.create else { return };
        if let Ok(cstr) = CString::new(name) {
            // SAFETY: `create` is the protocol's efiapi function; the caller
            // GUID and string outlive the call.
            unsafe {
                create(core::ptr::null(), &SRE_PERF_CALLER_GUID, cstr.as_ptr(), 0, 0, id, attr);
            }
        }
    }

    /// Begin a named phase: emits an FPDT start record and captures a
    /// timestamp. Drop the returned guard to close
    /// the phase.
    pub(crate) fn scope<'a>(&'a self, name: &'static str) -> PerfScope<'a> {
        self.emit(name, PERF_CROSS_MODULE_START, PERF_START_ENTRY);
        PerfScope { perf: self, name, start: cpu_ticks() }
    }
}

/// RAII guard for one measured phase. Closing it emits the FPDT end record and
/// logs the elapsed milliseconds.
pub(crate) struct PerfScope<'a> {
    perf: &'a Perf,
    name: &'static str,
    start: u64,
}

impl Drop for PerfScope<'_> {
    fn drop(&mut self) {
        self.perf.emit(self.name, PERF_CROSS_MODULE_END, PERF_END_ENTRY);
        if self.perf.ticks_per_ms > 0 {
            let elapsed = cpu_ticks().wrapping_sub(self.start);
            log::info!("SRE-PERF {}: {} ms", self.name, elapsed / self.perf.ticks_per_ms);
        }
    }
}

/// No-op-safe wrapper: when `perf` is `None`, returns `None` and the caller's
/// phase simply runs unmeasured.
pub(crate) fn scope<'a>(perf: &'a Option<Perf>, name: &'static str) -> Option<PerfScope<'a>> {
    perf.as_ref().map(|p| p.scope(name))
}
