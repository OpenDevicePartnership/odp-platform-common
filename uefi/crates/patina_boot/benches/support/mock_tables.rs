//! Mock `efi::BootServices` / `efi::RuntimeServices` tables for driving
//! `BootOrchestrator::execute()` (which takes the concrete `StandardBootServices`
//! wrappers) without firmware.
//!
//! Rust function pointers cannot be null, so every UEFI table slot must contain
//! a correctly typed function. Only slots exercised by the benchmark have mock
//! behavior; the rest log and return `UNSUPPORTED`. This is the single
//! maintenance point for the table shape: an `r-efi` API change intentionally
//! fails compilation here rather than leaving an invalid service table.
//!
//! This is a prototype for a reusable standard-service mock factory in Patina.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: MIT
//!
#![allow(clippy::missing_safety_doc)]
use r_efi::efi;
use r_efi::system::{
    AllocateType, CapsuleHeader, EventNotify, InterfaceType, LocateSearchType, MemoryDescriptor, MemoryType,
    OpenProtocolInformationEntry, ResetType, TableHeader, Time, TimeCapabilities, TimerDelay,
};

fn zero_header() -> TableHeader {
    TableHeader {
        signature: 0,
        revision: 0,
        header_size: 0,
        crc32: 0,
        reserved: 0,
    }
}

unsafe extern "efiapi" fn bs_raise_tpl(_a0: r_efi::base::Tpl) -> r_efi::base::Tpl {
    _a0
}

unsafe extern "efiapi" fn bs_restore_tpl(_a0: r_efi::base::Tpl) {
    // no-op
}

unsafe extern "efiapi" fn bs_allocate_pages(
    _a0: AllocateType,
    _a1: MemoryType,
    _a2: usize,
    _a3: *mut r_efi::base::PhysicalAddress,
) -> r_efi::base::Status {
    eprintln!("fake_tables: unimplemented slot called: allocate_pages");
    efi::Status::UNSUPPORTED
}

unsafe extern "efiapi" fn bs_free_pages(_a0: r_efi::base::PhysicalAddress, _a1: usize) -> r_efi::base::Status {
    eprintln!("fake_tables: unimplemented slot called: free_pages");
    efi::Status::UNSUPPORTED
}

unsafe extern "efiapi" fn bs_get_memory_map(
    _a0: *mut usize,
    _a1: *mut MemoryDescriptor,
    _a2: *mut usize,
    _a3: *mut usize,
    _a4: *mut u32,
) -> r_efi::base::Status {
    eprintln!("fake_tables: unimplemented slot called: get_memory_map");
    efi::Status::UNSUPPORTED
}

unsafe extern "efiapi" fn bs_allocate_pool(
    _a0: MemoryType,
    _a1: usize,
    _a2: *mut *mut core::ffi::c_void,
) -> r_efi::base::Status {
    eprintln!("fake_tables: unimplemented slot called: allocate_pool");
    efi::Status::UNSUPPORTED
}

unsafe extern "efiapi" fn bs_free_pool(_a0: *mut core::ffi::c_void) -> r_efi::base::Status {
    efi::Status::SUCCESS
}

unsafe extern "efiapi" fn bs_create_event(
    _a0: u32,
    _a1: r_efi::base::Tpl,
    _a2: Option<EventNotify>,
    _a3: *mut core::ffi::c_void,
    _a4: *mut r_efi::base::Event,
) -> r_efi::base::Status {
    // Hand back a dangling non-null event token.
    // SAFETY: out-pointers come from the wrapped table caller per the UEFI
    // contract; writes are null-checked or spec-required single writes.
    unsafe {
        *_a4 = 0x1000 as efi::Event;
    }
    efi::Status::SUCCESS
}

unsafe extern "efiapi" fn bs_set_timer(_a0: r_efi::base::Event, _a1: TimerDelay, _a2: u64) -> r_efi::base::Status {
    eprintln!("fake_tables: unimplemented slot called: set_timer");
    efi::Status::UNSUPPORTED
}

