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
use patina::boot_services::{MockBootServices, boxed::BootServicesBox};
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

/// Build a `MockBootServices` presenting `handle_count` synthetic handles:
/// `locate_handle_buffer` returns the same leaked handle array on every call
/// (so `connect_all`'s convergence loop sees a stable topology after one
/// pass) and `connect_controller` succeeds for every handle.
fn build_connect_topology_mock(handle_count: usize) -> &'static MockBootServices {
    let inner_mock_for_box: &'static MockBootServices = Box::leak(Box::new({
        let mut m = MockBootServices::new();
        m.expect_free_pool().returning(|_| Ok(()));
        m
    }));

    let mut m = MockBootServices::new();

    let handles: &'static mut [efi::Handle] =
        Vec::leak((1..=handle_count).map(|i| (0x1000 + i * 0x10) as efi::Handle).collect());
    let handles_ptr = handles.as_mut_ptr() as usize;
    let handles_len = handles.len();
    m.expect_locate_handle_buffer().returning(move |_| {
        // SAFETY: the pointer/len name the one-time leaked array above, which
        // is never freed (the mock `free_pool` is a no-op). Views never
        // overlap: the caller acquires one box per `locate_handle_buffer`
        // call and drops it before the next call, and only reads through it,
        // so no two mutable views of the buffer are live at once.
        let bx = unsafe {
            BootServicesBox::from_raw_parts_mut(handles_ptr as *mut efi::Handle, handles_len, inner_mock_for_box)
        };
        Ok(bx)
    });
    m.expect_connect_controller().returning(|_, _, _, _| Ok(()));

    Box::leak(Box::new(m))
}

/// `connect_all` against synthetic handle topologies of increasing size.
/// Cost model: the first pass connects every handle; later passes re-enumerate
/// but skip already-connected handles, and a stable topology converges after
/// two passes. Measured work still scales with the handle count via the
/// first-pass connects plus per-pass enumeration and seen-set lookups.
fn connect_all_topology(c: &mut Criterion) {
    let mut group = c.benchmark_group("connect_all");
    for &n in &[10usize, 100, 1000] {
        let mock = build_connect_topology_mock(n);
        group.bench_with_input(criterion::BenchmarkId::from_parameter(n), &n, |b, _| {
            b.iter(|| helpers::connect_all(black_box(mock)).expect("connect_all"))
        });
    }
    group.finish();
}

