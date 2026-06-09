//! SRE recovery boot path: read the recovery payload from NVMe Boot
//! Partition 1 via Get Log Page LID=0x15, register it as a RAM disk via
//! EFI_RAM_DISK_PROTOCOL, and chainload \EFI\Boot\bootx64.efi from the
//! resulting FAT volume.
//!
//! Hotkey detection uses MS_BUTTON_SERVICES_PROTOCOL (Surface platforms
//! publish this via the SAM button driver). The Vol-Up + Power chord at
//! power-on selects the SRE path; absence falls through to the normal
//! boot orchestration.
//!
//! This module inlines FFI for three protocols rather than depending on
//! helpers that may not be published yet:
//!   - EFI_NVM_EXPRESS_PASS_THRU_PROTOCOL (for Identify + Get Log Page)
//!   - EFI_RAM_DISK_PROTOCOL (UEFI 2.5+)
//!   - MS_BUTTON_SERVICES_PROTOCOL (Surface/Microsoft)
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: MIT
//!
extern crate alloc;

use alloc::vec;
use alloc::vec::Vec;
use core::{ffi::c_void, mem::MaybeUninit, ptr};

use patina::{boot_services::BootServices, error::EfiError};
use patina_boot::helpers;
use r_efi::efi;

/// Target boot partition for the SRE WIM payload.
const TARGET_BPID: u8 = 1;

/// Fixed BP size on platforms in scope (BPINFO.BPSZ * 128 KiB = 1 GiB).
const BPSIZE_BYTES: usize = 1024 * 1024 * 1024;

/// LID 0x15 response prepends a 16-byte header before the BP image bytes.
const LID_BP_HEADER_BYTES: u64 = 16;

/// Lower bound for chunked LID 0x15 reads.
const READ_CHUNK_MIN: usize = 64 * 1024;
/// Upper bound for chunked LID 0x15 reads (also the EDK2 NvmExpressPassThru
/// observed cap on platforms in scope).
const READ_CHUNK_MAX: usize = 512 * 1024;
/// Used when the MDTS probe fails.
const READ_CHUNK_DEFAULT: usize = 64 * 1024;

/// Path chainloaded from the registered RAM disk's FAT volume.
const CHAINLOAD_FILE_PATH: &str = "\\EFI\\Boot\\bootx64.efi";

/// EFI_NVM_EXPRESS_PASS_THRU_PROTOCOL FFI.
mod nvme_pass_thru {
    use core::ffi::c_void;
    use r_efi::efi;

    pub const PROTOCOL_GUID: efi::Guid =
        efi::Guid::from_fields(0x52c78312, 0x8edc, 0x4233, 0x98, 0xf2, &[0x1a, 0x1a, 0xa5, 0xe3, 0x88, 0xa5]);

    pub const OPCODE_GET_LOG_PAGE: u8 = 0x02;
    pub const OPCODE_IDENTIFY: u8 = 0x06;

    pub const IDENTIFY_CNS_CONTROLLER: u32 = 0x01;
    pub const IDENTIFY_BUFFER_BYTES: usize = 4096;
    pub const ID_CTRL_OFFSET_MDTS: usize = 77;

    pub const LID_BOOT_PARTITION: u32 = 0x15;

    pub const CMD_FLAG_CDW10_VALID: u8 = 1 << 2;
    pub const CMD_FLAG_CDW11_VALID: u8 = 1 << 3;
    pub const CMD_FLAG_CDW12_VALID: u8 = 1 << 4;
    pub const CMD_FLAG_CDW13_VALID: u8 = 1 << 5;

    pub const QUEUE_TYPE_ADMIN: u8 = 0;

    /// Timeouts in 100-ns units.
    pub const TIMEOUT_NS_5_SEC: u64 = 50_000_000;
    pub const TIMEOUT_NS_10_SEC: u64 = 100_000_000;

    pub type PassThruFn = extern "efiapi" fn(
        this: *mut Protocol,
        namespace_id: u32,
        packet: *mut CommandPacket,
        event: *mut c_void,
    ) -> efi::Status;

    #[repr(C)]
    pub struct Protocol {
        pub mode: *mut c_void,
        pub pass_thru: PassThruFn,
        pub get_next_namespace: *mut c_void,
        pub build_device_path: *mut c_void,
        pub get_namespace: *mut c_void,
    }

    #[repr(C)]
    pub struct CommandPacket {
        pub command_timeout: u64,
        pub transfer_buffer: *mut c_void,
        pub transfer_length: u32,
        pub metadata_buffer: *mut c_void,
        pub metadata_length: u32,
        pub queue_type: u8,
        pub nvme_cmd: *mut Command,
        pub nvme_completion: *mut Completion,
    }

    #[repr(C)]
    #[derive(Copy, Clone, Default)]
    pub struct Command {
        pub cdw0: u32,
        pub flags: u8,
        pub nsid: u32,
        pub cdw2: u32,
        pub cdw3: u32,
        pub cdw10: u32,
        pub cdw11: u32,
        pub cdw12: u32,
        pub cdw13: u32,
        pub cdw14: u32,
        pub cdw15: u32,
    }