unsafe extern "efiapi" fn bs_wait_for_event(
    _a0: usize,
    _a1: *mut r_efi::base::Event,
    _a2: *mut usize,
) -> r_efi::base::Status {
    eprintln!("fake_tables: unimplemented slot called: wait_for_event");
    efi::Status::UNSUPPORTED
}

unsafe extern "efiapi" fn bs_signal_event(_a0: r_efi::base::Event) -> r_efi::base::Status {
    efi::Status::SUCCESS
}

unsafe extern "efiapi" fn bs_close_event(_a0: r_efi::base::Event) -> r_efi::base::Status {
    efi::Status::SUCCESS
}

unsafe extern "efiapi" fn bs_check_event(_a0: r_efi::base::Event) -> r_efi::base::Status {
    eprintln!("fake_tables: unimplemented slot called: check_event");
    efi::Status::UNSUPPORTED
}

unsafe extern "efiapi" fn bs_install_protocol_interface(
    _a0: *mut r_efi::base::Handle,
    _a1: *mut r_efi::base::Guid,
    _a2: InterfaceType,
    _a3: *mut core::ffi::c_void,
) -> r_efi::base::Status {
    eprintln!("fake_tables: unimplemented slot called: install_protocol_interface");
    efi::Status::UNSUPPORTED
}

unsafe extern "efiapi" fn bs_reinstall_protocol_interface(
    _a0: r_efi::base::Handle,
    _a1: *mut r_efi::base::Guid,
    _a2: *mut core::ffi::c_void,
    _a3: *mut core::ffi::c_void,
) -> r_efi::base::Status {
    eprintln!("fake_tables: unimplemented slot called: reinstall_protocol_interface");
    efi::Status::UNSUPPORTED
}

unsafe extern "efiapi" fn bs_uninstall_protocol_interface(
    _a0: r_efi::base::Handle,
    _a1: *mut r_efi::base::Guid,
    _a2: *mut core::ffi::c_void,
) -> r_efi::base::Status {
    eprintln!("fake_tables: unimplemented slot called: uninstall_protocol_interface");
    efi::Status::UNSUPPORTED
}

unsafe extern "efiapi" fn bs_handle_protocol(
    _a0: r_efi::base::Handle,
    _a1: *mut r_efi::base::Guid,
    _a2: *mut *mut core::ffi::c_void,
) -> r_efi::base::Status {
    eprintln!("fake_tables: unimplemented slot called: handle_protocol");
    efi::Status::UNSUPPORTED
}

unsafe extern "efiapi" fn bs_register_protocol_notify(
    _a0: *mut r_efi::base::Guid,
    _a1: r_efi::base::Event,
    _a2: *mut *mut core::ffi::c_void,
) -> r_efi::base::Status {
    eprintln!("fake_tables: unimplemented slot called: register_protocol_notify");
    efi::Status::UNSUPPORTED
}

unsafe extern "efiapi" fn bs_locate_handle(
    _a0: LocateSearchType,
    _a1: *mut r_efi::base::Guid,
    _a2: *mut core::ffi::c_void,
    _a3: *mut usize,
    _a4: *mut r_efi::base::Handle,
) -> r_efi::base::Status {
    eprintln!("fake_tables: unimplemented slot called: locate_handle");
    efi::Status::UNSUPPORTED
}

unsafe extern "efiapi" fn bs_locate_device_path(
    _a0: *mut r_efi::base::Guid,
    _a1: *mut *mut r_efi::protocols::device_path::Protocol,
    _a2: *mut r_efi::base::Handle,
) -> r_efi::base::Status {
    eprintln!("fake_tables: unimplemented slot called: locate_device_path");
    efi::Status::UNSUPPORTED
}

unsafe extern "efiapi" fn bs_install_configuration_table(
    _a0: *mut r_efi::base::Guid,
    _a1: *mut core::ffi::c_void,
) -> r_efi::base::Status {
    eprintln!("fake_tables: unimplemented slot called: install_configuration_table");
    efi::Status::UNSUPPORTED
}

