//! Microbenchmark for `patina_boot::helpers::connect_all`.
//!
//! Run with:
//!
//!     cargo bench --bench connect_all
//!
//! Add `-- --output-format bencher` for libtest-style lines that
//! standard perf-tracking tooling consumes.
//!
//! Reports elapsed reference cycles (`rdtsc`) per iteration via the shared
//! [`support::Cycles`] measurement. Measures how `connect_all` scales with the
//! number of handles in the synthetic device topology: `connect_all` calls
//! `locate_handle_buffer` and connects each returned handle, so the parameter
//! is the handle count `N`.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: MIT
//!
extern crate alloc;

use alloc::{boxed::Box, vec::Vec};

use core::sync::atomic::{AtomicUsize, Ordering};

use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main};
use patina::uefi::boot_services::{MockBootServices, boxed::BootServicesBox};
use patina_boot::helpers;
use r_efi::efi;

#[path = "support/mod.rs"]
mod support;

/// Build a `MockBootServices` whose expectations cover exactly what
/// `connect_all` calls against a topology of `n` handles:
/// `locate_handle_buffer` and `connect_controller`. Returns a leaked
/// `'static` reference because `connect_all` borrows the mock and
/// criterion's iter closures outlive the surrounding stack frame.
///
/// `connect_all` loops until the handle count stabilizes, connecting every
/// returned handle on each pass. The mock returns the same `n` handles on
/// every call, so the count matches on the second pass and the loop stops.
/// Both `locate_handle_buffer` and `connect_controller` are therefore
/// registered with `.returning(...)` to allow the repeated calls.
fn build_mock(n: usize) -> &'static MockBootServices {
    // Raw pointer types (`efi::Handle = *mut c_void`) are not `Send`, so
    // handle addresses are carried into the returning closure as `usize`
    // and cast back to the pointer type inside. The `n` synthetic handles get
    // distinct addresses to model a real handle set.
    let handle_addrs: Vec<efi::Handle> = (0..n).map(|i| (0x1000 + i * 0x10) as efi::Handle).collect();

    let inner_mock_for_box: &'static MockBootServices = Box::leak(Box::new({
        let mut m = MockBootServices::new();
        m.expect_free_pool().returning(|_| Ok(()));
        m
    }));

    let mut m = MockBootServices::new();

    // locate_handle_buffer: return the `n` synthetic handles each call,
    // backed by a one-time leaked array so per-iteration memory use stays
    // flat across the run.
    let handles: &'static mut [efi::Handle] = Vec::leak(handle_addrs);
    let handles_ptr = handles.as_mut_ptr() as usize;
    let handles_len = handles.len();
    m.expect_locate_handle_buffer().returning(move |_| {
        // SAFETY: the pointer/len name the one-time leaked array above,
        // which is never freed (the mock `free_pool` is a no-op), and each
        // returned `BootServicesBox` only reads from it within the call.
        // `inner_mock_for_box` outlives the returned box.
        let bx = unsafe {
            BootServicesBox::from_raw_parts_mut(handles_ptr as *mut efi::Handle, handles_len, inner_mock_for_box)
        };
        Ok(bx)
    });

    m.expect_connect_controller().returning(|_, _, _, _| Ok(()));

    Box::leak(Box::new(m))
}

/// Bench `connect_all` across a range of synthetic topology sizes to
/// characterize its per-handle scaling.
fn connect_all_scaling(c: &mut Criterion<support::Cycles>) {
    const SIZES: [usize; 4] = [1, 16, 128, 512];

    let iter_count = AtomicUsize::new(0);
    let mut group = c.benchmark_group("connect_all");

    for &n in SIZES.iter() {
        let mock = build_mock(n);
        group.throughput(Throughput::Elements(n as u64));
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.iter(|| {
                // Assert the happy path so the numbers can't silently become a
                // measurement of an error/early-return path.
                helpers::connect_all(mock).expect("connect_all");
                black_box(iter_count.fetch_add(1, Ordering::Relaxed));
            })
        });
    }

    group.finish();
}

criterion_group! {
    name = benches;
    config = Criterion::default().with_measurement(support::Cycles);
    targets = connect_all_scaling
}
criterion_main!(benches);
