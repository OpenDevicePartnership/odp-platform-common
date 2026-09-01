//! Microbenchmarks for `patina_boot`.
//!
//! Run with:
//!
//!     cargo bench --bench orchestrator
//!
//! Add `-- --output-format bencher` for libtest-style lines that
//! standard perf-tracking tooling consumes.
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

use criterion::{Criterion, black_box, criterion_group, criterion_main};
use patina::uefi::boot_services::{MockBootServices, boxed::BootServicesBox};
use patina_boot::helpers;
use r_efi::efi;

/// Build a `MockBootServices` whose method expectations cover the
/// sequence `connect_all` + `signal_bds_phase_entry` +
/// `signal_ready_to_boot` exercise: `locate_handle_buffer`,
/// `connect_controller`, `create_event_ex_unchecked`, `signal_event`,
/// and `close_event`. Returns a leaked `'static` reference because
/// `BootServicesBox` borrows the mock and criterion's iter closures
/// outlive the surrounding stack frame.
fn build_mock() -> &'static MockBootServices {
    // Raw pointer types (`efi::Handle = *mut c_void`, `efi::Event`)
    // are not `Send`, so addresses are carried into the returning
    // closures as `usize` and cast back to the pointer type inside.
    let handle_addr: usize = 0x1000;
    let event_addr: usize = 0x2000;

    let inner_mock_for_box: &'static MockBootServices = Box::leak(Box::new({
        let mut m = MockBootServices::new();
        m.expect_free_pool().returning(|_| Ok(()));
        m
    }));

    let mut m = MockBootServices::new();

    // locate_handle_buffer: return a single synthetic handle each call,
    // backed by a one-time leaked array so per-iteration memory use stays
    // flat across the run.
    let handles: &'static mut [efi::Handle] = Vec::leak(alloc::vec![handle_addr as efi::Handle]);
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
    // The turbofish on `create_event_ex_unchecked::<()>` matches what
    // `signal_bds_phase_entry` and `signal_ready_to_boot` actually call:
    // a null `T` context for signal-only events.
    m.expect_create_event_ex_unchecked::<()>()
        .returning(move |_, _, _, _, _| Ok(event_addr as efi::Event));
    m.expect_signal_event().returning(|_| Ok(()));
    m.expect_close_event().returning(|_| Ok(()));

    Box::leak(Box::new(m))
}

/// Composite bench of the BDS-phase sequence that
/// `SimpleBootManager::execute()` runs before iterating boot options:
/// connect controllers, signal EndOfDxe, signal ReadyToBoot.
///
/// Note: a true `BootOrchestrator::execute()` bench requires a
/// `StandardBootServices` test factory (a fake `efi::BootServices`
/// table with stub function pointers) that does not exist yet.
/// Pending that, this composite is the closest end-to-end measurement
/// of the BDS chain achievable against the public helper surface.
fn bds_phase_composite(c: &mut Criterion) {
    let mock = build_mock();
    let iter_count = AtomicUsize::new(0);

    c.bench_function("bds_phase_composite", |b| {
        b.iter(|| {
            // Assert the happy path so the numbers can't silently become a
            // measurement of an error/early-return path.
            helpers::connect_all(mock).expect("connect_all");
            helpers::signal_bds_phase_entry(mock).expect("signal_bds_phase_entry");
            helpers::signal_ready_to_boot(mock).expect("signal_ready_to_boot");
            black_box(iter_count.fetch_add(1, Ordering::Relaxed));
        })
    });
}

criterion_group!(benches, bds_phase_composite);
criterion_main!(benches);