unsafe extern "efiapi" fn bs_load_image(
    _a0: r_efi::base::Boolean,
    _a1: r_efi::base::Handle,
    _a2: *mut r_efi::protocols::device_path::Protocol,
    _a3: *mut core::ffi::c_void,
    _a4: usize,
    _a5: *mut r_efi::base::Handle,
) -> r_efi::base::Status {
    // Every boot candidate is absent.
    efi::Status::NOT_FOUND
}

unsafe extern "efiapi" fn bs_start_image(
    _a0: r_efi::base::Handle,
    _a1: *mut usize,
    _a2: *mut *mut r_efi::base::Char16,
) -> r_efi::base::Status {
    eprintln!("fake_tables: unimplemented slot called: start_image");
    efi::Status::UNSUPPORTED
}

unsafe extern "efiapi" fn bs_exit(
    _a0: r_efi::base::Handle,
    _a1: r_efi::base::Status,
    _a2: usize,
    _a3: *mut r_efi::base::Char16,
) -> r_efi::base::Status {
    eprintln!("fake_tables: unimplemented slot called: exit");
    efi::Status::UNSUPPORTED
}

unsafe extern "efiapi" fn bs_unload_image(_a0: r_efi::base::Handle) -> r_efi::base::Status {
    eprintln!("fake_tables: unimplemented slot called: unload_image");
    efi::Status::UNSUPPORTED
}

unsafe extern "efiapi" fn bs_exit_boot_services(_a0: r_efi::base::Handle, _a1: usize) -> r_efi::base::Status {
    eprintln!("fake_tables: unimplemented slot called: exit_boot_services");
    efi::Status::UNSUPPORTED
}

unsafe extern "efiapi" fn bs_get_next_monotonic_count(_a0: *mut u64) -> r_efi::base::Status {
    eprintln!("fake_tables: unimplemented slot called: get_next_monotonic_count");
    efi::Status::UNSUPPORTED
}

unsafe extern "efiapi" fn bs_stall(_a0: usize) -> r_efi::base::Status {
    eprintln!("fake_tables: unimplemented slot called: stall");
    efi::Status::UNSUPPORTED
}

unsafe extern "efiapi" fn bs_set_watchdog_timer(
    _a0: usize,
    _a1: u64,
    _a2: usize,
    _a3: *mut r_efi::base::Char16,
) -> r_efi::base::Status {
    efi::Status::SUCCESS
}

unsafe extern "efiapi" fn bs_connect_controller(
    _a0: r_efi::base::Handle,
    _a1: *mut r_efi::base::Handle,
    _a2: *mut r_efi::protocols::device_path::Protocol,
    _a3: r_efi::base::Boolean,
) -> r_efi::base::Status {
    efi::Status::SUCCESS
}

unsafe extern "efiapi" fn bs_disconnect_controller(
    _a0: r_efi::base::Handle,
    _a1: r_efi::base::Handle,
    _a2: r_efi::base::Handle,
) -> r_efi::base::Status {
    eprintln!("fake_tables: unimplemented slot called: disconnect_controller");
    efi::Status::UNSUPPORTED
}

unsafe extern "efiapi" fn bs_open_protocol(
    _a0: r_efi::base::Handle,
    _a1: *mut r_efi::base::Guid,
    _a2: *mut *mut core::ffi::c_void,
    _a3: r_efi::base::Handle,
    _a4: r_efi::base::Handle,
    _a5: u32,
) -> r_efi::base::Status {
    eprintln!("fake_tables: unimplemented slot called: open_protocol");
    efi::Status::UNSUPPORTED
}

unsafe extern "efiapi" fn bs_close_protocol(
    _a0: r_efi::base::Handle,
    _a1: *mut r_efi::base::Guid,
    _a2: r_efi::base::Handle,
    _a3: r_efi::base::Handle,
) -> r_efi::base::Status {
    eprintln!("fake_tables: unimplemented slot called: close_protocol");
    efi::Status::UNSUPPORTED
}

