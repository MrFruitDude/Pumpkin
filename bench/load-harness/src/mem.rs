//! Memory measurement that does not depend on how hard the OS is reclaiming pages.
//!
//! Resident set size (what `sysinfo` reports) only counts pages that are in RAM right now. On a
//! host under memory pressure the OS compresses or swaps a server's idle pages, so its RSS falls
//! while the server still holds the memory: in the 2026-10-01 baseline Pumpkin's RSS fell from
//! 255 to 103 MB inside one constant-load window, with the host's swap 97% full. The headline
//! memory metric is therefore the process *footprint*: everything the process holds, wherever
//! the OS has put it.
//!
//! - macOS: `phys_footprint` from `proc_pid_rusage` (what Activity Monitor shows as "Memory";
//!   includes compressed and swapped-out anonymous memory).
//! - Linux: `Rss + Swap` from `/proc/<pid>/smaps_rollup`.
//! - elsewhere: not measured (`None`).
//!
//! For the Java servers the harness additionally records the live heap after a full GC, taken
//! with `jcmd` after the measurement window closes, so the forced collection never lands inside
//! the window.

/// Footprint of `pid` in MiB, or `None` where it cannot be measured.
pub fn footprint_mb(pid: u32) -> Option<f64> {
    footprint_bytes(pid).map(|b| b as f64 / 1_048_576.0)
}

#[cfg(target_os = "macos")]
fn footprint_bytes(pid: u32) -> Option<u64> {
    let mut info = std::mem::MaybeUninit::<libc::rusage_info_v2>::zeroed();
    // SAFETY: `info` is a correctly sized, writable rusage_info_v2 for flavor RUSAGE_INFO_V2.
    let rc = unsafe {
        libc::proc_pid_rusage(
            i32::try_from(pid).ok()?,
            libc::RUSAGE_INFO_V2,
            info.as_mut_ptr().cast::<libc::rusage_info_t>(),
        )
    };
    // SAFETY: proc_pid_rusage returned 0, so it filled the struct.
    (rc == 0).then(|| unsafe { info.assume_init() }.ri_phys_footprint)
}

#[cfg(target_os = "linux")]
fn footprint_bytes(pid: u32) -> Option<u64> {
    let text = std::fs::read_to_string(format!("/proc/{pid}/smaps_rollup")).ok()?;
    parse_smaps_rollup_kb(&text).map(|kb| kb * 1024)
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn footprint_bytes(_pid: u32) -> Option<u64> {
    None
}

/// `Rss + Swap` in kB from the text of `/proc/<pid>/smaps_rollup`. `None` if `Rss` is missing.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn parse_smaps_rollup_kb(text: &str) -> Option<u64> {
    let field = |name: &str| {
        text.lines().find_map(|line| {
            let rest = line.strip_prefix(name)?.strip_prefix(':')?;
            rest.trim().strip_suffix("kB")?.trim().parse::<u64>().ok()
        })
    };
    Some(field("Rss")? + field("Swap").unwrap_or(0))
}

/// Used heap in MiB from `jcmd <pid> GC.heap_info` output. Handles both the JDK 25+ form
/// (`total reserved …K, committed …K, used …K`) and the older `total …K, used …K`.
pub fn parse_heap_info_used_mb(text: &str) -> Option<f64> {
    let line = text
        .lines()
        .find(|l| l.contains(" heap ") && l.contains("used "))?;
    let used = line.split("used ").nth(1)?;
    let digits: String = used.chars().take_while(char::is_ascii_digit).collect();
    let unit = used[digits.len()..].chars().next()?;
    let value: f64 = digits.parse().ok()?;
    let mb = match unit {
        'K' => value / 1024.0,
        'M' => value,
        'G' => value * 1024.0,
        _ => return None,
    };
    Some(mb)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smaps_rollup_adds_swap_to_rss() {
        // Shape of /proc/<pid>/smaps_rollup on Linux 6.x.
        let text = "55d0c0a00000-7ffd8e5fe000 ---p 00000000 00:00 0                          [rollup]\n\
                    Rss:              123456 kB\n\
                    Pss:              120000 kB\n\
                    Pss_Anon:         100000 kB\n\
                    Anonymous:        100000 kB\n\
                    Swap:               2048 kB\n\
                    SwapPss:            2048 kB\n";
        assert_eq!(parse_smaps_rollup_kb(text), Some(125_504));
    }

    #[test]
    fn smaps_rollup_without_swap_or_rss() {
        assert_eq!(parse_smaps_rollup_kb("Rss:  10 kB\n"), Some(10));
        assert_eq!(parse_smaps_rollup_kb("Pss: 10 kB\nSwapPss: 3 kB\n"), None);
        // `SwapPss` must not be mistaken for `Swap`.
        assert_eq!(
            parse_smaps_rollup_kb("Rss: 10 kB\nSwapPss: 3 kB\n"),
            Some(10)
        );
    }

    #[test]
    fn heap_info_jdk26() {
        // Captured from Homebrew OpenJDK 26.0.1, G1, after `jcmd <pid> GC.run`.
        let text = "10360:\n\
                    garbage-first heap   total reserved 262144K, committed 262144K, used 105686K [0x0000000125000000, 0x0000000135000000)\n \
                    region size 1M, 0 eden (0M), 0 survivor (0M), 7 old (7M), 100 humongous (100M), 149 free (149M)\n";
        let mb = parse_heap_info_used_mb(text).unwrap();
        assert!((mb - 103.209).abs() < 0.01, "{mb}");
    }

    #[test]
    fn heap_info_older_format_and_garbage() {
        let text = " garbage-first heap   total 2097152K, used 524288K [0x0, 0x0)\n";
        assert_eq!(parse_heap_info_used_mb(text), Some(512.0));
        assert_eq!(
            parse_heap_info_used_mb("10360:\nCommand executed successfully\n"),
            None
        );
        assert_eq!(parse_heap_info_used_mb(""), None);
    }

    #[test]
    fn own_footprint_is_measured_where_supported() {
        let mb = footprint_mb(std::process::id());
        if cfg!(any(target_os = "macos", target_os = "linux")) {
            let mb = mb.expect("footprint of the test process");
            assert!(mb > 0.1 && mb < 65_536.0, "{mb}");
        } else {
            assert!(mb.is_none());
        }
    }
}
