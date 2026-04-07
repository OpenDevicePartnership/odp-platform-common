#![no_std]
#![no_main]

use core::{fmt::Write, panic::PanicInfo};
use log::{info, Level, LevelFilter, Metadata, Record};
use r_efi::efi::Status;
use spin::Mutex;
use uart_16550::SerialPort;

// This struct implements the log::Log trait to send log messages to a serial port (UART) for debugging purposes.
struct DebugLogger {
    uart: Mutex<SerialPort>,
}

// This is the constructor for the DebugLogger struct, which initializes the UART serial port at the specified I/O port address (0x402).
impl DebugLogger {
    const fn new() -> Self {
        Self {
            uart: Mutex::new(unsafe { SerialPort::new(0x402) }),
        }
    }
}

// Implementation of the log::Log trait for DebugLogger, allowing it to be used as a logger in the Rust logging framework.
impl log::Log for DebugLogger {
    // enabled() returns true for Info or lower log levels
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= Level::Info
    }

    // primary function to stream log messages to serial port
    fn log(&self, record: &Record) {
        if self.enabled(record.metadata()) {
            let mut buffer = heapless::String::<256>::new();

            let _ = write!(buffer, "[{:5}] {}\r\n", record.level(), record.args());

            let mut uart = self.uart.lock();
            for byte in buffer.as_bytes() {
                uart.send(*byte);
            }
        }
    }

    // function does nothing since the uart is synchronous and doesn't require flushing
    fn flush(&self) {}
}

// Create a static instance of the DebugLogger to be used as the global logger for the application.
static LOGGER: DebugLogger = DebugLogger::new();

// The entry point of the UEFI driver
#[no_mangle]
pub extern "efiapi" fn efi_main(
    _image_handle: *const core::ffi::c_void,
    _system_table: *const r_efi::system::SystemTable,
) -> u64 {
    let _ = log::set_logger(&LOGGER).map(|()| log::set_max_level(LevelFilter::Info));

    info!("Hello Rust UART DXE Demo!");

    Status::SUCCESS.as_usize() as u64
}

// The Rust panic handler necessary for no_std environments
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {}
}