unsafe extern "efiapi" fn bs_open_protocol_information(
    _a0: r_efi::base::Handle,
    _a1: *mut r_efi::base::Guid,
    _a2: *mut *mut OpenProtocolInformationEntry,
    _a3: *mut usize,
) -> r_efi::base::Status {
    eprintln!("fake_tables: unimplemented slot called: open_protocol_information");
    efi::Status::UNSUPPORTED
}

unsafe extern "efiapi" fn bs_protocols_per_handle(
    _a0: r_efi::base::Handle,
    _a1: *mut *mut *mut r_efi::base::Guid,
    _a2: *mut usize,
) -> r_efi::base::Status {
    eprintln!("fake_tables: unimplemented slot called: protocols_per_handle");
    efi::Status::UNSUPPORTED
}

unsafe extern "efiapi" fn bs_locate_handle_buffer(
    _a0: LocateSearchType,
    _a1: *mut r_efi::base::Guid,
    _a2: *mut core::ffi::c_void,
    _a3: *mut usize,
    _a4: *mut *mut r_efi::base::Handle,
) -> r_efi::base::Status {
    // No handles: connect/dispatch converges after one pass and discovery finds
    // no volumes. SUCCESS ensures the orchestrator exercises that loop.
    // SAFETY: out-pointers come from the wrapped table caller per the UEFI
    // contract. A non-null dangling pointer is required because the wrapper
    // constructs a zero-length Rust slice from this successful result.
    unsafe {
        if !_a3.is_null() {
            *_a3 = 0;
        }
        if !_a4.is_null() {
            *_a4 = core::ptr::NonNull::<r_efi::base::Handle>::dangling().as_ptr();
        }
    }
    efi::Status::SUCCESS
}

unsafe extern "efiapi" fn bs_locate_protocol(
    _a0: *mut r_efi::base::Guid,
    _a1: *mut core::ffi::c_void,
    _a2: *mut *mut core::ffi::c_void,
) -> r_efi::base::Status {
    eprintln!("fake_tables: unimplemented slot called: locate_protocol");
    efi::Status::UNSUPPORTED
}

unsafe extern "efiapi" fn bs_install_multiple_protocol_interfaces(
    _a0: *mut r_efi::base::Handle,
    _a1: *mut core::ffi::c_void,
    _a2: *mut core::ffi::c_void,
) -> r_efi::base::Status {
    eprintln!("fake_tables: unimplemented slot called: install_multiple_protocol_interfaces");
    efi::Status::UNSUPPORTED
}

unsafe extern "efiapi" fn bs_uninstall_multiple_protocol_interfaces(
    _a0: r_efi::base::Handle,
    _a1: *mut core::ffi::c_void,
    _a2: *mut core::ffi::c_void,
) -> r_efi::base::Status {
    eprintln!("fake_tables: unimplemented slot called: uninstall_multiple_protocol_interfaces");
    efi::Status::UNSUPPORTED
}

unsafe extern "efiapi" fn bs_calculate_crc32(
    _a0: *mut core::ffi::c_void,
    _a1: usize,
    _a2: *mut u32,
) -> r_efi::base::Status {
    eprintln!("fake_tables: unimplemented slot called: calculate_crc32");
    efi::Status::UNSUPPORTED
}

unsafe extern "efiapi" fn bs_copy_mem(_a0: *mut core::ffi::c_void, _a1: *mut core::ffi::c_void, _a2: usize) {
    // no-op
}

unsafe extern "efiapi" fn bs_set_mem(_a0: *mut core::ffi::c_void, _a1: usize, _a2: u8) {
    // no-op
}

