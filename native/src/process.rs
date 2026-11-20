//! Reads UDP bindings owned by a running Minecraft process.

use std::collections::BTreeSet;
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

/// Returns candidate local UDP ports of Minecraft for Windows and Bedrock servers.
pub fn udp_ports() -> Result<Vec<u16>, String> {
    let mut system = System::new();
    system.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing()
            .with_exe(UpdateKind::OnlyIfNotSet)
            .with_cmd(UpdateKind::OnlyIfNotSet),
    );
    let games = system
        .processes()
        .iter()
        .filter_map(|(pid, process)| {
            let matches = process_name(&process.name().to_string_lossy())
                || process
                    .exe()
                    .and_then(|path| path.file_name())
                    .is_some_and(|name| process_name(&name.to_string_lossy()))
                || cfg!(target_os = "linux")
                    && process
                        .cmd()
                        .first()
                        .is_some_and(|name| process_name(&name.to_string_lossy()));
            matches.then_some(*pid)
        })
        .collect();
    ports_for(&games)
}

fn process_name(name: &str) -> bool {
    let basename = name.rsplit(['/', '\\']).next().unwrap_or_default();
    [
        "Minecraft.Windows.exe",
        "Minecraft.Windows",
        "bedrock_server.exe",
        "bedrock_server",
    ]
    .iter()
    .any(|expected| basename.eq_ignore_ascii_case(expected))
}

#[cfg(windows)]
fn ports_for(games: &BTreeSet<Pid>) -> Result<Vec<u16>, String> {
    use windows::Win32::NetworkManagement::IpHelper::{
        MIB_UDP6ROW_OWNER_PID, MIB_UDPROW_OWNER_PID,
    };
    use windows::Win32::Networking::WinSock::{AF_INET, AF_INET6};
    if games.is_empty() {
        return Ok(Vec::new());
    }
    let ipv4 = udp_table::<MIB_UDPROW_OWNER_PID>(AF_INET.0.into(), |row| {
        (row.dwOwningPid, row.dwLocalPort)
    })?;
    let ipv6 = udp_table::<MIB_UDP6ROW_OWNER_PID>(AF_INET6.0.into(), |row| {
        (row.dwOwningPid, row.dwLocalPort)
    })?;
    Ok(ipv4
        .into_iter()
        .chain(ipv6)
        .filter_map(|(pid, port)| {
            (port != 0 && games.contains(&Pid::from_u32(pid))).then_some(port)
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect())
}

#[cfg(windows)]
#[expect(unsafe_code, reason = "Windows IP Helper UDP owner table")]
fn udp_table<Row: Copy>(
    family: u32,
    fields: impl Fn(Row) -> (u32, u32),
) -> Result<Vec<(u32, u16)>, String> {
    use windows::Win32::NetworkManagement::IpHelper::{GetExtendedUdpTable, UDP_TABLE_OWNER_PID};
    const HEADER: usize = size_of::<u32>();
    const INSUFFICIENT_BUFFER: u32 = 122;
    let mut size = 0;
    let mut buffer = Vec::<u8>::new();
    for _ in 0..4 {
        let output = (!buffer.is_empty()).then(|| buffer.as_mut_ptr().cast());
        // SAFETY: output is either a size query or a live allocation of size bytes.
        let status = unsafe {
            GetExtendedUdpTable(output, &mut size, false, family, UDP_TABLE_OWNER_PID, 0)
        };
        if status == INSUFFICIENT_BUFFER {
            buffer.resize(size as usize, 0);
            continue;
        }
        if status != 0 {
            return Err(format!(
                "UDP owner table failed for address family {family}: Windows error {status}"
            ));
        }
        if buffer.len() < HEADER {
            return Err("invalid UDP owner table header".to_string());
        }
        // SAFETY: header is present and table byte storage may be unaligned.
        let count = unsafe { buffer.as_ptr().cast::<u32>().read_unaligned() } as usize;
        if count > (buffer.len() - HEADER) / size_of::<Row>() {
            return Err("truncated UDP owner table".to_string());
        }
        return Ok((0..count)
            .map(|index| {
                let offset = HEADER + index * size_of::<Row>();
                // SAFETY: the full row lies within the validated buffer.
                let row = unsafe { buffer.as_ptr().add(offset).cast::<Row>().read_unaligned() };
                let (pid, port) = fields(row);
                (pid, u16::from_be(port as u16))
            })
            .collect());
    }
    Err("UDP owner table changed repeatedly during enumeration".to_string())
}

#[cfg(target_os = "linux")]
fn ports_for(games: &BTreeSet<Pid>) -> Result<Vec<u16>, String> {
    use std::fs;
    let mut ports = BTreeSet::new();
    for pid in games {
        let base = format!("/proc/{pid}");
        let descriptors = match fs::read_dir(format!("{base}/fd")) {
            Ok(files) => files,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(format!("read game sockets for PID {pid}: {error}")),
        };
        let sockets = descriptors
            .flatten()
            .filter_map(|entry| {
                let link = fs::read_link(entry.path()).ok()?;
                let inode = link.to_str()?.strip_prefix("socket:[")?.strip_suffix(']')?;
                inode.parse::<u64>().ok()
            })
            .collect::<BTreeSet<_>>();
        for table in ["udp", "udp6"] {
            let path = format!("{base}/net/{table}");
            let content = match fs::read_to_string(&path) {
                Ok(content) => content,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(format!("read {path}: {error}")),
            };
            ports.extend(
                content
                    .lines()
                    .skip(1)
                    .filter_map(|line| {
                        let mut fields = line.split_ascii_whitespace();
                        let port = fields.nth(1)?.rsplit_once(':')?.1;
                        let inode = fields.nth(7)?.parse::<u64>().ok()?;
                        sockets
                            .contains(&inode)
                            .then(|| u16::from_str_radix(port, 16).ok())
                            .flatten()
                    })
                    .filter(|port| *port != 0),
            );
        }
    }
    Ok(ports.into_iter().collect())
}

#[cfg(not(any(windows, target_os = "linux")))]
fn ports_for(_: &BTreeSet<Pid>) -> Result<Vec<u16>, String> {
    Err("game UDP port enumeration is unavailable on this platform".to_string())
}

#[cfg(all(test, any(windows, target_os = "linux")))]
mod tests {
    use super::*;

    #[test]
    fn finds_ipv4_and_ipv6_ports_owned_by_the_selected_process() {
        let ipv4 = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let ipv6 = std::net::UdpSocket::bind("[::1]:0").unwrap();
        let own = BTreeSet::from([Pid::from_u32(std::process::id())]);
        let ports = ports_for(&own).unwrap();
        assert!(ports.contains(&ipv4.local_addr().unwrap().port()));
        assert!(ports.contains(&ipv6.local_addr().unwrap().port()));
        assert!(ports_for(&BTreeSet::new()).unwrap().is_empty());
    }
}