/// `expand_device_path` against synthetic filesystem topologies: `volumes`
/// handles each publish a full ACPI/HD device path with a distinct GPT
/// signature; the partial path's HardDrive node matches only the LAST
/// handle, so the expansion scans the whole set (worst case).
fn expand_device_path_topology(c: &mut Criterion) {
    use patina::device_path::node_defs::{Acpi, FilePath, HardDrive};
    use patina::device_path::paths::{DevicePath, DevicePathBuf};
    use r_efi::protocols::device_path;

    let mut group = c.benchmark_group("expand_device_path");
    for &volumes in &[4usize, 64, 256] {
        let inner_mock_for_box: &'static MockBootServices = Box::leak(Box::new({
            let mut m = MockBootServices::new();
            m.expect_free_pool().returning(|_| Ok(()));
            m
        }));

        // One full device path per handle; only the last carries the target
        // GPT signature.
        let target_sig = [0xAA_u8; 16];
        let path_addrs: &'static [usize] = Vec::leak(
            (0..volumes)
                .map(|i| {
                    let sig = if i == volumes - 1 {
                        target_sig
                    } else {
                        // Guaranteed to differ from target_sig ([0xAA; 16]):
                        // bytes 2..16 are 0x55, and the index in bytes 0..2
                        // keeps every non-target signature distinct.
                        let mut s = [0x55_u8; 16];
                        s[0] = i as u8;
                        s[1] = (i >> 8) as u8;
                        s
                    };
                    let dp = Box::leak(Box::new(DevicePathBuf::from_device_path_node_iter(
                        [Acpi::new_pci_root(0)].into_iter(),
                    )));
                    let hd = DevicePathBuf::from_device_path_node_iter(
                        [HardDrive::new_gpt(1, 2048, 1_000_000, sig)].into_iter(),
                    );
                    dp.append_device_path(&hd);
                    dp.as_ref() as *const DevicePath as *const u8 as usize
                })
                .collect(),
        );

        let handles: &'static mut [efi::Handle] =
            Vec::leak((1..=volumes).map(|i| (0x4000 + i * 0x10) as efi::Handle).collect());
        let handles_ptr = handles.as_mut_ptr() as usize;
        let handles_len = handles.len();

        let mut m = MockBootServices::new();
        m.expect_locate_handle_buffer().returning(move |_| {
            // SAFETY: same leaked-array contract as build_connect_topology_mock:
            // one box live at a time, read-only access, buffer never freed.
            let bx = unsafe {
                BootServicesBox::from_raw_parts_mut(handles_ptr as *mut efi::Handle, handles_len, inner_mock_for_box)
            };
            Ok(bx)
        });
        // SAFETY: Bench code — returns pointers to leaked, valid DevicePathBufs.
        unsafe {
            m.expect_handle_protocol::<device_path::Protocol>()
                .returning(move |handle| {
                    let idx = (handle as usize - 0x4010) / 0x10;
                    Ok((path_addrs[idx] as *mut device_path::Protocol).as_mut().unwrap())
                });
        }
        let mock: &'static MockBootServices = Box::leak(Box::new(m));

        // Partial path: HD(target signature)/FilePath — expansion must find
        // the handle whose HD node matches and splice the remainder.
        let mut partial =
            DevicePathBuf::from_device_path_node_iter([HardDrive::new_gpt(1, 2048, 1_000_000, target_sig)].into_iter());
        let file_node =
            DevicePathBuf::from_device_path_node_iter([FilePath::new("\\EFI\\Boot\\BOOTX64.efi")].into_iter());
        partial.append_device_path(&file_node);
        let partial: &'static DevicePathBuf = Box::leak(Box::new(partial));

        group.bench_with_input(criterion::BenchmarkId::from_parameter(volumes), &volumes, |b, _| {
            b.iter(|| helpers::expand_device_path(black_box(mock), black_box(partial.as_ref())).expect("expand"))
        });
    }
    group.finish();
}

#[path = "support/fake_tables.rs"]
mod fake_tables;

/// True end-to-end bench of `BootOrchestrator::execute()` against fake
/// firmware tables: `StandardBootServices`/`StandardRuntimeServices` wrap
/// stub `efi::BootServices`/`efi::RuntimeServices` tables (see
/// `support/fake_tables.rs`), and dispatch is a no-op mock. The canned
/// behavior presents no handles, no variables, and no loadable images, so
/// each iteration runs the full flow (connect, phase signals, console and
/// boot-option discovery, boot attempts) and exhausts with an error.
fn execute_e2e(c: &mut Criterion) {
    use patina::boot_services::StandardBootServices;
    use patina::component::service::dxe_dispatch::MockDxeDispatch;
    use patina::error::EfiError;
    use patina::runtime_services::StandardRuntimeServices;
    use patina_boot::{boot_orchestrator::BootOrchestrator, config::BootConfig, orchestrators::SimpleBootManager};

    let bs_table = Box::leak(fake_tables::fake_boot_services());
    let rt_table = Box::leak(fake_tables::fake_runtime_services());
    let boot_services = StandardBootServices::new(bs_table);
    let runtime_services = StandardRuntimeServices::new(rt_table);

    let mut dispatch = MockDxeDispatch::new();
    dispatch.expect_dispatch().returning(|| Ok(false));

    let config = BootConfig::new(patina::device_path::paths::DevicePathBuf::from_device_path_node_iter(
        [patina::device_path::node_defs::Acpi::new_pci_root(0)].into_iter(),
    ));
    let manager = SimpleBootManager::new(config);

    c.bench_function("execute_e2e", |b| {
        b.iter(|| {
            let result = manager.execute(
                &boot_services,
                &runtime_services,
                &dispatch,
                core::ptr::dangling_mut::<core::ffi::c_void>(),
            );
            // Exhausting every option is the expected terminal state.
            assert!(matches!(result, Err(EfiError::NotFound)));
            black_box(result.err())
        })
    });
}

criterion_group!(
    benches,
    bds_phase_composite,
    connect_all_topology,
    expand_device_path_topology,
    execute_e2e
);
criterion_main!(benches);