unsafe extern "efiapi" fn bs_create_event_ex(
    _a0: u32,
    _a1: r_efi::base::Tpl,
    _a2: Option<EventNotify>,
    _a3: *const core::ffi::c_void,
    _a4: *const r_efi::base::Guid,
    _a5: *mut r_efi::base::Event,
) -> r_efi::base::Status {
    // Hand back a dangling non-null event token.
    // SAFETY: out-pointers come from the wrapped table caller per the UEFI
    // contract; writes are null-checked or spec-required single writes.
    unsafe {
        *_a5 = 0x1000 as efi::Event;
    }
    efi::Status::SUCCESS
}

unsafe extern "efiapi" fn rt_get_time(_a0: *mut Time, _a1: *mut TimeCapabilities) -> r_efi::base::Status {
    eprintln!("fake_tables: unimplemented slot called: get_time");
    efi::Status::UNSUPPORTED
}

unsafe extern "efiapi" fn rt_set_time(_a0: *mut Time) -> r_efi::base::Status {
    eprintln!("fake_tables: unimplemented slot called: set_time");
    efi::Status::UNSUPPORTED
}

unsafe extern "efiapi" fn rt_get_wakeup_time(
    _a0: *mut r_efi::base::Boolean,
    _a1: *mut r_efi::base::Boolean,
    _a2: *mut Time,
) -> r_efi::base::Status {
    eprintln!("fake_tables: unimplemented slot called: get_wakeup_time");
    efi::Status::UNSUPPORTED
}

unsafe extern "efiapi" fn rt_set_wakeup_time(_a0: r_efi::base::Boolean, _a1: *mut Time) -> r_efi::base::Status {
    eprintln!("fake_tables: unimplemented slot called: set_wakeup_time");
    efi::Status::UNSUPPORTED
}

unsafe extern "efiapi" fn rt_set_virtual_address_map(
    _a0: usize,
    _a1: usize,
    _a2: u32,
    _a3: *mut MemoryDescriptor,
) -> r_efi::base::Status {
    eprintln!("fake_tables: unimplemented slot called: set_virtual_address_map");
    efi::Status::UNSUPPORTED
}

unsafe extern "efiapi" fn rt_convert_pointer(_a0: usize, _a1: *mut *mut core::ffi::c_void) -> r_efi::base::Status {
    eprintln!("fake_tables: unimplemented slot called: convert_pointer");
    efi::Status::UNSUPPORTED
}

unsafe extern "efiapi" fn rt_get_variable(
    _a0: *mut r_efi::base::Char16,
    _a1: *mut r_efi::base::Guid,
    _a2: *mut u32,
    _a3: *mut usize,
    _a4: *mut core::ffi::c_void,
) -> r_efi::base::Status {
    // No BootOrder / Boot#### variables exist.
    efi::Status::NOT_FOUND
}

unsafe extern "efiapi" fn rt_get_next_variable_name(
    _a0: *mut usize,
    _a1: *mut r_efi::base::Char16,
    _a2: *mut r_efi::base::Guid,
) -> r_efi::base::Status {
    eprintln!("fake_tables: unimplemented slot called: get_next_variable_name");
    efi::Status::UNSUPPORTED
}

unsafe extern "efiapi" fn rt_set_variable(
    _a0: *mut r_efi::base::Char16,
    _a1: *mut r_efi::base::Guid,
    _a2: u32,
    _a3: usize,
    _a4: *mut core::ffi::c_void,
) -> r_efi::base::Status {
    efi::Status::UNSUPPORTED
}

unsafe extern "efiapi" fn rt_get_next_high_mono_count(_a0: *mut u32) -> r_efi::base::Status {
    eprintln!("fake_tables: unimplemented slot called: get_next_high_mono_count");
    efi::Status::UNSUPPORTED
}

unsafe extern "efiapi" fn rt_reset_system(
    _a0: ResetType,
    _a1: r_efi::base::Status,
    _a2: usize,
    _a3: *mut core::ffi::c_void,
) {
    // no-op
}

unsafe extern "efiapi" fn rt_update_capsule(
    _a0: *mut *mut CapsuleHeader,
    _a1: usize,
    _a2: r_efi::base::PhysicalAddress,
) -> r_efi::base::Status {
    eprintln!("fake_tables: unimplemented slot called: update_capsule");
    efi::Status::UNSUPPORTED
}