    #[repr(C)]
    #[derive(Copy, Clone, Default)]
    pub struct Completion {
        pub dw0: u32,
        pub dw1: u32,
        pub dw2: u32,
        pub dw3: u32,
    }
}

/// EFI_RAM_DISK_PROTOCOL FFI (UEFI 2.5+).
mod ram_disk {
    use core::ffi::c_void;
    use r_efi::efi;

    /// EFI_RAM_DISK_PROTOCOL_GUID
    pub const PROTOCOL_GUID: efi::Guid =
        efi::Guid::from_fields(0xab38a0df, 0x6873, 0x44a9, 0x87, 0xe6, &[0xd4, 0xeb, 0x56, 0x14, 0x86, 0x49]);

    /// EFI_VIRTUAL_DISK_GUID — generic virtual disk type used for raw RAM disks.
    pub const VIRTUAL_DISK_GUID: efi::Guid =
        efi::Guid::from_fields(0x77ab535a, 0x45fc, 0x624b, 0x55, 0x60, &[0xf7, 0xb2, 0x81, 0xd1, 0xf9, 0x6e]);

    pub type RegisterFn = extern "efiapi" fn(
        ram_disk_base: u64,
        ram_disk_size: u64,
        ram_disk_type: *const efi::Guid,
        parent_device_path: *const c_void,
        device_path: *mut *const c_void,
    ) -> efi::Status;

    pub type UnregisterFn = extern "efiapi" fn(device_path: *const c_void) -> efi::Status;

    #[repr(C)]
    pub struct Protocol {
        pub register: RegisterFn,
        pub unregister: UnregisterFn,
    }
}

/// MS_BUTTON_SERVICES_PROTOCOL FFI (Surface platform button service).
mod button_services {
    use core::ffi::c_void;
    use r_efi::efi;

    /// MS_BUTTON_SERVICES_PROTOCOL_GUID
    /// {7E057B73-A18D-4B19-86F3-30C5A6F167CE}
    pub const PROTOCOL_GUID: efi::Guid =
        efi::Guid::from_fields(0x7e057b73, 0xa18d, 0x4b19, 0x86, 0xf3, &[0x30, 0xc5, 0xa6, 0xf1, 0x67, 0xce]);

    pub type ButtonCheckFn =
        extern "efiapi" fn(this: *mut Protocol, button_state: *mut efi::Boolean) -> efi::Status;
    pub type ClearStateFn = extern "efiapi" fn(this: *mut Protocol) -> efi::Status;

    #[repr(C)]
    pub struct Protocol {
        pub pre_boot_volume_up_check: ButtonCheckFn,
        pub pre_boot_volume_down_check: ButtonCheckFn,
        pub pre_boot_clear_state: ClearStateFn,
        // Surface adds more entries; we only need these three.
        pub _reserved: *mut c_void,
    }
}

/// Returns `true` if the Vol-Up + Power chord was registered at power-on.
///
/// Reads via MS_BUTTON_SERVICES_PROTOCOL. Returns `false` if the protocol is
/// absent (platform doesn't publish a SAM button service).
pub fn detect_sre_hotkey<B: BootServices>(boot_services: &B) -> bool {
    // SAFETY: dereferencing the returned interface only via raw pointer calls below.
    let protocol = match unsafe {
        boot_services.locate_protocol_unchecked(&button_services::PROTOCOL_GUID, ptr::null_mut())
    } {
        Ok(p) => p as *mut button_services::Protocol,
        Err(_) => {
            log::info!("detect_sre_hotkey: MS_BUTTON_SERVICES_PROTOCOL not present");
            return false;
        }
    };

    // SAFETY: protocol pointer returned by locate_protocol for the button service GUID.
    unsafe { detect_sre_hotkey_via_protocol(protocol) }
}

/// Inner detect_sre_hotkey that takes a raw protocol pointer. Separated so
/// tests can pass a synthetic protocol with controlled callbacks.
///
/// # Safety
///
/// `protocol` must be a valid, non-null pointer to an
/// `MS_BUTTON_SERVICES_PROTOCOL` instance owned by the platform.
unsafe fn detect_sre_hotkey_via_protocol(protocol: *mut button_services::Protocol) -> bool {
    let mut state: efi::Boolean = efi::Boolean::FALSE;
    // SAFETY: caller guarantees protocol pointer is valid.
    let status = unsafe { ((*protocol).pre_boot_volume_up_check)(protocol, &mut state) };

    // Clear so other consumers don't double-act on the same press.
    // SAFETY: caller guarantees protocol pointer is valid.
    let _ = unsafe { ((*protocol).pre_boot_clear_state)(protocol) };

    status == efi::Status::SUCCESS && bool::from(state)
}

