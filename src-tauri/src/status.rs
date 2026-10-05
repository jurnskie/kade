//! Server status for the Status tab: one small shell script over the SSH
//! connection, parsed into numbers. Written for Linux (/proc); other systems
//! fill in what they can and leave the rest empty.

use russh::ChannelMsg;
use serde::Serialize;

use crate::error::{AppError, AppResult};
use crate::ssh::Session;

/// Sections start with "@name". POSIX sh, no bashisms; every command may fail.
const SCRIPT: &str = r#"export LC_ALL=C
echo @hostname; uname -n
echo @os; if [ -r /etc/os-release ]; then . /etc/os-release; echo "$PRETTY_NAME"; elif command -v sw_vers >/dev/null; then echo "$(sw_vers -productName) $(sw_vers -productVersion)"; else uname -s; fi
echo @kernel; uname -sr
echo @uptime; cat /proc/uptime 2>/dev/null
echo @loadavg; cat /proc/loadavg 2>/dev/null || sysctl -n vm.loadavg 2>/dev/null
echo @cpus; nproc 2>/dev/null || getconf _NPROCESSORS_ONLN 2>/dev/null
echo @meminfo; grep -E '^(MemTotal|MemAvailable|SwapTotal|SwapFree):' /proc/meminfo 2>/dev/null
echo @df; df -kP -x tmpfs -x devtmpfs -x overlay -x squashfs -x efivarfs 2>/dev/null || df -kP 2>/dev/null
echo @stat1; head -n 1 /proc/stat 2>/dev/null
sleep 0.5
echo @stat2; head -n 1 /proc/stat 2>/dev/null
echo @ps; ps -eo pid=,pcpu=,pmem=,comm= --sort=-pcpu 2>/dev/null | head -n 8
"#;

#[derive(Debug, Default, Clone, Serialize, PartialEq)]
pub struct Disk {
    pub mount: String,
    pub filesystem: String,
    pub size: u64,
    pub used: u64,
    pub available: u64,
}

#[derive(Debug, Default, Clone, Serialize, PartialEq)]
pub struct Process {
    pub pid: u32,
    pub cpu: f64,
    pub mem: f64,
    pub command: String,
}

#[derive(Debug, Default, Clone, Serialize, PartialEq)]
pub struct ServerStatus {
    pub hostname: Option<String>,
    pub os: Option<String>,
    pub kernel: Option<String>,
    pub uptime_secs: Option<u64>,
    /// 1, 5 and 15 minute load averages.
    pub load: Option<[f64; 3]>,
    pub cpus: Option<u32>,
    /// CPU busy percentage over the half second the script sleeps.
    pub cpu_percent: Option<f64>,
    pub mem_total: Option<u64>,
    pub mem_available: Option<u64>,
    pub swap_total: Option<u64>,
    pub swap_free: Option<u64>,
    pub disks: Vec<Disk>,
    pub processes: Vec<Process>,
}

pub async fn fetch(session: &Session) -> AppResult<ServerStatus> {
    let mut channel = session.ssh()?.channel_open_session().await?;
    channel.exec(true, SCRIPT).await?;
    let mut out = Vec::new();
    let read = async {
        while let Some(msg) = channel.wait().await {
            match msg {
                ChannelMsg::Data { data } => out.extend_from_slice(&data),
                ChannelMsg::Close => break,
                _ => {}
            }
        }
    };
    tokio::time::timeout(std::time::Duration::from_secs(10), read)
        .await
        .map_err(|_| AppError::other(tr!("The server took too long to answer", "De server antwoordde niet op tijd")))?;
    Ok(parse(&String::from_utf8_lossy(&out)))
}

