//! SRE recovery boot path: read the recovery payload from NVMe Boot
//! Partition 1 via Get Log Page LID=0x15, install a synthesized
//! EFI_BLOCK_IO_PROTOCOL handle backed by the in-memory buffer, and
//! chainload the architecture's default bootloader
//! ([`DEFAULT_BOOT_FILE_PATH`]) from the resulting FAT volume.
//!
//! This module inlines FFI for EFI_NVM_EXPRESS_PASS_THRU_PROTOCOL (for
//! Identify + Get Log Page) and EFI_BLOCK_IO_PROTOCOL (the RAM disk
//! interface synthesized in-Rust) rather than depending on helpers that
//! may not be published yet.
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
use core::{ffi::c_void, ptr};

use patina::{
    boot_services::BootServices,
    device_path::{
        parse_node::Header as DevicePathNodeHdr,
        paths::{DevicePath, DevicePathBuf},
    },
    error::EfiError,
};
use patina_boot::helpers;
use r_efi::{efi, protocols::device_path};
use scroll::{
    Endian,
    ctx::{TryFromCtx, TryIntoCtx},
};

/// Target boot partition for the SRE WIM payload.
const TARGET_BPID: u8 = 1;

/// Byte size of a device-path node header.
const NODE_HDR_BYTES: usize = DevicePathNodeHdr::size_of_header();

/// Sanity cap for device-path walks; real device paths are well under this.
const DEVICE_PATH_MAX_BYTES: usize = 64 * 1024;

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

/// Removable-media default bootloader path for the target architecture
/// (UEFI spec §3.5.1.1). Chainloaded from the registered RAM disk's FAT
/// volume, and appended to USB SimpleFileSystem paths by the boot manager.
#[cfg(target_arch = "aarch64")]
pub(crate) const DEFAULT_BOOT_FILE_PATH: &str = "\\EFI\\Boot\\bootaa64.efi";
#[cfg(not(target_arch = "aarch64"))]
pub(crate) const DEFAULT_BOOT_FILE_PATH: &str = "\\EFI\\Boot\\bootx64.efi";

/// EFI_NVM_EXPRESS_PASS_THRU_PROTOCOL FFI.
mod nvme_pass_thru {
    use core::ffi::c_void;
    use r_efi::efi;

    pub const PROTOCOL_GUID: efi::Guid = efi::Guid::from_fields(
        0x52c78312,
        0x8edc,
        0x4233,
        0x98,
        0xf2,
        &[0x1a, 0x1a, 0xa5, 0xe3, 0x88, 0xa5],
    );

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

/// Probe controller MDTS via Identify Controller (CNS=0x01); derive the
/// largest LID 0x15 chunk size we'll use. Falls back to `READ_CHUNK_DEFAULT`
/// on probe failure. Clamps to `[READ_CHUNK_MIN, READ_CHUNK_MAX]`.
fn probe_max_transfer(passthru: *mut nvme_pass_thru::Protocol) -> usize {
    let mut buf = vec![0u8; nvme_pass_thru::IDENTIFY_BUFFER_BYTES];
    let mut cmd = nvme_pass_thru::Command {
        cdw0: nvme_pass_thru::OPCODE_IDENTIFY as u32,
        cdw10: nvme_pass_thru::IDENTIFY_CNS_CONTROLLER,
        flags: nvme_pass_thru::CMD_FLAG_CDW10_VALID,
        ..Default::default()
    };

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
        log::warn!(
            "Identify Controller failed: {:?}; falling back to {} KiB chunk",
            status,
            READ_CHUNK_DEFAULT / 1024
        );
        return READ_CHUNK_DEFAULT;
    }

