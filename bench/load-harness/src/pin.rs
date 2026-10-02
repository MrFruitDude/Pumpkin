//! CPU pinning for the server and the bot swarm (Linux only).
//!
//! On Linux the server and the bots can be given disjoint CPU sets through `taskset`, so they
//! never compete with each other and, with the remaining load kept off those CPUs (for example
//! `isolcpus` or a cgroup cpuset for everything else), not with the rest of the host either.
//! macOS has no CPU affinity for user processes (thread affinity tags are hints and are ignored
//! on Apple Silicon), so asking for pinning there is an error instead of a silent no-op.

use eyre::{bail, eyre};

/// Parses a `taskset -c` style list such as `2-5,8`. Returns the CPUs sorted and deduplicated.
pub fn parse_cpu_list(list: &str) -> eyre::Result<Vec<usize>> {
    let mut cpus = Vec::new();
    for part in list.split(',').map(str::trim) {
        if part.is_empty() {
            bail!("empty entry in CPU list {list:?}");
        }
        let parse = |s: &str| {
            s.trim()
                .parse::<usize>()
                .map_err(|_| eyre!("bad CPU number {s:?} in {list:?}"))
        };
        match part.split_once('-') {
            Some((a, b)) => {
                let (a, b) = (parse(a)?, parse(b)?);
                if a > b {
                    bail!("descending CPU range {part:?} in {list:?}");
                }
                cpus.extend(a..=b);
            }
            None => cpus.push(parse(part)?),
        }
    }
    cpus.sort_unstable();
    cpus.dedup();
    Ok(cpus)
}

/// Checks a requested pinning before anything starts: supported OS, valid lists, CPUs that
/// exist, and no CPU shared between the server and the bots.
pub fn validate(
    server: Option<&str>,
    bots: Option<&str>,
    logical_cpus: usize,
    os_supports_pinning: bool,
) -> eyre::Result<()> {
    if server.is_none() && bots.is_none() {
        return Ok(());
    }
    if !os_supports_pinning {
        bail!(
            "CPU pinning (--server-cpus/--bots-cpus) needs Linux with taskset; this OS has no CPU \
             affinity for processes"
        );
    }
    let server = server.map(parse_cpu_list).transpose()?.unwrap_or_default();
    let bots = bots.map(parse_cpu_list).transpose()?.unwrap_or_default();
    if let Some(cpu) = server.iter().chain(&bots).find(|c| **c >= logical_cpus) {
        bail!("CPU {cpu} does not exist; this host has {logical_cpus} logical CPUs");
    }
    if let Some(cpu) = server.iter().find(|c| bots.contains(c)) {
        bail!("CPU {cpu} is in both --server-cpus and --bots-cpus");
    }
    Ok(())
}

/// The program and arguments that run `program args…` on `cpus` (unchanged when `None`).
pub fn wrap(
    program: std::ffi::OsString,
    args: Vec<std::ffi::OsString>,
    cpus: Option<&str>,
) -> (std::ffi::OsString, Vec<std::ffi::OsString>) {
    match cpus {
        None => (program, args),
        Some(list) => {
            let mut wrapped = vec!["-c".into(), list.into(), program];
            wrapped.extend(args);
            ("taskset".into(), wrapped)
        }
    }
}

pub const OS_SUPPORTS_PINNING: bool = cfg!(target_os = "linux");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_lists() {
        assert_eq!(parse_cpu_list("2-5,8").unwrap(), vec![2, 3, 4, 5, 8]);
        assert_eq!(parse_cpu_list(" 3 , 1,1-2").unwrap(), vec![1, 2, 3]);
        assert!(parse_cpu_list("").is_err());
        assert!(parse_cpu_list("5-2").is_err());
        assert!(parse_cpu_list("a").is_err());
        assert!(parse_cpu_list("1,,2").is_err());
    }

    #[test]
    fn validation() {
        assert!(validate(None, None, 4, false).is_ok());
        assert!(validate(Some("0-1"), None, 4, false).is_err());
        assert!(validate(Some("0-1"), Some("2-3"), 4, true).is_ok());
        assert!(validate(Some("0-2"), Some("2-3"), 4, true).is_err());
        assert!(validate(Some("0-4"), None, 4, true).is_err());
    }

    #[test]
    fn wraps_in_taskset() {
        let (p, a) = wrap(
            "java".into(),
            vec!["-jar".into(), "s.jar".into()],
            Some("2-3"),
        );
        assert_eq!(p, "taskset");
        assert_eq!(a, ["-c", "2-3", "java", "-jar", "s.jar"]);
        let (p, a) = wrap("pumpkin".into(), vec![], None);
        assert_eq!((p.as_os_str(), a.len()), ("pumpkin".as_ref(), 0));
    }
}