unsafe extern "efiapi" fn rt_query_capsule_capabilities(
    _a0: *mut *mut CapsuleHeader,
    _a1: usize,
    _a2: *mut u64,
    _a3: *mut ResetType,
) -> r_efi::base::Status {
    eprintln!("fake_tables: unimplemented slot called: query_capsule_capabilities");
    efi::Status::UNSUPPORTED
}

unsafe extern "efiapi" fn rt_query_variable_info(
    _a0: u32,
    _a1: *mut u64,
    _a2: *mut u64,
    _a3: *mut u64,
) -> r_efi::base::Status {
    eprintln!("fake_tables: unimplemented slot called: query_variable_info");
    efi::Status::UNSUPPORTED
}

pub fn mock_boot_services() -> Box<efi::BootServices> {
    Box::new(efi::BootServices {
        hdr: zero_header(),
        reserved: core::ptr::null_mut(),
        raise_tpl: bs_raise_tpl,
        restore_tpl: bs_restore_tpl,
        allocate_pages: bs_allocate_pages,
        free_pages: bs_free_pages,
        get_memory_map: bs_get_memory_map,
        allocate_pool: bs_allocate_pool,
        free_pool: bs_free_pool,
        create_event: bs_create_event,
        set_timer: bs_set_timer,
        wait_for_event: bs_wait_for_event,
        signal_event: bs_signal_event,
        close_event: bs_close_event,
        check_event: bs_check_event,
        install_protocol_interface: bs_install_protocol_interface,
        reinstall_protocol_interface: bs_reinstall_protocol_interface,
        uninstall_protocol_interface: bs_uninstall_protocol_interface,
        handle_protocol: bs_handle_protocol,
        register_protocol_notify: bs_register_protocol_notify,
        locate_handle: bs_locate_handle,
        locate_device_path: bs_locate_device_path,
        install_configuration_table: bs_install_configuration_table,
        load_image: bs_load_image,
        start_image: bs_start_image,
        exit: bs_exit,
        unload_image: bs_unload_image,
        exit_boot_services: bs_exit_boot_services,
        get_next_monotonic_count: bs_get_next_monotonic_count,
        stall: bs_stall,
        set_watchdog_timer: bs_set_watchdog_timer,
        connect_controller: bs_connect_controller,
        disconnect_controller: bs_disconnect_controller,
        open_protocol: bs_open_protocol,
        close_protocol: bs_close_protocol,
        open_protocol_information: bs_open_protocol_information,
        protocols_per_handle: bs_protocols_per_handle,
        locate_handle_buffer: bs_locate_handle_buffer,
        locate_protocol: bs_locate_protocol,
        install_multiple_protocol_interfaces: bs_install_multiple_protocol_interfaces,
        uninstall_multiple_protocol_interfaces: bs_uninstall_multiple_protocol_interfaces,
        calculate_crc32: bs_calculate_crc32,
        copy_mem: bs_copy_mem,
        set_mem: bs_set_mem,
        create_event_ex: bs_create_event_ex,
    })
}

pub fn mock_runtime_services() -> Box<efi::RuntimeServices> {
    Box::new(efi::RuntimeServices {
        hdr: zero_header(),
        get_time: rt_get_time,
        set_time: rt_set_time,
        get_wakeup_time: rt_get_wakeup_time,
        set_wakeup_time: rt_set_wakeup_time,
        set_virtual_address_map: rt_set_virtual_address_map,
        convert_pointer: rt_convert_pointer,
        get_variable: rt_get_variable,
        get_next_variable_name: rt_get_next_variable_name,
        set_variable: rt_set_variable,
        get_next_high_mono_count: rt_get_next_high_mono_count,
        reset_system: rt_reset_system,
        update_capsule: rt_update_capsule,
        query_capsule_capabilities: rt_query_capsule_capabilities,
        query_variable_info: rt_query_variable_info,
    })
}