/// Probe controller MDTS via Identify Controller (CNS=0x01); derive the
/// largest LID 0x15 chunk size we'll use. Falls back to `READ_CHUNK_DEFAULT`
/// on probe failure. Clamps to `[READ_CHUNK_MIN, READ_CHUNK_MAX]`.
fn probe_max_transfer(passthru: *mut nvme_pass_thru::Protocol) -> usize {
    let mut buf = vec![0u8; nvme_pass_thru::IDENTIFY_BUFFER_BYTES];
    let mut cmd = nvme_pass_thru::Command::default();
    cmd.cdw0 = nvme_pass_thru::OPCODE_IDENTIFY as u32;
    cmd.cdw10 = nvme_pass_thru::IDENTIFY_CNS_CONTROLLER;
    cmd.flags = nvme_pass_thru::CMD_FLAG_CDW10_VALID;

    let mut completion = nvme_pass_thru::Completion::default();

    let mut packet = nvme_pass_thru::CommandPacket {
        command_timeout: nvme_pass_thru::TIMEOUT_NS_5_SEC,
        transfer_buffer: buf.as_mut_ptr() as *mut c_void,
        transfer_length: buf.len() as u32,
        metadata_buffer: ptr::null_mut(),
        metadata_length: 0,
        queue_type: nvme_pass_thru::QUEUE_TYPE_ADMIN,
        nvme_cmd: &mut cmd,
        nvme_completion: &mut completion,
    };

    // SAFETY: passthru pointer is non-null and points to a valid protocol
    // instance owned by the controller; buffers in `packet` live for the call.
    let status = unsafe { ((*passthru).pass_thru)(passthru, 0, &mut packet, ptr::null_mut()) };
    if status != efi::Status::SUCCESS {
        log::warn!("Identify Controller failed: {:?}; falling back to {} KiB chunk", status, READ_CHUNK_DEFAULT / 1024);
        return READ_CHUNK_DEFAULT;
    }

    let mdts = buf[nvme_pass_thru::ID_CTRL_OFFSET_MDTS];
    let chunk = if mdts == 0 {
        READ_CHUNK_MAX
    } else {
        let max_transfer = (1usize << mdts) * 4096;
        max_transfer.clamp(READ_CHUNK_MIN, READ_CHUNK_MAX)
    };

    log::info!("Identify Controller: MDTS={} -> chunk {} KiB", mdts, chunk / 1024);
    chunk
}

/// Issue Get Log Page LID=0x15 in chunked reads totalling `BPSIZE_BYTES`,
/// landing pure BP image bytes (header stripped via LPOL offset) into `dest`.
fn read_bp_via_log_page(
    passthru: *mut nvme_pass_thru::Protocol,
    bp_id: u8,
    chunk_bytes: usize,
    dest: &mut [u8],
) -> Result<(), EfiError> {
    let mut bp_off: u64 = 0;
    while (bp_off as usize) < dest.len() {
        let remaining = dest.len() - bp_off as usize;
        let read_bytes = chunk_bytes.min(remaining);
        let lpol = LID_BP_HEADER_BYTES + bp_off;

        let numd = ((read_bytes / 4) - 1) as u32;
        let mut cmd = nvme_pass_thru::Command::default();
        cmd.cdw0 = nvme_pass_thru::OPCODE_GET_LOG_PAGE as u32;
        cmd.cdw10 = ((numd & 0xFFFF) << 16) | (((bp_id as u32) & 0x7F) << 8) | nvme_pass_thru::LID_BOOT_PARTITION;
        cmd.cdw11 = (numd >> 16) & 0xFFFF;
        cmd.cdw12 = (lpol & 0xFFFFFFFF) as u32;
        cmd.cdw13 = ((lpol >> 32) & 0xFFFFFFFF) as u32;
        cmd.flags = nvme_pass_thru::CMD_FLAG_CDW10_VALID
            | nvme_pass_thru::CMD_FLAG_CDW11_VALID
            | nvme_pass_thru::CMD_FLAG_CDW12_VALID
            | nvme_pass_thru::CMD_FLAG_CDW13_VALID;

        let mut completion = nvme_pass_thru::Completion::default();

        let slice = &mut dest[bp_off as usize..bp_off as usize + read_bytes];
        let mut packet = nvme_pass_thru::CommandPacket {
            command_timeout: nvme_pass_thru::TIMEOUT_NS_10_SEC,
            transfer_buffer: slice.as_mut_ptr() as *mut c_void,
            transfer_length: read_bytes as u32,
            metadata_buffer: ptr::null_mut(),
            metadata_length: 0,
            queue_type: nvme_pass_thru::QUEUE_TYPE_ADMIN,
            nvme_cmd: &mut cmd,
            nvme_completion: &mut completion,
        };

        // SAFETY: passthru valid, packet buffer slice lives for the call.
        let status = unsafe { ((*passthru).pass_thru)(passthru, 0, &mut packet, ptr::null_mut()) };
        if status != efi::Status::SUCCESS {
            log::error!(
                "Get Log Page LID=0x15 at LPOL={} ({} bytes) failed: {:?}",
                lpol, read_bytes, status
            );
            return Err(EfiError::from(status));
        }

        bp_off += read_bytes as u64;
        if (bp_off & ((16u64 * 1024 * 1024) - 1)) == 0 {
            log::info!("BP read progress: {} MiB", bp_off / (1024 * 1024));
        }
    }

    Ok(())
}