fn sections(text: &str) -> std::collections::HashMap<&str, Vec<&str>> {
    let mut map = std::collections::HashMap::new();
    let mut current: Option<&str> = None;
    for line in text.lines() {
        if let Some(name) = line.strip_prefix('@') {
            current = Some(name.trim());
            map.entry(name.trim()).or_insert_with(Vec::new);
        } else if let Some(name) = current {
            if !line.trim().is_empty() {
                map.get_mut(name).unwrap().push(line);
            }
        }
    }
    map
}

/// Busy share between two "cpu …" lines of /proc/stat (idle + iowait are idle).
fn cpu_percent(a: &str, b: &str) -> Option<f64> {
    let nums = |l: &str| -> Option<Vec<u64>> {
        let rest = l.strip_prefix("cpu ")?;
        rest.split_whitespace().map(|n| n.parse().ok()).collect()
    };
    let (a, b) = (nums(a)?, nums(b)?);
    if a.len() < 4 || a.len() != b.len() {
        return None;
    }
    let total = |v: &[u64]| v.iter().take(8).sum::<u64>();
    let idle = |v: &[u64]| v[3] + v.get(4).copied().unwrap_or(0);
    let dt = total(&b).checked_sub(total(&a))?;
    let di = idle(&b).checked_sub(idle(&a))?;
    if dt == 0 {
        return Some(0.0);
    }
    Some(((dt - di.min(dt)) as f64 / dt as f64 * 100.0).clamp(0.0, 100.0))
}