    let mdts = buf[nvme_pass_thru::ID_CTRL_OFFSET_MDTS];
    let chunk = if mdts == 0 {
        READ_CHUNK_MAX
    } else {
        // MDTS is a controller-reported value; guard the shift/multiply so a
        // malformed Identify (mdts >= usize bit-width, or an overflowing
        // product) cannot panic in firmware. Fall back to the max chunk.
        let max_transfer = 1usize
            .checked_shl(mdts as u32)
            .and_then(|pages| pages.checked_mul(4096))
            .unwrap_or(READ_CHUNK_MAX);
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
    // Reads are issued in whole dwords (NUMD is a 0-based dword count); a
    // zero or unaligned size would underflow `numd` or stall the loop.
    if chunk_bytes < 4 || !chunk_bytes.is_multiple_of(4) || !dest.len().is_multiple_of(4) {
        log::error!(
            "read_bp_via_log_page: sizes must be dword-aligned and non-zero (chunk={}, dest={})",
            chunk_bytes,
            dest.len()
        );
        return Err(EfiError::from(efi::Status::INVALID_PARAMETER));
    }
    let mut bp_off: u64 = 0;
    while (bp_off as usize) < dest.len() {
        let remaining = dest.len() - bp_off as usize;
        let read_bytes = chunk_bytes.min(remaining);
        let lpol = LID_BP_HEADER_BYTES + bp_off;

        let numd = ((read_bytes / 4) - 1) as u32;
        let mut cmd = nvme_pass_thru::Command {
            cdw0: nvme_pass_thru::OPCODE_GET_LOG_PAGE as u32,
            cdw10: ((numd & 0xFFFF) << 16) | (((bp_id as u32) & 0x7F) << 8) | nvme_pass_thru::LID_BOOT_PARTITION,
            cdw11: (numd >> 16) & 0xFFFF,
            cdw12: (lpol & 0xFFFFFFFF) as u32,
            cdw13: ((lpol >> 32) & 0xFFFFFFFF) as u32,
            flags: nvme_pass_thru::CMD_FLAG_CDW10_VALID
                | nvme_pass_thru::CMD_FLAG_CDW11_VALID
                | nvme_pass_thru::CMD_FLAG_CDW12_VALID
                | nvme_pass_thru::CMD_FLAG_CDW13_VALID,
            ..Default::default()
        };

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
                lpol,
                read_bytes,
                status
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

/// EFI_BLOCK_IO_PROTOCOL FFI — we install this directly on a synthesized
/// handle (no `RamDiskDxe` dependency) so `FatDxe` can bind to the FAT
/// volume in BP1 without depending on the EDK2 `MdeModulePkg` RAM disk
/// producer (which has a `[Depex]` on HII Database/Config Routing that
/// alternate dispatchers like Patina don't reliably satisfy).
mod block_io {
    use core::ffi::c_void;
    use r_efi::efi;

    /// EFI_BLOCK_IO_PROTOCOL_GUID. `static` (not `const`) so that
    /// `install_protocol_interface_unchecked` can borrow a `&'static`.
    pub static PROTOCOL_GUID: efi::Guid = efi::Guid::from_fields(
        0x964e5b21,
        0x6459,
        0x11d2,
        0x8e,
        0x39,
        &[0x00, 0xa0, 0xc9, 0x69, 0x72, 0x3b],
    );

    pub const REVISION: u64 = 0x00010000;

    #[repr(C)]
    pub struct Media {
        pub media_id: u32,
        pub removable_media: efi::Boolean,
        pub media_present: efi::Boolean,
        pub logical_partition: efi::Boolean,
        pub read_only: efi::Boolean,
        pub write_caching: efi::Boolean,
        pub block_size: u32,
        pub io_align: u32,
        pub last_block: u64,
    }

    pub type ResetFn = extern "efiapi" fn(this: *mut Protocol, extended_verification: efi::Boolean) -> efi::Status;
    pub type ReadFn = extern "efiapi" fn(
        this: *mut Protocol,
        media_id: u32,
        lba: u64,
        buffer_size: usize,
        buffer: *mut c_void,
    ) -> efi::Status;
    pub type WriteFn = extern "efiapi" fn(
        this: *mut Protocol,
        media_id: u32,
        lba: u64,
        buffer_size: usize,
        buffer: *const c_void,
    ) -> efi::Status;
    pub type FlushFn = extern "efiapi" fn(this: *mut Protocol) -> efi::Status;

    #[repr(C)]
    pub struct Protocol {
        pub revision: u64,
        pub media: *mut Media,
        pub reset: ResetFn,
        pub read_blocks: ReadFn,
        pub write_blocks: WriteFn,
        pub flush_blocks: FlushFn,
    }
}

/// Block-size used when exposing the BP1 buffer as block media. 512 is the
/// universal lowest common denominator and what `BuildBpFatImage.ps1`
/// formats against.
const RAM_DISK_BLOCK_SIZE: u32 = 512;

/// Synthetic backing struct for our `EFI_BLOCK_IO_PROTOCOL` install. The
/// `protocol` field must come first and have identical layout to
/// `block_io::Protocol` so a `*mut block_io::Protocol` callback receiver
/// can be cast back to `*mut RamDiskBlockIo` to reach the extension
/// fields (buffer pointer/size, owned `Media`).
#[repr(C)]
struct RamDiskBlockIo {
    protocol: block_io::Protocol,
    media: block_io::Media,
    buffer_ptr: *const u8,
    buffer_size: u64,
}

extern "efiapi" fn ram_disk_reset(_this: *mut block_io::Protocol, _ext: efi::Boolean) -> efi::Status {
    efi::Status::SUCCESS
}

extern "efiapi" fn ram_disk_read_blocks(
    this: *mut block_io::Protocol,
    media_id: u32,
    lba: u64,
    buffer_size: usize,
    buffer: *mut core::ffi::c_void,
) -> efi::Status {
    if this.is_null() || buffer.is_null() {
        return efi::Status::INVALID_PARAMETER;
    }
    // SAFETY: `this` was installed by `register_ram_disk` as a pointer to a
    // leaked `RamDiskBlockIo` whose first field is `protocol`. We cast back
    // to access the extension fields.
    let state = unsafe { &*(this as *const RamDiskBlockIo) };
    if media_id != state.media.media_id {
        return efi::Status::MEDIA_CHANGED;
    }
    let offset = lba.saturating_mul(state.media.block_size as u64);
    let end = offset.saturating_add(buffer_size as u64);
    if end > state.buffer_size {
        return efi::Status::INVALID_PARAMETER;
    }
    if !buffer_size.is_multiple_of(state.media.block_size as usize) {
        return efi::Status::BAD_BUFFER_SIZE;
    }
    // SAFETY: bounds checked above; src is the backing buffer (live for the
    // boot), dst is caller-supplied and non-null.
    unsafe {
        core::ptr::copy_nonoverlapping(state.buffer_ptr.add(offset as usize), buffer as *mut u8, buffer_size);
    }
    efi::Status::SUCCESS
}

extern "efiapi" fn ram_disk_write_blocks(
    _this: *mut block_io::Protocol,
    _media_id: u32,
    _lba: u64,
    _buffer_size: usize,
    _buffer: *const core::ffi::c_void,
) -> efi::Status {
    efi::Status::WRITE_PROTECTED
}

extern "efiapi" fn ram_disk_flush_blocks(_this: *mut block_io::Protocol) -> efi::Status {
    efi::Status::SUCCESS
}

/// A registered RAM disk install: the synthesized handle, the device path
/// installed on it, and the leaked protocol state. Returned by
/// [`register_ram_disk`] so the caller can `locate_device_path` the RAM disk
/// for chainloading and tear the install down if control returns.
struct RamDiskInstall {
    handle: efi::Handle,
    device_path: *const c_void,
    state: *mut RamDiskBlockIo,
}

impl RamDiskInstall {
    /// Disconnect bound drivers and uninstall both protocols, then free the
    /// protocol state. Returns `Err` without freeing the state if an
    /// uninstall is refused (e.g. a driver still holds the interface open);
    /// in that case the backing buffer must also stay live.
    ///
    /// The 24-byte leaked device-path allocation is not reclaimed: consumers
    /// may retain the pointer, and the size is negligible.
    fn teardown<B: BootServices>(self, boot_services: &B) -> Result<(), EfiError> {
        // Best-effort: close driver opens (FatDxe/PartitionDxe) so the
        // uninstalls below are not refused with ACCESS_DENIED.
        // SAFETY: handle is the RAM disk handle created by register_ram_disk;
        // its driver bindings are live for the duration of the call.
        let _ = unsafe { boot_services.disconnect_controller(self.handle, None, None) };
        // SAFETY: both interfaces were installed on `handle` with exactly
        // these pointers by register_ram_disk.
        unsafe {
            boot_services.uninstall_protocol_interface_unchecked(
                self.handle,
                &efi::protocols::device_path::PROTOCOL_GUID,
                self.device_path as *mut c_void,
            )
        }
        .map_err(EfiError::from)?;
        // SAFETY: as above.
        unsafe {
            boot_services.uninstall_protocol_interface_unchecked(
                self.handle,
                &block_io::PROTOCOL_GUID,
                self.state as *mut c_void,
            )
        }
        .map_err(EfiError::from)?;
        // SAFETY: no protocol references the state after the uninstalls.
        drop(unsafe { alloc::boxed::Box::from_raw(self.state) });
        Ok(())
    }
}

/// Expose `dram` as a Block IO device on a synthesized handle so the
/// rest of UEFI (PartitionDxe / FatDxe) can bind to it without needing
/// `MdeModulePkg`'s `RamDiskDxe` (which depexes on HII and doesn't
/// reliably install under Patina-style dispatchers). Returns a
/// [`RamDiskInstall`] whose device path the chainload step can
/// `locate_device_path` and `LoadImage` from the FAT volume that
/// `FatDxe` will bind on top.
fn register_ram_disk<B: BootServices>(boot_services: &B, dram: &[u8]) -> Result<RamDiskInstall, EfiError> {
    // Allocate state on the heap and leak it so the install survives this
    // function return. The buffer + state must outlive the firmware's use of
    // the protocol; they stay live until teardown or ExitBootServices.
    let mut state = alloc::boxed::Box::new(RamDiskBlockIo {
        protocol: block_io::Protocol {
            revision: block_io::REVISION,
            media: ptr::null_mut(),
            reset: ram_disk_reset,
            read_blocks: ram_disk_read_blocks,
            write_blocks: ram_disk_write_blocks,
            flush_blocks: ram_disk_flush_blocks,
        },
        media: block_io::Media {
            media_id: 0,
            removable_media: efi::Boolean::FALSE,
            media_present: efi::Boolean::TRUE,
            // LogicalPartition=TRUE so FatDxe binds directly to our BlockIo
            // (BP1 holds a raw FAT volume, not a partition table).
            logical_partition: efi::Boolean::TRUE,
            read_only: efi::Boolean::TRUE,
            write_caching: efi::Boolean::FALSE,
            block_size: RAM_DISK_BLOCK_SIZE,
            io_align: 0,
            last_block: (dram.len() as u64 / RAM_DISK_BLOCK_SIZE as u64).saturating_sub(1),
        },
        buffer_ptr: dram.as_ptr(),
        buffer_size: dram.len() as u64,
    });
    // Wire the media pointer to the owned Media field after the Box exists
    // (we need the final heap address).
    state.protocol.media = &mut state.media as *mut block_io::Media;
    let state_ptr: *mut RamDiskBlockIo = alloc::boxed::Box::into_raw(state);

    // Build a synthetic device path = Media/Vendor node + EndEntire.
    // Vendor GUID is arbitrary but unique to this RAM disk install so
    // callers can distinguish it from any other Block IO device.
    let dp_bytes: Vec<u8> = vec![
        // Media Vendor node: type=4, subtype=3, length=20 (4 header + 16 GUID)
        0x04, 0x03, 0x14, 0x00, // Vendor GUID 5d0b1c7e-8a2f-4e5c-9d3a-7b8e9f1c2d4a (byte-wise)
        0x7e, 0x1c, 0x0b, 0x5d, 0x2f, 0x8a, 0x5c, 0x4e, 0x9d, 0x3a, 0x7b, 0x8e, 0x9f, 0x1c, 0x2d, 0x4a,
        // EndEntire node
        0x7f, 0xff, 0x04, 0x00,
    ];
    let leaked_dp: &'static mut [u8] = Vec::leak(dp_bytes);
    let dp_ptr = leaked_dp.as_mut_ptr() as *mut c_void;

    // Install Block IO first (creates the handle), then add the device path
    // to the same handle.
    // SAFETY: state_ptr points at a leaked RamDiskBlockIo whose first field
    // is the standard Block IO protocol layout. The protocol struct lives
    // for the rest of this boot.
    let handle = unsafe {
        boot_services.install_protocol_interface_unchecked(None, &block_io::PROTOCOL_GUID, state_ptr as *mut c_void)
    }
    .map_err(|s| {
        log::error!("InstallProtocolInterface(BlockIo): {:?}", s);
        EfiError::from(s)
    })?;

    // SAFETY: handle returned by install above; device path bytes are leaked
    // and well-formed (Vendor node + End).
    if let Err(s) = unsafe {
        boot_services.install_protocol_interface_unchecked(
            Some(handle),
            &efi::protocols::device_path::PROTOCOL_GUID,
            dp_ptr,
        )
    } {
        log::error!("InstallProtocolInterface(DevicePath): {:?}", s);
        // Roll back the BlockIo install so no half-registered handle remains.
        // SAFETY: block_io was installed on `handle` with `state_ptr` above.
        let rollback = unsafe {
            boot_services.uninstall_protocol_interface_unchecked(
                handle,
                &block_io::PROTOCOL_GUID,
                state_ptr as *mut c_void,
            )
        };
        if rollback.is_ok() {
            // SAFETY: no protocol references the state after the uninstall.
            drop(unsafe { alloc::boxed::Box::from_raw(state_ptr) });
        }
        return Err(EfiError::from(s));
    }

    log::info!(
        "RAM disk Block IO installed on synthesized handle ({} MiB)",
        dram.len() / (1024 * 1024)
    );

    // Recursive connect so PartitionDxe + FatDxe (or just FatDxe directly
    // since LogicalPartition=TRUE) bind. After this, SFS should be on the
    // same handle so the existing chainload_from_ramdisk path can find it
    // via locate_device_path.
    // SAFETY: handle is valid; empty driver list + null remaining_dp +
    // recursive=true is the standard "let the dispatcher figure it out".
    if let Err(s) = unsafe { boot_services.connect_controller(handle, Vec::new(), None, true) } {
        log::warn!(
            "ConnectController(RamDiskHandle): {:?} (continuing; SFS may still be bound)",
            s
        );
    }

    Ok(RamDiskInstall {
        handle,
        device_path: dp_ptr as *const c_void,
        state: state_ptr,
    })
}

/// Compute length of a device path in bytes (walks until END_ENTIRE).
///
/// Returns 0 on malformed input — a node length below the 4-byte header
/// minimum, or a total size above the sanity cap — so callers can fail
/// cleanly instead of hanging or slicing out of bounds.
pub(crate) unsafe fn device_path_size(dp: *const u8) -> usize {
    let mut consumed: usize = 0;
    loop {
        // Do not read another header once fewer than its bytes remain within
        // the cap. This bounds the walk even if the previous node ended
        // exactly at the cap without an END_ENTIRE node.
        if consumed > DEVICE_PATH_MAX_BYTES - NODE_HDR_BYTES {
            return 0;
        }
        // SAFETY: caller guarantees `dp` points at a device path; the header
        // is copied out before parsing, and the length and cap checks below
        // stop the walk on malformed input instead of reading arbitrary
        // memory. `consumed` is bounded by the cap, so the read stays within
        // the walked window.
        let hdr_bytes = unsafe { ptr::read_unaligned(dp.wrapping_add(consumed) as *const [u8; NODE_HDR_BYTES]) };
        let Ok((hdr, _)) = DevicePathNodeHdr::try_from_ctx(&hdr_bytes, Endian::Little) else {
            return 0;
        };
        let length = hdr.length;
        // A node length below the header minimum can't be walked (length 0
        // would spin forever). Return 0 so callers fail cleanly.
        if length < NODE_HDR_BYTES {
            return 0;
        }
        let is_end_entire = hdr.r#type == device_path::TYPE_END && hdr.sub_type == device_path::End::SUBTYPE_ENTIRE;
        if is_end_entire && length != NODE_HDR_BYTES {
            return 0;
        }
        // Accumulate the walked size in an independent counter, not a pointer
        // subtraction, so a malformed length cannot wrap the total back under
        // the cap. The cap bounds how far a malformed path carries the walk.
        let total = match consumed.checked_add(length) {
            Some(t) if t <= DEVICE_PATH_MAX_BYTES => t,
            _ => return 0,
        };
        consumed = total;
        if is_end_entire {
            return total;
        }
    }
}

/// Build a MEDIA_DEVICE_PATH/MEDIA_FILEPATH_DP node followed by END_ENTIRE.
/// Returns the raw bytes; caller is responsible for prepending the parent
/// device-path bytes when constructing a full path.
pub(crate) fn build_file_path_node(path: &str) -> Vec<u8> {
    // UTF-16 + null terminator.
    let mut utf16: Vec<u16> = path.encode_utf16().collect();
    utf16.push(0);
    let path_bytes = utf16.len() * 2;
    let node_len = NODE_HDR_BYTES + path_bytes;

    // END_ENTIRE: length=4.
    let end_hdr = DevicePathNodeHdr::new(device_path::TYPE_END, device_path::End::SUBTYPE_ENTIRE, NODE_HDR_BYTES);

    // The node length field is 16-bit; a path long enough to overflow it
    // would produce a header that misdescribes the appended bytes. Fail
    // safely with a bare END_ENTIRE node, which parses cleanly and makes
    // the subsequent LoadImage return NotFound.
    if node_len > u16::MAX as usize {
        log::error!("build_file_path_node: path too long for a FilePath node ({node_len} bytes)");
        return encode_node_header(end_hdr).to_vec();
    }

    // MEDIA_FILEPATH_DP node.
    let file_hdr = DevicePathNodeHdr::new(device_path::TYPE_MEDIA, device_path::Media::SUBTYPE_FILE_PATH, node_len);

    let mut out = Vec::with_capacity(node_len + NODE_HDR_BYTES);
    out.extend_from_slice(&encode_node_header(file_hdr));
    for w in &utf16 {
        out.extend_from_slice(&w.to_le_bytes());
    }
    out.extend_from_slice(&encode_node_header(end_hdr));
    out
}

fn encode_node_header(header: DevicePathNodeHdr) -> [u8; NODE_HDR_BYTES] {
    let mut bytes = [0; NODE_HDR_BYTES];
    let written = header
        .try_into_ctx(&mut bytes, Endian::Little)
        .expect("fixed-size buffer must encode a device-path header");
    debug_assert_eq!(written, NODE_HDR_BYTES);
    bytes
}

/// Find the FAT SimpleFileSystem handle rooted at `parent_dp`, then chainload
/// `\EFI\Boot\bootx64.efi` from it via `helpers::boot_from_device_path`.
fn chainload_from_ramdisk<B: BootServices>(
    locked_boot: &helpers::LockedBoot<'_, B>,
    image_handle: efi::Handle,
    parent_dp: *const c_void,
) -> Result<(), EfiError> {
    use patina::boot_services::protocol_handler::HandleSearchType;
    use r_efi::protocols::simple_file_system;

    let boot_services = locked_boot.boot_services();

    // Connect the RAM disk handle so PartitionDxe + FAT bind.
    let mut remaining_dp = parent_dp as *mut efi::protocols::device_path::Protocol;
    let ram_handle =
        unsafe { boot_services.locate_device_path(&efi::protocols::device_path::PROTOCOL_GUID, &mut remaining_dp) }
            .map_err(EfiError::from)?;
    // SAFETY: ram_handle was just returned by locate_device_path; an empty
    // driver_image_handles list is permitted per the trait contract.
    let _ = unsafe { boot_services.connect_controller(ram_handle, Vec::new(), None, true) };

    // Patina's LoadImage doesn't walk a parent-path + FilePath through the
    // dynamically-bound filesystem stack the way EDK2 DxeCore does. Find
    // the SFS handle that descended from our parent_dp explicitly, then
    // build the LoadImage path off THAT handle's device path. Same
    // workaround pattern used for USB SFS dispatch.
    let parent_size = unsafe { device_path_size(parent_dp as *const u8) };
    // Strip END_ENTIRE; checked_sub guards against a malformed path
    // (device_path_size returns 0) underflowing into a huge slice.
    let Some(parent_payload_size) = parent_size.checked_sub(4) else {
        log::error!("RAM disk device path malformed (size={})", parent_size);
        return Err(EfiError::from(efi::Status::INVALID_PARAMETER));
    };
    let parent_prefix = unsafe { core::slice::from_raw_parts(parent_dp as *const u8, parent_payload_size) };

    let sfs_handles = boot_services
        .locate_handle_buffer(HandleSearchType::ByProtocol(&simple_file_system::PROTOCOL_GUID))
        .map_err(EfiError::from)?;

    let mut chosen_sfs_dp: Option<*const u8> = None;
    for &h in sfs_handles.iter() {
        // SAFETY: handle returned by locate_handle_buffer for the SFS GUID.
        let dp_ptr =
            match unsafe { boot_services.handle_protocol_unchecked(h, &efi::protocols::device_path::PROTOCOL_GUID) } {
                Ok(p) if !p.is_null() => p as *const u8,
                _ => continue,
            };
        let dp_total = unsafe { device_path_size(dp_ptr) };
        if dp_total < parent_payload_size {
            continue;
        }
        // SAFETY: dp_ptr valid for dp_total bytes.
        let candidate_prefix = unsafe { core::slice::from_raw_parts(dp_ptr, parent_payload_size) };
        if candidate_prefix == parent_prefix {
            log::info!("Found descendant SFS handle for RAM disk (path prefix matches)");
            chosen_sfs_dp = Some(dp_ptr);
            break;
        }
    }

    let sfs_dp = chosen_sfs_dp.ok_or_else(|| {
        log::error!(
            "No SimpleFileSystem handle found whose device path descends from the RAM disk (FatDxe may not have bound)"
        );
        EfiError::NotFound
    })?;

    // Build chainload path: <sfs_handle_dp> (less END) + FilePath + END.
    let sfs_total = unsafe { device_path_size(sfs_dp) };
    // Same malformed-path guard as the parent-path calculation above.
    let Some(sfs_payload_size) = sfs_total.checked_sub(4) else {
        log::error!("SFS device path malformed (size={})", sfs_total);
        return Err(EfiError::from(efi::Status::INVALID_PARAMETER));
    };
    let file_node = build_file_path_node(DEFAULT_BOOT_FILE_PATH);

    let mut full_path = Vec::with_capacity(sfs_payload_size + file_node.len());
    // SAFETY: sfs_dp + sfs_payload_size in-bounds per device_path_size.
    let sfs_payload = unsafe { core::slice::from_raw_parts(sfs_dp, sfs_payload_size) };
    full_path.extend_from_slice(sfs_payload);
    full_path.extend_from_slice(&file_node);

    // SAFETY: full_path is assembled from a validated device path prefix and
    // a well-formed FilePath + EndEntire suffix.
    let device_path = unsafe { DevicePath::try_from_ptr(full_path.as_ptr()) }
        .map(DevicePathBuf::from)
        .map_err(|_| EfiError::InvalidParameter)?;

    locked_boot.boot_from_device_path(image_handle, &device_path)
}

/// Run the SRE recovery flow end-to-end on Vol-Up hotkey:
///
///   1. ConnectAll (priority-boot dispatch runs before BdsConnectAll)
///   2. Locate NvmExpressPassThru
///   3. Probe MDTS, pick chunk size
///   4. Read BP1 via chunked LID 0x15 into a fresh DRAM allocation
///   5. Register the allocation as a RAM disk
///   6. Chainload [`DEFAULT_BOOT_FILE_PATH`] from the FAT volume on the RAM disk
///
/// BP write protection (lock/unlock) is owned by the FMP capsule flow;
/// SRE boot is read-only against BPWPS.
pub fn run_sre_flow<B: BootServices>(
    locked_boot: &helpers::LockedBoot<'_, B>,
    image_handle: efi::Handle,
) -> Result<(), EfiError> {
    let boot_services = locked_boot.boot_services();
    helpers::connect_all(boot_services).ok();

    // SAFETY: dereferencing the returned interface only via raw pointer calls below.
    let passthru = unsafe { boot_services.locate_protocol_unchecked(&nvme_pass_thru::PROTOCOL_GUID, ptr::null_mut()) }
        .map_err(|e| {
        log::error!("LocateProtocol(NvmExpressPassThru): {:?}", e);
        EfiError::from(e)
    })? as *mut nvme_pass_thru::Protocol;

    let chunk = probe_max_transfer(passthru);

    // 1 GiB DRAM allocation for BP image. The RAM-disk protocol borrows the
    // pointer while registered; this frame retains ownership and frees the
    // buffer after a successful teardown when the chainload returns control.
    // On a successful boot the chainloaded OS takes over memory ownership at
    // ExitBootServices and this frame never resumes.
    //
    // Fallible alloc so we can surface OUT_OF_RESOURCES rather than panic.
    let mut buf: Vec<u8> = Vec::new();
    buf.try_reserve_exact(BPSIZE_BYTES).map_err(|_| {
        log::error!("BP buffer allocation failed ({} MiB)", BPSIZE_BYTES / (1024 * 1024));
        EfiError::from(efi::Status::OUT_OF_RESOURCES)
    })?;
    buf.resize(BPSIZE_BYTES, 0);
    log::info!(
        "BP read: {} MiB via LID 0x15, chunk={} KiB",
        BPSIZE_BYTES / (1024 * 1024),
        chunk / 1024
    );
    read_bp_via_log_page(passthru, TARGET_BPID, chunk, &mut buf)?;
    log::info!("BP read complete ({} bytes)", buf.len());

    let install = register_ram_disk(boot_services, &buf)?;

    let result = chainload_from_ramdisk(locked_boot, image_handle, install.device_path);

    // Control only reaches here if the load failed or the chainloaded image
    // exited; a successful boot calls ExitBootServices and never returns.
    // Tear the RAM disk down so later boot attempts in this same boot don't
    // dispatch against a BlockIo backed by a freed buffer, and reclaim the
    // BP allocation.
    match install.teardown(boot_services) {
        Ok(()) => drop(buf),
        Err(e) => {
            // An interface is still open somewhere; keep the buffer live for
            // the rest of this boot rather than free memory a driver may
            // still read through the protocol.
            log::warn!("RAM disk teardown failed ({:?}); BP buffer stays allocated", e);
            core::mem::forget(buf);
        }
    }

    result
}

/// True if BP1 contains a FAT volume at offset 0 — the production-correct
/// SRE payload layout produced by `Stage-SreflashUsb.ps1 -WrapWim` and the
/// only layout `bp_recovery::run_sre_flow` can chainload from.
///
/// Check criterion:
///   - bytes 510-511 are the FAT/MBR boot signature `0x55 0xAA`
///
/// The BPB jump opcode at byte 0 is NOT checked. Standard FAT images
/// use `0xEB`/`0xE9`/`0x90` there, but the FAT image produced by
/// `BuildBpFatImage.ps1` (called from `-WrapWim`) writes custom boot
/// code that starts with `0x33` (`XOR EAX, EAX`). Trusting just the
/// 0x55AA signature avoids false negatives while keeping the
/// false-positive probability vanishingly small (1 in 65 536 for a
/// random sector).
///
/// Used by the SreBootManager fallback path to decide between "boot SRE
/// from BP1" and "give up" when normal boot has exhausted all options
/// (or when a USB Boot#### entry would just re-run the flashing tool that
/// put the payload there).
///
/// Returns `false` on any error (no NvmExpressPassThru protocol, BP1
/// inaccessible, signature mismatch). Caller should treat as "no SRE
/// payload present, fall through to default failure handling".
///
/// Raw-WIM payloads (no `-WrapWim`) are intentionally NOT recognized
/// here: `run_sre_flow` cannot chainload from a raw-WIM BP1 (no FAT
/// volume → no `\EFI\Boot\bootx64.efi` to load), so accepting them would
/// trigger a fallback that immediately fails. The staging flow must
/// always use `-WrapWim`.
///
/// Cost: one 512-byte LID 0x15 head read of BP1.
pub fn bp_has_sre_payload<B: BootServices>(boot_services: &B) -> bool {
    // The NVMe pass-thru protocol is produced by connecting the NVMe
    // controller; the caller connects the boot device path before calling
    // this, so no whole-tree connect is needed here. If NVMe is not connected,
    // the locate below fails cleanly and this returns false.

    // SAFETY: dereferencing the returned interface only via raw pointer calls below.
    let passthru =
        match unsafe { boot_services.locate_protocol_unchecked(&nvme_pass_thru::PROTOCOL_GUID, ptr::null_mut()) } {
            Ok(p) => p as *mut nvme_pass_thru::Protocol,
            Err(e) => {
                log::warn!("bp_has_sre_payload: NvmExpressPassThru not available: {:?}", e);
                return false;
            }
        };

    let mut head = [0u8; 512];
    if let Err(e) = read_bp_via_log_page(passthru, TARGET_BPID, head.len(), &mut head) {
        log::warn!("bp_has_sre_payload: BP1 head read failed: {:?}", e);
        return false;
    }

    let is_fat = head[510] == 0x55 && head[511] == 0xAA;
    log::info!(
        "bp_has_sre_payload: FAT match = {} (head[0]={:#x}, head[510..512]=[{:#x},{:#x}])",
        is_fat,
        head[0],
        head[510],
        head[511]
    );
    is_fat
}

#[cfg(test)]
mod tests {
    extern crate alloc;

    use super::*;
    use core::sync::atomic::{AtomicU8, AtomicU32, AtomicU64, AtomicUsize, Ordering};

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
    fn test_build_file_path_node_overlong_path_falls_back_to_end_node() {
        // A path whose UTF-16 encoding overflows the 16-bit node length must
        // not emit a header that misdescribes the appended bytes; the
        // fallback is a bare END_ENTIRE node.
        let long_path = alloc::string::String::from_utf8(alloc::vec![b'a'; 40 * 1024]).unwrap();
        let bytes = build_file_path_node(&long_path);
        assert_eq!(bytes.as_slice(), &[0x7F, 0xFF, 0x04, 0x00]);
    }

    #[test]
    fn test_device_path_size_over_cap_returns_zero() {
        // A chain of valid nodes with no END_ENTIRE inside the 64 KiB cap
        // must be rejected via the cap. The buffer is real and larger than
        // the cap, so every header read the walk performs stays in-bounds.
        const NODE_LEN: usize = 1024;
        let mut path = alloc::vec![0u8; 72 * 1024];
        let mut off = 0;
        while off + NODE_LEN <= path.len() {
            path[off] = 0x01; // Hardware, arbitrary non-END type
            path[off + 1] = 0x01;
            path[off + 2] = (NODE_LEN & 0xFF) as u8;
            path[off + 3] = (NODE_LEN >> 8) as u8;
            off += NODE_LEN;
        }
        // SAFETY: the buffer outlives the call and exceeds the walk cap, so
        // all reads are in-bounds until the cap rejects the walk.
        assert_eq!(unsafe { device_path_size(path.as_ptr()) }, 0);
    }

    #[test]
    fn test_device_path_size_rejects_missing_end_at_cap() {
        let mut path = alloc::vec![0u8; DEVICE_PATH_MAX_BYTES + NODE_HDR_BYTES];

        // Three valid nodes consume exactly the cap without an END_ENTIRE
        // node. The walk must reject the path before attempting a header read
        // at the first byte beyond the cap.
        let first_node_len = DEVICE_PATH_MAX_BYTES - (2 * NODE_HDR_BYTES);
        path[0] = 0x01;
        path[1] = 0x01;
        path[2..4].copy_from_slice(&(first_node_len as u16).to_le_bytes());
        for offset in [first_node_len, first_node_len + NODE_HDR_BYTES] {
            path[offset] = 0x01;
            path[offset + 1] = 0x01;
            path[offset + 2..offset + 4].copy_from_slice(&(NODE_HDR_BYTES as u16).to_le_bytes());
        }

        // SAFETY: the buffer outlives the call and contains every header up
        // to the cap. The additional header-sized allocation avoids requiring
        // an out-of-bounds read to exercise this malformed path.
        assert_eq!(unsafe { device_path_size(path.as_ptr()) }, 0);
    }

    #[test]
    fn test_build_file_path_node_chainload_default() {
        let bytes = build_file_path_node(DEFAULT_BOOT_FILE_PATH);
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

    #[test]
    fn test_device_path_size_rejects_end_entire_with_invalid_length() {
        let dp: [u8; 8] = [0x7F, 0xFF, 0x08, 0x00, 0, 0, 0, 0];
        // SAFETY: dp is a valid allocation containing the malformed header.
        assert_eq!(unsafe { device_path_size(dp.as_ptr()) }, 0);
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
        assert_eq!(
            IDENTIFY_CDW0.load(Ordering::SeqCst) & 0xFF,
            nvme_pass_thru::OPCODE_IDENTIFY as u32
        );
        assert_eq!(
            IDENTIFY_CDW10.load(Ordering::SeqCst),
            nvme_pass_thru::IDENTIFY_CNS_CONTROLLER
        );
    }

    #[test]
    fn test_probe_max_transfer_mdts_0_yields_chunk_max() {
        MDTS_TO_REPORT.store(0, Ordering::SeqCst);
        let mut protocol = make_nvme_protocol(mock_identify_with_mdts);
        // SAFETY: protocol alive on stack.
        let chunk = probe_max_transfer(&mut protocol);
        assert_eq!(
            chunk, READ_CHUNK_MAX,
            "MDTS=0 (no limit) clamps to self-imposed ceiling"
        );
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

    #[test]
    fn test_read_bp_via_log_page_rejects_unaligned_sizes() {
        let mut protocol = make_nvme_protocol(mock_get_log_page_fails);
        let mut dest = [0u8; 64];
        // Zero and non-dword chunk sizes must be rejected before any command
        // is issued (the mock would fail the call if one were issued).
        assert!(read_bp_via_log_page(&mut protocol, 1, 0, &mut dest).is_err());
        assert!(read_bp_via_log_page(&mut protocol, 1, 6, &mut dest).is_err());
    }

    #[test]
    fn test_device_path_size_walks_and_rejects_malformed() {
        // Well-formed: single END_ENTIRE node.
        let end_only = [0x7Fu8, 0xFF, 0x04, 0x00];
        // SAFETY: well-formed, END-terminated byte stream.
        assert_eq!(unsafe { device_path_size(end_only.as_ptr()) }, 4);

        // Well-formed: one 6-byte node + END_ENTIRE.
        let usb = [0x03u8, 0x05, 0x06, 0x00, 0, 0, 0x7F, 0xFF, 0x04, 0x00];
        // SAFETY: as above.
        assert_eq!(unsafe { device_path_size(usb.as_ptr()) }, 10);

        // Malformed: node length 0 would never advance the walk.
        let zero_len = [0x01u8, 0x01, 0x00, 0x00, 0x7F, 0xFF, 0x04, 0x00];
        // SAFETY: the length guard returns before advancing past the node.
        assert_eq!(unsafe { device_path_size(zero_len.as_ptr()) }, 0);

        // Malformed: node length below the 4-byte header minimum.
        let short_len = [0x01u8, 0x01, 0x03, 0x00, 0x7F, 0xFF, 0x04, 0x00];
        // SAFETY: as above.
        assert_eq!(unsafe { device_path_size(short_len.as_ptr()) }, 0);
    }
}