/// Register `dram` (a contiguous allocation of `BPSIZE_BYTES` holding the
/// BP image) as a RAM disk and return the protocol-issued device path.
fn register_ram_disk<B: BootServices>(
    boot_services: &B,
    dram: &[u8],
) -> Result<*const c_void, EfiError> {
    // SAFETY: dereferencing the returned interface only via raw pointer calls below.
    let protocol = unsafe {
        boot_services.locate_protocol_unchecked(&ram_disk::PROTOCOL_GUID, ptr::null_mut())
    }
    .map_err(|e| {
        log::error!("LocateProtocol(RamDisk): {:?} (platform missing RamDiskDxe?)", e);
        EfiError::from(e)
    })? as *mut ram_disk::Protocol;

    // SAFETY: protocol non-null; dram lives for the duration of this boot.
    unsafe { register_ram_disk_via_protocol(protocol, dram) }
}

/// Inner register_ram_disk that takes a raw protocol pointer. Separated so
/// tests can pass a synthetic protocol whose register callback captures the
/// arguments.
///
/// # Safety
///
/// `protocol` must be a valid, non-null pointer to an `EFI_RAM_DISK_PROTOCOL`
/// instance, and `dram` must remain live until the chainloaded OS takes
/// ownership at ExitBootServices.
unsafe fn register_ram_disk_via_protocol(
    protocol: *mut ram_disk::Protocol,
    dram: &[u8],
) -> Result<*const c_void, EfiError> {
    let mut new_dp: *const c_void = ptr::null();
    // SAFETY: caller guarantees protocol pointer is valid.
    let status = unsafe {
        ((*protocol).register)(
            dram.as_ptr() as u64,
            dram.len() as u64,
            &ram_disk::VIRTUAL_DISK_GUID,
            ptr::null(),
            &mut new_dp,
        )
    };
    if status != efi::Status::SUCCESS {
        log::error!("RamDisk->Register: {:?}", status);
        return Err(EfiError::from(status));
    }
    log::info!("RAM disk registered ({} MiB at {:p})", dram.len() / (1024 * 1024), dram.as_ptr());
    Ok(new_dp)
}

/// Compute length of a device path in bytes (walks until END_ENTIRE).
unsafe fn device_path_size(dp: *const u8) -> usize {
    let mut p = dp;
    loop {
        // SAFETY: caller guarantees `dp` is a valid UEFI device-path.
        let dp_type = unsafe { *p };
        let dp_subtype = unsafe { *p.add(1) };
        let length = unsafe { (*p.add(2) as u16) | ((*p.add(3) as u16) << 8) } as usize;
        p = unsafe { p.add(length) };
        if dp_type == 0x7F && dp_subtype == 0xFF {
            return (p as usize) - (dp as usize);
        }
    }
}

/// Build a MEDIA_DEVICE_PATH/MEDIA_FILEPATH_DP node followed by END_ENTIRE.
/// Returns the raw bytes; caller is responsible for prepending the parent
/// device-path bytes when constructing a full path.
fn build_file_path_node(path: &str) -> Vec<u8> {
    // UTF-16 + null terminator.
    let mut utf16: Vec<u16> = path.encode_utf16().collect();
    utf16.push(0);
    let path_bytes = utf16.len() * 2;
    let node_len = 4 + path_bytes;

    let mut out = Vec::with_capacity(node_len + 4);
    // FilePath node: type=4 (MEDIA), subtype=4 (FILEPATH), length=node_len
    out.push(0x04);
    out.push(0x04);
    out.push((node_len & 0xFF) as u8);
    out.push(((node_len >> 8) & 0xFF) as u8);
    for w in &utf16 {
        out.push((w & 0xFF) as u8);
        out.push(((w >> 8) & 0xFF) as u8);
    }
    // END_ENTIRE: type=0x7F, subtype=0xFF, length=4
    out.push(0x7F);
    out.push(0xFF);
    out.push(0x04);
    out.push(0x00);
    out
}

/// Find the FAT SimpleFileSystem handle rooted at `parent_dp`, then chainload
/// `\EFI\Boot\bootx64.efi` from it via `helpers::boot_from_device_path`.
fn chainload_from_ramdisk<B: BootServices>(
    boot_services: &B,
    image_handle: efi::Handle,
    parent_dp: *const c_void,
) -> Result<(), EfiError> {
    // Connect the RAM disk handle so PartitionDxe + FAT bind.
    let mut remaining_dp = parent_dp as *mut efi::protocols::device_path::Protocol;
    let ram_handle = unsafe {
        boot_services.locate_device_path(&efi::protocols::device_path::PROTOCOL_GUID, &mut remaining_dp)
    }
    .map_err(EfiError::from)?;
    // SAFETY: ram_handle was just returned by locate_device_path; an empty
    // driver_image_handles list is permitted per the trait contract.
    let _ = unsafe { boot_services.connect_controller(ram_handle, Vec::new(), ptr::null_mut(), true) };

    // Build full boot device path: parent (RAM disk) + FilePath node + END.
    // SAFETY: parent_dp is a valid UEFI device path returned by the RAM disk
    // protocol; its size is bounded by the END node walk.
    let parent_size = unsafe { device_path_size(parent_dp as *const u8) };
    // Strip the parent's END node (last 4 bytes) before appending the file
    // path node + new END.
    let parent_payload_size = parent_size - 4;
    let file_node = build_file_path_node(CHAINLOAD_FILE_PATH);

    let mut full_path = Vec::with_capacity(parent_payload_size + file_node.len());
    // SAFETY: parent_dp + parent_payload_size is in-bounds per device_path_size.
    let parent_slice = unsafe { core::slice::from_raw_parts(parent_dp as *const u8, parent_payload_size) };
    full_path.extend_from_slice(parent_slice);
    full_path.extend_from_slice(&file_node);

    // Hand to boot_from_device_path via a DevicePathBuf reinterpretation.
    // patina's DevicePathBuf wraps the raw bytes with the END node terminator
    // already included; safe to construct from our well-formed sequence.
    let device_path_ptr = full_path.as_ptr() as *mut efi::protocols::device_path::Protocol;
    let new_image = boot_services
        .load_image(true, image_handle, device_path_ptr, None)
        .map_err(|status| {
            log::error!("LoadImage({}): {:?}", CHAINLOAD_FILE_PATH, status);
            EfiError::from(status)
        })?;

    log::info!("LoadImage OK; StartImage...");
    boot_services
        .start_image(new_image)
        .map_err(|(status, _exit_data)| {
            log::error!("StartImage: {:?}", status);
            EfiError::from(status)
        })
}

