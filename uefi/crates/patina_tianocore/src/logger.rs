//! Minimal UEFI logger that routes `log` output to ConOut and serial.
//!
//! SPDX-License-Identifier: MIT
//!
//! Automatically initialised by the `driver_entry!` macro. Writes to both
//! the UEFI console (ConOut) and COM1 serial port on x86_64 so output is
//! visible in QEMU's `-nographic` mode and the graphical console.
//!
//! On non-x86_64 targets the serial path is omitted (each architecture has
//! its own UART hardware; adding more is straightforward but out of scope
//! for this crate's first release). ConOut still works everywhere.

use core::fmt::Write;
use core::sync::atomic::{AtomicPtr, Ordering};

use r_efi::efi;
use r_efi::protocols::simple_text_output;

#[cfg(target_arch = "x86_64")]
use spin::Mutex;
#[cfg(target_arch = "x86_64")]
use uart_16550::SerialPort;

/// Global logger instance. Initialised by [`init`].
static LOGGER: UefiLogger = UefiLogger {
    con_out: AtomicPtr::new(core::ptr::null_mut()),
};

/// COM1 base I/O port. Standard for x86 PCs.
#[cfg(target_arch = "x86_64")]
const COM1_BASE: u16 = 0x3F8;

/// Lazy-initialised serial port for x86_64. Accessed only from [`init`] and
/// from the logger callback, both of which run at low frequency in DXE
/// phase, so spin contention is not a concern.
#[cfg(target_arch = "x86_64")]
static SERIAL: Mutex<Option<SerialPort>> = Mutex::new(None);

struct UefiLogger {
    con_out: AtomicPtr<simple_text_output::Protocol>,
}

// SAFETY: `con_out` is accessed exclusively through the atomic pointer.
unsafe impl Send for UefiLogger {}
unsafe impl Sync for UefiLogger {}

impl log::Log for UefiLogger {
    fn enabled(&self, _metadata: &log::Metadata) -> bool {
        true
    }

    fn log(&self, record: &log::Record) {
        if !self.enabled(record.metadata()) {
            return;
        }

        // Serial output (x86_64 only — other archs use ConOut only).
        #[cfg(target_arch = "x86_64")]
        {
            let mut guard = SERIAL.lock();
            if let Some(port) = guard.as_mut() {
                let _ = write!(port, "[{:<5}] {}\r\n", record.level(), record.args());
            }
        }

        // ConOut output, if available.
        let con_out = self.con_out.load(Ordering::Acquire);
        if !con_out.is_null() {
            let _ = write!(
                ConOutWriter { con_out },
                "[{:<5}] {}\r\n",
                record.level(),
                record.args()
            );
        }
    }

    fn flush(&self) {}
}

/// Initialise the UEFI logger.
///
/// Wires up ConOut from the system table and, on x86_64, also opens COM1
/// for serial output. Idempotent — repeated calls re-arm ConOut but do not
/// re-initialise the serial port.
///
/// # Safety
///
/// `system_table` must point to a valid UEFI system table that remains
/// valid for the lifetime of the driver.
pub unsafe fn init(system_table: *const efi::SystemTable) {
    // SAFETY: Caller guarantees system_table is valid.
    let con_out = unsafe { (*system_table).con_out };
    LOGGER.con_out.store(con_out, Ordering::Release);

    // Open the serial port once, on x86_64 only.
    #[cfg(target_arch = "x86_64")]
    {
        let mut guard = SERIAL.lock();
        if guard.is_none() {
            // SAFETY: COM1 is the standard PC serial port; we initialise it
            // exactly once (this branch runs at most one time).
            let mut port = unsafe { SerialPort::new(COM1_BASE) };
            port.init();
            *guard = Some(port);
        }
    }

    // Ignore set_logger error: worst case we get no log output.
    let _ = log::set_logger(&LOGGER);
    log::set_max_level(log::LevelFilter::Info);
}

// ---------------------------------------------------------------------------
// ConOut writer — writes via UEFI SimpleTextOutput.
// ---------------------------------------------------------------------------

struct ConOutWriter {
    con_out: *mut simple_text_output::Protocol,
}

impl Write for ConOutWriter {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        // UEFI expects UCS-2 (roughly UTF-16). We convert one character at a
        // time into a 2-element buffer [char, null].
        let mut buf = [0u16; 2];
        for c in s.chars() {
            // Only BMP characters fit in a single UCS-2 code unit.
            if (c as u32) < 0x10000 {
                buf[0] = c as u16;
                buf[1] = 0;
                // SAFETY: con_out is valid per init() contract; buf is null-terminated UCS-2.
                unsafe {
                    ((*self.con_out).output_string)(self.con_out, buf.as_mut_ptr());
                }
            }
        }
        Ok(())
    }
}
