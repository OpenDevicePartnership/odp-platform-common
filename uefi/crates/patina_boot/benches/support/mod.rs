//! Shared bench support.
//!
//! [`Cycles`] is a criterion measurement backed by the x86 timestamp counter
//! (`rdtsc`), reporting elapsed reference cycles per iteration instead of
//! wall-clock time. The board acceptance criteria call for cycle counts, which
//! are also frequency-independent and so more stable across machines than
//! wall-time.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: MIT
//!
use criterion::{
    Throughput,
    measurement::{Measurement, ValueFormatter},
};

/// Criterion measurement reporting elapsed reference cycles (`rdtsc`).
pub struct Cycles;

impl Measurement for Cycles {
    type Intermediate = u64;
    type Value = u64;

    fn start(&self) -> u64 {
        // SAFETY: `_rdtsc` reads the timestamp counter; it is available on every
        // x86_64 host (where these benches run) and has no preconditions.
        unsafe { core::arch::x86_64::_rdtsc() }
    }

    fn end(&self, start: u64) -> u64 {
        // SAFETY: see `start`.
        let now = unsafe { core::arch::x86_64::_rdtsc() };
        now.wrapping_sub(start)
    }

    fn add(&self, v1: &u64, v2: &u64) -> u64 {
        v1.wrapping_add(*v2)
    }

    fn zero(&self) -> u64 {
        0
    }

    fn to_f64(&self, value: &u64) -> f64 {
        *value as f64
    }

    fn formatter(&self) -> &dyn ValueFormatter {
        &CyclesFormatter
    }
}

struct CyclesFormatter;

impl CyclesFormatter {
    /// Scale raw cycle counts to cyc/Kcyc/Mcyc/Gcyc by magnitude and return the
    /// unit label.
    fn scale(typical: f64, values: &mut [f64]) -> &'static str {
        let (denom, unit) = if typical < 1e3 {
            (1.0, "cyc")
        } else if typical < 1e6 {
            (1e3, "Kcyc")
        } else if typical < 1e9 {
            (1e6, "Mcyc")
        } else {
            (1e9, "Gcyc")
        };
        for v in values.iter_mut() {
            *v /= denom;
        }
        unit
    }
}

impl ValueFormatter for CyclesFormatter {
    fn scale_values(&self, typical_value: f64, values: &mut [f64]) -> &'static str {
        Self::scale(typical_value, values)
    }

    fn scale_throughputs(&self, _typical: f64, throughput: &Throughput, values: &mut [f64]) -> &'static str {
        // Report cycles per unit of work rather than per second, which has no
        // meaning against a cycle-count base.
        let (n, unit) = match throughput {
            Throughput::Elements(n) => (*n as f64, "cyc/elem"),
            Throughput::Bytes(n) | Throughput::BytesDecimal(n) => (*n as f64, "cyc/byte"),
        };
        for v in values.iter_mut() {
            *v /= n;
        }
        unit
    }

    fn scale_for_machines(&self, _values: &mut [f64]) -> &'static str {
        "cycles"
    }
}