/// Run the SRE recovery flow end-to-end on Vol-Up hotkey:
///
///   1. ConnectAll (priority-boot dispatch runs before BdsConnectAll)
///   2. Locate NvmExpressPassThru
///   3. Probe MDTS, pick chunk size
///   4. Read BP1 via chunked LID 0x15 into a fresh DRAM allocation
///   5. Register the allocation as a RAM disk
///   6. Chainload \EFI\Boot\bootx64.efi from the FAT volume on the RAM disk
///
/// BP write protection (lock/unlock) is owned by the FMP capsule flow;
/// SRE boot is read-only against BPWPS.
pub fn run_sre_flow<B: BootServices>(
    boot_services: &B,
    image_handle: efi::Handle,
) -> Result<(), EfiError> {
    helpers::connect_all(boot_services).ok();

    // SAFETY: dereferencing the returned interface only via raw pointer calls below.
    let passthru = unsafe {
        boot_services.locate_protocol_unchecked(&nvme_pass_thru::PROTOCOL_GUID, ptr::null_mut())
    }
    .map_err(|e| {
        log::error!("LocateProtocol(NvmExpressPassThru): {:?}", e);
        EfiError::from(e)
    })? as *mut nvme_pass_thru::Protocol;

    let chunk = probe_max_transfer(passthru);

    // 1 GiB DRAM allocation for BP image. Vec keeps the lifetime tied to
    // this function's scope; the RAM-disk protocol borrows the pointer
    // and keeps the region live via its own bookkeeping while registered.
    // SAFETY note: handing a non-static-lifetime allocation to a UEFI
    // protocol is a deliberate hand-off; the chainloaded OS takes over
    // memory ownership when ExitBootServices fires.
    let mut buf: Vec<u8> = vec![0u8; BPSIZE_BYTES];
    log::info!("BP read: {} MiB via LID 0x15, chunk={} KiB", BPSIZE_BYTES / (1024 * 1024), chunk / 1024);
    read_bp_via_log_page(passthru, TARGET_BPID, chunk, &mut buf)?;
    log::info!("BP read complete ({} bytes)", buf.len());

    let ram_dp = register_ram_disk(boot_services, &buf)?;

    // The buffer's lifetime now belongs to the RAM disk + chainloaded OS.
    // Leak it so the Vec destructor doesn't free the backing pages out
    // from under the RAM disk before ExitBootServices.
    let _leaked = MaybeUninit::new(buf);
    core::mem::forget(_leaked);

    chainload_from_ramdisk(boot_services, image_handle, ram_dp)
}

#[cfg(test)]
mod tests {
    extern crate alloc;

    use super::*;
    use core::sync::atomic::{AtomicU32, AtomicU64, AtomicU8, AtomicUsize, Ordering};
    use patina::boot_services::MockBootServices;

    // === Pure function tests ===

    #[test]
    fn test_build_file_path_node_header_and_path_bytes() {
        let bytes = build_file_path_node("\\X");
        // Expected layout: type=0x04, subtype=0x04, length=4 + 2*3 = 10 (2 bytes LE);
        // UTF-16 LE: 0x005C 0x0058 0x0000; then END_ENTIRE: 0x7F 0xFF 0x04 0x00.
        let expected: &[u8] = &[
            0x04, 0x04, 0x0A, 0x00, // node header
            0x5C, 0x00, 0x58, 0x00, 0x00, 0x00, // UTF-16 "\X\0"
            0x7F, 0xFF, 0x04, 0x00, // END_ENTIRE
        ];
        assert_eq!(bytes.as_slice(), expected);
    }

    #[test]
    fn test_build_file_path_node_chainload_default() {
        let bytes = build_file_path_node(CHAINLOAD_FILE_PATH);
        // First two bytes must always be the FilePath node type/subtype.
        assert_eq!(bytes[0], 0x04, "MEDIA_DEVICE_PATH type");
        assert_eq!(bytes[1], 0x04, "MEDIA_FILEPATH_DP subtype");
        // Last four bytes must always be the END_ENTIRE node.
        let len = bytes.len();
        assert_eq!(&bytes[len - 4..], &[0x7F, 0xFF, 0x04, 0x00]);
    }

