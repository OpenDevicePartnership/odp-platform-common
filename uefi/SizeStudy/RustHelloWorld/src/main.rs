//! Hello World Rust DXE Driver
//!
//! Demonstrates how to build a DXE driver written in Rust.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation. All rights reserved.
//!
//! SPDX-License-Identifier: BSD-2-Clause-Patent
//!

#![cfg(target_os = "uefi")]
#![no_std]
#![no_main]

use arm_pl011_uart::{PL011Registers, Uart, UniqueMmioPointer};
use core::{ffi::c_void, fmt::Write, panic::PanicInfo, ptr::NonNull};
use spin::Mutex;

const UART_BASE: usize = 0x040D_0000;

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {}
}

//
// Simple logger implementation using UART
//

struct SimpleLogger {
    uart: Mutex<Option<Uart<'static>>>,
}

impl SimpleLogger {
    const fn new() -> Self {
        Self {
            uart: Mutex::new(None),
        }
    }

    fn init(&self) {
        let registers = NonNull::new(UART_BASE as *mut PL011Registers).unwrap();
        // SAFETY: UART_BASE is UART3's MMIO register block and access is serialized by the mutex.
        let registers = unsafe { UniqueMmioPointer::new(registers) };
        *self.uart.lock() = Some(Uart::new(registers));
    }
}

impl log::Log for SimpleLogger {
    fn enabled(&self, _metadata: &log::Metadata) -> bool {
        true
    }

    fn log(&self, record: &log::Record) {
        if self.enabled(record.metadata()) {
            if let Some(uart) = self.uart.lock().as_mut() {
                let _ = writeln!(uart, "[Rust {}] {}\r", record.level(), record.args());
            }
        }
    }

    fn flush(&self) {}
}

static LOGGER: SimpleLogger = SimpleLogger::new();

//
// Primary entry point
//

#[unsafe(no_mangle)]
pub extern "efiapi" fn efi_main(_image_handle: *mut c_void, _system_table: *mut c_void) -> usize {
    LOGGER.init();

    let _ = log::set_logger(&LOGGER).map(|()| log::set_max_level(log::LevelFilter::Info));
    log::info!("[XXXXXXXX] Test Rust Driver");

    0
}