pub fn parse(text: &str) -> ServerStatus {
    let s = sections(text);
    let first = |k: &str| s.get(k).and_then(|v| v.first()).map(|l| l.trim().to_string()).filter(|l| !l.is_empty());

    let load = first("loadavg").and_then(|l| {
        // Linux: "0.52 0.58 0.59 1/389 12345"; BSD sysctl: "{ 0.52 0.58 0.59 }".
        let v: Vec<f64> = l.trim_matches(|c| c == '{' || c == '}').split_whitespace().take(3).filter_map(|n| n.parse().ok()).collect();
        (v.len() == 3).then(|| [v[0], v[1], v[2]])
    });

    let mut mem = std::collections::HashMap::new();
    for line in s.get("meminfo").into_iter().flatten() {
        let mut parts = line.split_whitespace();
        if let (Some(key), Some(kb)) = (parts.next(), parts.next().and_then(|n| n.parse::<u64>().ok())) {
            mem.insert(key.trim_end_matches(':').to_string(), kb * 1024);
        }
    }

    let disks = s
        .get("df")
        .into_iter()
        .flatten()
        .skip(1) // header
        .filter_map(|line| {
            let f: Vec<&str> = line.split_whitespace().collect();
            if f.len() < 6 {
                return None;
            }
            let kb = |i: usize| f[i].parse::<u64>().ok().map(|n| n * 1024);
            let disk = Disk { filesystem: f[0].to_string(), size: kb(1)?, used: kb(2)?, available: kb(3)?, mount: f[5..].join(" ") };
            // Skip pseudo and tiny mounts (boot partitions, snaps, runtime dirs).
            let pseudo = disk.mount.starts_with("/snap/")
                || disk.mount.starts_with("/run")
                || disk.mount.starts_with("/sys")
                || disk.mount.starts_with("/dev");
            (!pseudo && disk.size >= 256 * 1024 * 1024).then_some(disk)
        })
        .fold(Vec::<Disk>::new(), |mut acc, d| {
            // One filesystem mounted several times (btrfs subvolumes, bind mounts): keep the shortest mount.
            match acc.iter_mut().find(|x| x.filesystem == d.filesystem && x.size == d.size) {
                Some(x) if d.mount.len() < x.mount.len() => *x = d,
                Some(_) => {}
                None => acc.push(d),
            }
            acc
        });

    let processes = s
        .get("ps")
        .into_iter()
        .flatten()
        .filter_map(|line| {
            let mut f = line.split_whitespace();
            Some(Process {
                pid: f.next()?.parse().ok()?,
                cpu: f.next()?.parse().ok()?,
                mem: f.next()?.parse().ok()?,
                command: f.collect::<Vec<_>>().join(" "),
            })
        })
        .filter(|p| p.command != "ps" && p.command != "head" && p.command != "sh")
        .take(6)
        .collect();

    ServerStatus {
        hostname: first("hostname"),
        os: first("os"),
        kernel: first("kernel"),
        uptime_secs: first("uptime").and_then(|l| l.split_whitespace().next()?.parse::<f64>().ok()).map(|f| f as u64),
        load,
        cpus: first("cpus").and_then(|l| l.parse().ok()),
        cpu_percent: first("stat1").zip(first("stat2")).and_then(|(a, b)| cpu_percent(&a, &b)),
        mem_total: mem.get("MemTotal").copied(),
        mem_available: mem.get("MemAvailable").copied(),
        swap_total: mem.get("SwapTotal").copied(),
        swap_free: mem.get("SwapFree").copied(),
        disks,
        processes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LINUX: &str = "@hostname
web-01
@os
Debian GNU/Linux 12 (bookworm)
@kernel
Linux 6.1.0-25-amd64
@uptime
350735.47 1388570.20
@loadavg
0.52 0.58 0.59 1/389 12345
@cpus
4
@meminfo
MemTotal:        8040132 kB
MemAvailable:    5123456 kB
SwapTotal:       1048572 kB
SwapFree:        1048572 kB
@df
Filesystem     1024-blocks     Used Available Capacity Mounted on
/dev/sda1         81000000 40500000  36000000      53% /
/dev/sda1         81000000 40500000  36000000      53% /var/lib/docker/btrfs
/dev/sda15          126678    11840    114838      10% /boot/efi
/dev/sdb1        500000000 100000000 400000000     20% /mnt/backup disk
@stat1
cpu  1000 0 500 8000 100 0 0 0 0 0
@stat2
cpu  1060 0 520 8100 120 0 0 0 0 0
@ps
  812  12.5  3.1 php-fpm8.3
    1   0.1  0.2 systemd
 9999   0.0  0.0 ps
";

    #[test]
    fn parses_linux_output() {
        let s = parse(LINUX);
        assert_eq!(s.hostname.as_deref(), Some("web-01"));
        assert_eq!(s.os.as_deref(), Some("Debian GNU/Linux 12 (bookworm)"));
        assert_eq!(s.uptime_secs, Some(350735));
        assert_eq!(s.load, Some([0.52, 0.58, 0.59]));
        assert_eq!(s.cpus, Some(4));
        assert_eq!(s.mem_total, Some(8040132 * 1024));
        assert_eq!(s.mem_available, Some(5123456 * 1024));
        // 200 ticks passed, 120 of them idle (100 idle + 20 iowait) → 40% busy.
        assert_eq!(s.cpu_percent.map(|p| p.round()), Some(40.0));
        // The duplicate mount and the small EFI partition are left out; spaces in mount points survive.
        let mounts: Vec<_> = s.disks.iter().map(|d| d.mount.as_str()).collect();
        assert_eq!(mounts, ["/", "/mnt/backup disk"]);
        assert_eq!(s.disks[0].used, 40500000 * 1024);
        let procs: Vec<_> = s.processes.iter().map(|p| p.command.as_str()).collect();
        assert_eq!(procs, ["php-fpm8.3", "systemd"]);
    }

    #[test]
    fn tolerates_missing_sections() {
        let s = parse("@hostname\nmac-mini\n@os\nmacOS 15.1\n@loadavg\n{ 1.20 1.10 0.90 }\n@meminfo\n@ps\n");
        assert_eq!(s.hostname.as_deref(), Some("mac-mini"));
        assert_eq!(s.load, Some([1.2, 1.1, 0.9]));
        assert_eq!(s.mem_total, None);
        assert_eq!(s.cpu_percent, None);
        assert!(s.disks.is_empty() && s.processes.is_empty());
    }
}