    #[test]
    fn test_device_path_size_walks_to_end_entire() {
        // Synthetic device path: one ACPI node (type=2, subtype=1, length=12) +
        // END_ENTIRE (type=0x7F, subtype=0xFF, length=4).
        let dp: [u8; 16] = [
            0x02, 0x01, 0x0C, 0x00, // ACPI node header
            0x41, 0xD0, 0x0A, 0x03, // HID
            0x00, 0x00, 0x00, 0x00, // UID
            0x7F, 0xFF, 0x04, 0x00, // END_ENTIRE
        ];
        // SAFETY: dp is a valid synthetic UEFI device path terminated by END_ENTIRE.
        let size = unsafe { device_path_size(dp.as_ptr()) };
        assert_eq!(size, 16);
    }

    #[test]
    fn test_device_path_size_end_entire_only() {
        let dp: [u8; 4] = [0x7F, 0xFF, 0x04, 0x00];
        // SAFETY: dp is a valid synthetic UEFI device path terminated by END_ENTIRE.
        let size = unsafe { device_path_size(dp.as_ptr()) };
        assert_eq!(size, 4);
    }

    // === detect_sre_hotkey ===

    #[test]
    fn test_detect_sre_hotkey_returns_false_when_protocol_absent() {
        let mut mock = MockBootServices::new();
        mock.expect_locate_protocol_unchecked().returning(|_, _| Err(efi::Status::NOT_FOUND));

        assert!(!detect_sre_hotkey(&mock), "absent button service must produce false");
    }

    // Inner-function tests use synthetic protocols whose function pointers
    // capture/control behavior, mirroring patina_boot::partition's pattern.

    static BUTTON_CHECK_CALLED: AtomicUsize = AtomicUsize::new(0);
    static BUTTON_CLEAR_CALLED: AtomicUsize = AtomicUsize::new(0);

    extern "efiapi" fn mock_button_check_returns_true(
        _this: *mut button_services::Protocol,
        state: *mut efi::Boolean,
    ) -> efi::Status {
        BUTTON_CHECK_CALLED.fetch_add(1, Ordering::SeqCst);
        // SAFETY: caller passes a valid &mut Boolean.
        unsafe { *state = efi::Boolean::TRUE };
        efi::Status::SUCCESS
    }

    extern "efiapi" fn mock_button_check_returns_false(
        _this: *mut button_services::Protocol,
        state: *mut efi::Boolean,
    ) -> efi::Status {
        // SAFETY: caller passes a valid &mut Boolean.
        unsafe { *state = efi::Boolean::FALSE };
        efi::Status::SUCCESS
    }

    extern "efiapi" fn mock_button_check_errors(
        _this: *mut button_services::Protocol,
        _state: *mut efi::Boolean,
    ) -> efi::Status {
        efi::Status::DEVICE_ERROR
    }

    extern "efiapi" fn mock_clear_state(_this: *mut button_services::Protocol) -> efi::Status {
        BUTTON_CLEAR_CALLED.fetch_add(1, Ordering::SeqCst);
        efi::Status::SUCCESS
    }

    fn make_button_protocol(
        check: button_services::ButtonCheckFn,
    ) -> button_services::Protocol {
        button_services::Protocol {
            pre_boot_volume_up_check: check,
            pre_boot_volume_down_check: check,
            pre_boot_clear_state: mock_clear_state,
            _reserved: ptr::null_mut(),
        }
    }

    #[test]
    fn test_detect_sre_hotkey_inner_pressed() {
        BUTTON_CHECK_CALLED.store(0, Ordering::SeqCst);
        BUTTON_CLEAR_CALLED.store(0, Ordering::SeqCst);

        let mut protocol = make_button_protocol(mock_button_check_returns_true);
        // SAFETY: protocol is alive on the test stack.
        let pressed = unsafe { detect_sre_hotkey_via_protocol(&mut protocol) };
        assert!(pressed, "TRUE state must surface as pressed");
        assert_eq!(BUTTON_CHECK_CALLED.load(Ordering::SeqCst), 1);
        assert_eq!(BUTTON_CLEAR_CALLED.load(Ordering::SeqCst), 1, "clear must run even on success");
    }

    #[test]
    fn test_detect_sre_hotkey_inner_not_pressed() {
        let mut protocol = make_button_protocol(mock_button_check_returns_false);
        // SAFETY: protocol is alive on the test stack.
        assert!(!unsafe { detect_sre_hotkey_via_protocol(&mut protocol) });
    }

    #[test]
    fn test_detect_sre_hotkey_inner_check_errors_returns_false() {
        let mut protocol = make_button_protocol(mock_button_check_errors);
        // SAFETY: protocol is alive on the test stack.
        assert!(!unsafe { detect_sre_hotkey_via_protocol(&mut protocol) });
    }

    // === probe_max_transfer ===

    static IDENTIFY_CDW0: AtomicU32 = AtomicU32::new(0);
    static IDENTIFY_CDW10: AtomicU32 = AtomicU32::new(0);
    static MDTS_TO_REPORT: AtomicU8 = AtomicU8::new(0);

