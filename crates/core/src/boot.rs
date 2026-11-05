//! The machine's boot: its id changes with every boot.

use std::path::Path;

/// Where the kernel keeps the boot id, which changes with every boot.
pub const BOOT_ID: &str = "/proc/sys/kernel/random/boot_id";

/// The boot id in this file, "" when there is none.
pub fn boot_id(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_default().trim().to_string()
}
