//! Microbenchmark for `patina_boot::helpers::expand_device_path`.
//!
//! Run with:
//!
//!     cargo bench --bench expand_device_path
//!
//! Add `-- --output-format bencher` for libtest-style lines that
//! standard perf-tracking tooling consumes.
//!
//! Reports elapsed reference cycles (`rdtsc`) per iteration via the shared
//! [`support::Cycles`] measurement. Measures how expansion scales while
//! scanning a synthetic device topology to find the handle whose HardDrive node
//! matches the target partition. The matching handle is placed last in the
//! buffer so every run performs a full scan (worst case).
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

use criterion::{BenchmarkId, Criterion, black_box, criterion_group, criterion_main};
use patina::{
    uefi::boot_services::{MockBootServices, boxed::BootServicesBox},
    uefi::device_path::{
        node_defs::{FilePath, HardDrive},
        paths::{DevicePath, DevicePathBuf},
    },
};
use patina_boot::helpers;

#[path = "support/mod.rs"]
mod support;
use r_efi::efi;

/// GPT partition signature the partial path targets; the one matching
/// handle in each topology carries this same signature.
const TARGET_GUID: [u8; 16] = [0xAA; 16];
/// Signature used for every non-matching handle so the scan walks a real
/// HardDrive node per handle without matching until the last one.
const OTHER_GUID: [u8; 16] = [0xBB; 16];
/// Base address for synthetic handle values. Handle `i` is `BASE + i`, so
/// the `handle_protocol` closure recovers the index by subtracting `BASE`.
const HANDLE_BASE: usize = 0x1000;

/// Build a full-form handle device path `PciRoot/HardDrive(guid)`, matching
/// the shape `expand_device_path` walks looking for a HardDrive node.
fn build_handle_path(guid: [u8; 16]) -> DevicePathBuf {
    use patina::uefi::device_path::node_defs::{Acpi, Pci};
    let mut path = DevicePathBuf::from_device_path_node_iter([Acpi::new_pci_root(0)].into_iter());
    let pci = DevicePathBuf::from_device_path_node_iter(
        [Pci {
            function: 0,
            device: 0x1D,
        }]
        .into_iter(),
    );
    path.append_device_path(&pci);
    let hd = DevicePathBuf::from_device_path_node_iter([HardDrive::new_gpt(1, 2048, 1000000, guid)].into_iter());
    path.append_device_path(&hd);
    path
}

/// Build the partial (short-form) input: `HardDrive(TARGET_GUID)/FilePath`,
/// EndEntire-terminated by `DevicePathBuf`. This is a well-formed partial
/// path so `expand_device_path` reaches the topology scan rather than
/// early-returning.
fn build_partial_path() -> DevicePathBuf {
    let mut partial =
        DevicePathBuf::from_device_path_node_iter([HardDrive::new_gpt(1, 2048, 1000000, TARGET_GUID)].into_iter());
    let fp = DevicePathBuf::from_device_path_node_iter([FilePath::new("\\EFI\\Boot\\BOOTX64.efi")].into_iter());
    partial.append_device_path(&fp);
    partial
}

/// Build a `MockBootServices` that presents an `n`-handle synthetic
/// topology: `n - 1` non-matching HardDrive handles followed by the single
/// matching handle. Returns a leaked `'static` reference because
/// `BootServicesBox` borrows the mock and criterion's iter closures outlive
/// the surrounding stack frame.
fn build_mock(n: usize) -> &'static MockBootServices {
    // Per-handle device path pointers, indexed by handle order. Each buffer
    // is leaked so the pointers stay valid for the whole run. Raw pointer
    // types are not `Send`, so pointers are carried into the closure as
    // `usize` and cast back inside.
    let mut dp_ptrs: Vec<usize> = Vec::with_capacity(n);
    let mut handle_addrs: Vec<usize> = Vec::with_capacity(n);
    for i in 0..n {
        let guid = if i == n - 1 { TARGET_GUID } else { OTHER_GUID };
        let leaked: &'static DevicePathBuf = Box::leak(Box::new(build_handle_path(guid)));
        dp_ptrs.push(leaked.as_ref() as *const DevicePath as *const u8 as usize);
        handle_addrs.push(HANDLE_BASE + i);
    }
    let dp_ptrs: &'static [usize] = dp_ptrs.leak();

    // Inner mock the returned `BootServicesBox` borrows; its `free_pool` is a
    // no-op so the leaked handle array is never actually freed.
    let inner_mock_for_box: &'static MockBootServices = Box::leak(Box::new({
        let mut m = MockBootServices::new();
        m.expect_free_pool().returning(|_| Ok(()));
        m
    }));

    // One-time leaked handle buffer; `locate_handle_buffer` re-wraps it each
    // call so per-iteration memory use stays flat.
    let handles: &'static mut [efi::Handle] =
        Vec::leak(handle_addrs.iter().map(|&a| a as efi::Handle).collect::<Vec<_>>());
    let handles_ptr = handles.as_mut_ptr() as usize;
    let handles_len = handles.len();

    let mut m = MockBootServices::new();

    m.expect_locate_handle_buffer().returning(move |_| {
        // SAFETY: the pointer/len name the one-time leaked handle array above,
        // which is never freed (the mock `free_pool` is a no-op), and each
        // returned `BootServicesBox` only reads from it within the call.
        // `inner_mock_for_box` outlives the returned box.
        let bx = unsafe {
            BootServicesBox::from_raw_parts_mut(handles_ptr as *mut efi::Handle, handles_len, inner_mock_for_box)
        };
        Ok(bx)
    });

    // SAFETY: each returned pointer names a leaked, never-freed `DevicePathBuf`
    // buffer; the handle index is recovered from the synthetic handle value and
    // is always in range because every queried handle came from the buffer above.
    unsafe {
        m.expect_handle_protocol::<efi::protocols::device_path::Protocol>()
            .returning(move |handle| {
                let idx = handle as usize - HANDLE_BASE;
                let ptr = dp_ptrs[idx] as *mut efi::protocols::device_path::Protocol;
                Ok(ptr.as_mut().unwrap())
            });
    }

    Box::leak(Box::new(m))
}

/// Scaling bench: expand a partial path against synthetic topologies of
/// increasing size, matching the last handle each time (full scan).
fn expand_scaling(c: &mut Criterion<support::Cycles>) {
    let partial: &'static DevicePathBuf = Box::leak(Box::new(build_partial_path()));
    let counter = AtomicUsize::new(0);

    let mut group = c.benchmark_group("expand_device_path");
    for &n in &[1usize, 16, 128, 512] {
        let mock = build_mock(n);
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.iter(|| {
                // Assert the happy path so an error/early-return can't
                // masquerade as a fast measurement.
                helpers::expand_device_path(mock, partial.as_ref()).expect("expand_device_path");
                black_box(counter.fetch_add(1, Ordering::Relaxed));
            })
        });
    }
    group.finish();
}

criterion_group! {
    name = benches;
    config = Criterion::default().with_measurement(support::Cycles);
    targets = expand_scaling
}
criterion_main!(benches);