    extern "efiapi" fn mock_identify_with_mdts(
        _this: *mut nvme_pass_thru::Protocol,
        _nsid: u32,
        packet: *mut nvme_pass_thru::CommandPacket,
        _event: *mut c_void,
    ) -> efi::Status {
        // SAFETY: caller (probe_max_transfer) builds a valid packet.
        unsafe {
            let pkt = &*packet;
            let cmd = &*pkt.nvme_cmd;
            IDENTIFY_CDW0.store(cmd.cdw0, Ordering::SeqCst);
            IDENTIFY_CDW10.store(cmd.cdw10, Ordering::SeqCst);
            // Write MDTS into the response buffer at offset 77.
            let buf = pkt.transfer_buffer as *mut u8;
            *buf.add(nvme_pass_thru::ID_CTRL_OFFSET_MDTS) = MDTS_TO_REPORT.load(Ordering::SeqCst);
        }
        efi::Status::SUCCESS
    }

    extern "efiapi" fn mock_identify_fails(
        _this: *mut nvme_pass_thru::Protocol,
        _nsid: u32,
        _packet: *mut nvme_pass_thru::CommandPacket,
        _event: *mut c_void,
    ) -> efi::Status {
        efi::Status::DEVICE_ERROR
    }

    fn make_nvme_protocol(pass_thru: nvme_pass_thru::PassThruFn) -> nvme_pass_thru::Protocol {
        nvme_pass_thru::Protocol {
            mode: ptr::null_mut(),
            pass_thru,
            get_next_namespace: ptr::null_mut(),
            build_device_path: ptr::null_mut(),
            get_namespace: ptr::null_mut(),
        }
    }

    #[test]
    fn test_probe_max_transfer_mdts_7_yields_512_kib() {
        MDTS_TO_REPORT.store(7, Ordering::SeqCst);
        let mut protocol = make_nvme_protocol(mock_identify_with_mdts);
        // SAFETY: protocol alive on stack.
        let chunk = probe_max_transfer(&mut protocol);
        // MDTS=7 -> 128 * 4096 = 512 KiB; equals READ_CHUNK_MAX, returned as-is.
        assert_eq!(chunk, 512 * 1024);
        // CDW0 opcode + CDW10 CNS=1 must match Identify Controller request.
        assert_eq!(IDENTIFY_CDW0.load(Ordering::SeqCst) & 0xFF, nvme_pass_thru::OPCODE_IDENTIFY as u32);
        assert_eq!(IDENTIFY_CDW10.load(Ordering::SeqCst), nvme_pass_thru::IDENTIFY_CNS_CONTROLLER);
    }

    #[test]
    fn test_probe_max_transfer_mdts_0_yields_chunk_max() {
        MDTS_TO_REPORT.store(0, Ordering::SeqCst);
        let mut protocol = make_nvme_protocol(mock_identify_with_mdts);
        // SAFETY: protocol alive on stack.
        let chunk = probe_max_transfer(&mut protocol);
        assert_eq!(chunk, READ_CHUNK_MAX, "MDTS=0 (no limit) clamps to self-imposed ceiling");
    }

    #[test]
    fn test_probe_max_transfer_mdts_low_clamps_to_min() {
        // MDTS=3 -> 8 * 4096 = 32 KiB, below READ_CHUNK_MIN.
        MDTS_TO_REPORT.store(3, Ordering::SeqCst);
        let mut protocol = make_nvme_protocol(mock_identify_with_mdts);
        // SAFETY: protocol alive on stack.
        let chunk = probe_max_transfer(&mut protocol);
        assert_eq!(chunk, READ_CHUNK_MIN, "below floor clamps to READ_CHUNK_MIN");
    }

    #[test]
    fn test_probe_max_transfer_identify_failure_uses_default() {
        let mut protocol = make_nvme_protocol(mock_identify_fails);
        // SAFETY: protocol alive on stack.
        let chunk = probe_max_transfer(&mut protocol);
        assert_eq!(chunk, READ_CHUNK_DEFAULT);
    }

    // === read_bp_via_log_page ===

    static GET_LOG_PAGE_LPOL: AtomicU64 = AtomicU64::new(0);
    static GET_LOG_PAGE_CDW10: AtomicU32 = AtomicU32::new(0);
    static GET_LOG_PAGE_LEN: AtomicUsize = AtomicUsize::new(0);

    extern "efiapi" fn mock_get_log_page_capture(
        _this: *mut nvme_pass_thru::Protocol,
        _nsid: u32,
        packet: *mut nvme_pass_thru::CommandPacket,
        _event: *mut c_void,
    ) -> efi::Status {
        // SAFETY: caller (read_bp_via_log_page) builds a valid packet.
        unsafe {
            let pkt = &*packet;
            let cmd = &*pkt.nvme_cmd;
            let lpol = (cmd.cdw12 as u64) | ((cmd.cdw13 as u64) << 32);
            GET_LOG_PAGE_LPOL.store(lpol, Ordering::SeqCst);
            GET_LOG_PAGE_CDW10.store(cmd.cdw10, Ordering::SeqCst);
            GET_LOG_PAGE_LEN.store(pkt.transfer_length as usize, Ordering::SeqCst);
            // Fill the destination with a sentinel pattern.
            core::ptr::write_bytes(pkt.transfer_buffer as *mut u8, 0xA5, pkt.transfer_length as usize);
        }
        efi::Status::SUCCESS
    }

