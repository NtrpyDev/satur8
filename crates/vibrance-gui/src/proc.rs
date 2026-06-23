//! Enumerate running userspace programs, so the GUI can offer a "pick a running
//! game" list like VibranceGUI does on Windows.

use std::collections::BTreeSet;
use std::fs;

/// Distinct executable basenames of currently-running userspace processes,
/// sorted. Kernel threads (no `exe` link) are skipped, so this is the list of
/// real programs you could profile.
pub fn running_executables() -> Vec<String> {
    let mut set = BTreeSet::new();
    let Ok(entries) = fs::read_dir("/proc") else {
        return Vec::new();
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let pid = name.to_string_lossy();
        if !pid.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        // Resolve /proc/<pid>/exe -> the real binary; this also filters out
        // kernel threads (their exe link doesn't resolve).
        let Ok(target) = fs::read_link(format!("/proc/{pid}/exe")) else {
            continue;
        };
        if let Some(base) = target.file_name() {
            let base = base.to_string_lossy();
            // Skip the obvious non-games / our own tooling clutter.
            if base.is_empty() || base.starts_with("vibrance") {
                continue;
            }
            set.insert(base.to_string());
        }
    }
    set.into_iter().collect()
}