    extern "efiapi" fn mock_get_log_page_fails(
        _this: *mut nvme_pass_thru::Protocol,
        _nsid: u32,
        _packet: *mut nvme_pass_thru::CommandPacket,
        _event: *mut c_void,
    ) -> efi::Status {
        efi::Status::DEVICE_ERROR
    }

    #[test]
    fn test_read_bp_via_log_page_single_chunk_skips_lid_header() {
        let mut protocol = make_nvme_protocol(mock_get_log_page_capture);
        let mut dest = [0u8; 64 * 1024];
        // SAFETY: protocol alive on stack.
        let result = read_bp_via_log_page(&mut protocol, 1, 64 * 1024, &mut dest);
        assert!(result.is_ok());
        assert_eq!(GET_LOG_PAGE_LPOL.load(Ordering::SeqCst), LID_BP_HEADER_BYTES);
        let cdw10 = GET_LOG_PAGE_CDW10.load(Ordering::SeqCst);
        assert_eq!(cdw10 & 0xFF, nvme_pass_thru::LID_BOOT_PARTITION, "LID byte");
        assert_eq!((cdw10 >> 8) & 0x7F, 1, "BPID in LSP");
        assert_eq!(dest[0], 0xA5, "destination buffer was populated");
    }

    #[test]
    fn test_read_bp_via_log_page_propagates_failure() {
        let mut protocol = make_nvme_protocol(mock_get_log_page_fails);
        let mut dest = [0u8; 64 * 1024];
        // SAFETY: protocol alive on stack.
        let result = read_bp_via_log_page(&mut protocol, 1, 64 * 1024, &mut dest);
        assert!(result.is_err());
    }

    // === register_ram_disk ===

    static RAM_DISK_BASE: AtomicU64 = AtomicU64::new(0);
    static RAM_DISK_SIZE: AtomicU64 = AtomicU64::new(0);
    static RAM_DISK_OUT_DP: AtomicUsize = AtomicUsize::new(0);

    extern "efiapi" fn mock_ram_disk_register(
        ram_disk_base: u64,
        ram_disk_size: u64,
        _ram_disk_type: *const efi::Guid,
        _parent_device_path: *const c_void,
        device_path: *mut *const c_void,
    ) -> efi::Status {
        RAM_DISK_BASE.store(ram_disk_base, Ordering::SeqCst);
        RAM_DISK_SIZE.store(ram_disk_size, Ordering::SeqCst);
        // Return a sentinel non-null device path pointer.
        let sentinel: usize = 0xDEAD_BEEF;
        // SAFETY: caller passes a valid &mut *const c_void.
        unsafe { *device_path = sentinel as *const c_void };
        RAM_DISK_OUT_DP.store(sentinel, Ordering::SeqCst);
        efi::Status::SUCCESS
    }

    extern "efiapi" fn mock_ram_disk_register_fails(
        _ram_disk_base: u64,
        _ram_disk_size: u64,
        _ram_disk_type: *const efi::Guid,
        _parent_device_path: *const c_void,
        _device_path: *mut *const c_void,
    ) -> efi::Status {
        efi::Status::OUT_OF_RESOURCES
    }

    extern "efiapi" fn mock_ram_disk_unregister(_dp: *const c_void) -> efi::Status {
        efi::Status::SUCCESS
    }

    fn make_ram_disk_protocol(register: ram_disk::RegisterFn) -> ram_disk::Protocol {
        ram_disk::Protocol { register, unregister: mock_ram_disk_unregister }
    }

    #[test]
    fn test_register_ram_disk_inner_success_returns_dp() {
        let mut protocol = make_ram_disk_protocol(mock_ram_disk_register);
        let dram = [0xCDu8; 4096];
        // SAFETY: protocol alive on stack; dram alive for the call.
        let dp = unsafe { register_ram_disk_via_protocol(&mut protocol, &dram) }
            .expect("register should succeed");
        assert_eq!(dp as usize, 0xDEAD_BEEF);
        assert_eq!(RAM_DISK_BASE.load(Ordering::SeqCst), dram.as_ptr() as u64);
        assert_eq!(RAM_DISK_SIZE.load(Ordering::SeqCst), 4096);
    }

    #[test]
    fn test_register_ram_disk_inner_propagates_failure() {
        let mut protocol = make_ram_disk_protocol(mock_ram_disk_register_fails);
        let dram = [0u8; 4096];
        // SAFETY: protocol alive on stack; dram alive for the call.
        let result = unsafe { register_ram_disk_via_protocol(&mut protocol, &dram) };
        assert!(result.is_err());
    }

    #[test]
    fn test_register_ram_disk_outer_locate_failure() {
        let mut mock = MockBootServices::new();
        mock.expect_locate_protocol_unchecked().returning(|_, _| Err(efi::Status::NOT_FOUND));

        let dram = [0u8; 4096];
        let result = register_ram_disk(&mock, &dram);
        assert!(result.is_err());
    }
}

