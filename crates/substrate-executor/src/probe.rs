//! The probe child (design 34 § 6.2): the program the capability probe's Pod runs under the
//! executor's `run`, which writes the argv it received as one JSON line. The driver renders the
//! argv with `$(HOME)`, `$$`, a space and a non-ASCII character in it, and holds the line it reads
//! back to the one [`report`] makes from the argv it sent, byte for byte: a shell or a variable
//! expansion anywhere between the Pod spec and `execve` changes it, and `exec.argv-only` stays
//! absent.
//!
//! When the driver states its [`ProbeTargets`] in [`TARGETS_ENV`], the child also observes, and
//! writes what it observed as a second line, an [`Observation`]; the argv line keeps its bytes:
//!
//! - **The heartbeat.** It forks a grandchild that leaves the child's session and process group,
//!   ignores `TERM`, `HUP` and `INT`, and rewrites [`ProbeTargets::heartbeat`] every
//!   [`HEARTBEAT_EVERY`] with its pid and a beat counter until it is killed: it never exits on its
//!   own. `heartbeat` is `true` once the child saw the file change within
//!   [`HEARTBEAT_SEEN_WITHIN`]. The driver then reads the file from the keeper after the container
//!   shows `terminated`: unchanged for 2 s is `confinement.whole-tree-kill`.
//! - **Egress.** Three attempts, concurrently, each bounded at [`ATTEMPT_BOUND`]: TCP to the
//!   Kubernetes Service host on port 443, a DNS query to the first `nameserver` of
//!   [`ProbeTargets::resolv_conf`], and TCP to [`ProbeTargets::egress_probe_address`]. Each is
//!   reported with its outcome; the driver states `confinement.no-egress` only when every one is
//!   [`EgressOutcome::blocked`].

use std::io::{self, Read as _};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpStream, UdpSocket};
#[cfg(target_os = "linux")]
use std::path::Path;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// The executor role the probe Pod's argv names after the executor's own path.
pub const PROBE_CHILD: &str = "probe-child";

/// The argv the child received, in order, `argv[0]` first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArgvReport {
    pub argv: Vec<String>,
}

/// The exact bytes `probe-child` writes for `argv`: one JSON object and a newline.
///
/// # Panics
///
/// Never: a list of strings always serializes.
pub fn report<S: AsRef<str>>(argv: &[S]) -> String {
    let report = ArgvReport {
        argv: argv.iter().map(|part| part.as_ref().to_owned()).collect(),
    };
    let mut line = serde_json::to_string(&report).expect("an argv report serializes");
    line.push('\n');
    line
}

/// The arguments the probe Pod's child runs with after the executor path and [`PROBE_CHILD`]: a
/// Kubernetes variable reference, a shell parameter, a space and a non-ASCII character, each of
/// which a shell or the kubelet's expansion would change.
pub const PROBE_ARGUMENTS: [&str; 4] = ["$(HOME)", "$$", "a b", "\u{e9}"];

/// The variable of the run spec's `env` in which the driver states the child's [`ProbeTargets`]
/// as one JSON object. Without it the child writes its argv line and nothing else.
pub const TARGETS_ENV: &str = "SUBSTRATE_PROBE_TARGETS";

/// The heartbeat's file name in the probe's volume: the keeper reads it at the volume's root.
pub const HEARTBEAT_FILE: &str = "substrate-probe-heartbeat";

/// How often the grandchild rewrites its heartbeat.
pub const HEARTBEAT_EVERY: Duration = Duration::from_millis(100);

/// How long the child waits to see its grandchild's heartbeat change before it reports it absent.
pub const HEARTBEAT_SEEN_WITHIN: Duration = Duration::from_secs(2);

/// The bound on each egress attempt (design 34 § 6.2).
pub const ATTEMPT_BOUND: Duration = Duration::from_secs(3);

/// The port of the Kubernetes Service the first attempt dials (design 34 § 6.2).
pub const KUBERNETES_SERVICE_PORT: u16 = 443;

/// The port of the nameserver the DNS attempt queries.
pub const NAMESERVER_PORT: u16 = 53;

/// The most bytes of the resolver configuration the child reads.
const RESOLV_CONF_BYTES: u64 = 65_536;

/// What the driver tells the probe child to observe, through [`TARGETS_ENV`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProbeTargets {
    /// The absolute path, in the probe's volume, the grandchild's heartbeat is written to.
    pub heartbeat: String,
    /// The Kubernetes Service host the keeper's environment states; absent when it states none,
    /// and the attempt is then [`EgressOutcome::Unread`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kubernetes_service_host: Option<IpAddr>,
    /// The resolver configuration whose first `nameserver` the DNS attempt queries.
    pub resolv_conf: String,
    /// The operator's address outside the cluster (`egress_probe_address`).
    pub egress_probe_address: SocketAddr,
}

impl ProbeTargets {
    /// The targets as the one JSON object [`TARGETS_ENV`] carries.
    ///
    /// # Panics
    ///
    /// Never: strings and addresses always serialize.
    pub fn encode(&self) -> String {
        serde_json::to_string(self).expect("probe targets serialize")
    }

    /// Parses [`TARGETS_ENV`]'s value.
    ///
    /// # Errors
    ///
    /// A value that is not exactly the closed targets object.
    pub fn parse(text: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(text)
    }
}

/// What the child observed, written as its second line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Observation {
    /// The grandchild's heartbeat was seen to change within [`HEARTBEAT_SEEN_WITHIN`].
    pub heartbeat: bool,
    /// One attempt per [`EgressTarget::ALL`], in that order.
    pub egress: Vec<EgressAttempt>,
}

impl Observation {
    /// The observation as one JSON line with its newline.
    ///
    /// # Panics
    ///
    /// Never: the observation is booleans, enums, addresses and integers.
    pub fn line(&self) -> String {
        let mut line = serde_json::to_string(self).expect("an observation serializes");
        line.push('\n');
        line
    }

    /// Parses `text` as exactly one observation line, its newline included; anything else is
    /// `None`.
    pub fn parse_line(text: &str) -> Option<Self> {
        let body = text.strip_suffix('\n')?;
        if body.contains('\n') {
            return None;
        }
        serde_json::from_str(body).ok()
    }
}

/// One egress attempt and what came of it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EgressAttempt {
    pub target: EgressTarget,
    /// The address dialled; absent when the target could not be read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub address: Option<SocketAddr>,
    pub outcome: EgressOutcome,
    /// The error's `errno`, when the attempt failed with one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub errno: Option<i32>,
    /// How long the attempt took, in milliseconds.
    pub elapsed_ms: u64,
}

/// What an egress attempt dials (design 34 § 6.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EgressTarget {
    /// TCP to the Kubernetes Service host, port [`KUBERNETES_SERVICE_PORT`].
    KubernetesService,
    /// A DNS query over UDP to the resolver configuration's first nameserver.
    Nameserver,
    /// TCP to the operator's `egress_probe_address`.
    EgressProbe,
}

impl EgressTarget {
    /// Every target, in the order the observation reports them.
    pub const ALL: [Self; 3] = [Self::KubernetesService, Self::Nameserver, Self::EgressProbe];
}

/// How an egress attempt ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EgressOutcome {
    /// The TCP handshake completed, or a DNS answer arrived.
    Reached,
    /// A reset or a port-unreachable answered (`ECONNREFUSED`): something beyond the Pod
    /// answered, so the packet left it.
    Refused,
    /// Nothing answered within the bound.
    TimedOut,
    /// No route to the network or the host (`ENETUNREACH`, `EHOSTUNREACH`).
    Unreachable,
    /// The attempt was denied before it left (`EPERM`, `EACCES`).
    Denied,
    /// Any other error.
    Failed,
    /// The target could not be read, so nothing was attempted.
    Unread,
}

impl EgressOutcome {
    /// Whether the attempt failed without anything beyond the Pod answering it: it timed out,
    /// found no route, or was denied. A reached or refused attempt, any other error and an unread
    /// target are not.
    pub const fn blocked(self) -> bool {
        matches!(self, Self::TimedOut | Self::Unreachable | Self::Denied)
    }
}

/// The first `nameserver` a resolver configuration names whose address parses, or `None`.
pub fn nameserver(text: &str) -> Option<IpAddr> {
    text.lines().find_map(|line| {
        let mut words = line.split_whitespace();
        if words.next() != Some("nameserver") {
            return None;
        }
        words.next()?.parse().ok()
    })
}

/// One TCP attempt to `address`, bounded at `bound`; [`EgressOutcome::Unread`] without one.
pub fn attempt_tcp(
    target: EgressTarget,
    address: Option<SocketAddr>,
    bound: Duration,
) -> EgressAttempt {
    let Some(address) = address else {
        return unread(target);
    };
    let started = Instant::now();
    let result = TcpStream::connect_timeout(&address, bound).map(drop);
    attempted(target, address, started, result)
}

/// One DNS query over UDP to `address`, bounded at `bound`; [`EgressOutcome::Unread`] without
/// one. Any datagram back from the nameserver is [`EgressOutcome::Reached`].
pub fn attempt_dns(address: Option<SocketAddr>, bound: Duration) -> EgressAttempt {
    let Some(address) = address else {
        return unread(EgressTarget::Nameserver);
    };
    let started = Instant::now();
    let result = query_dns(address, started + bound);
    attempted(EgressTarget::Nameserver, address, started, result)
}

/// The three attempts of design 34 § 6.2, concurrently, in [`EgressTarget::ALL`]'s order.
pub fn attempt_egress(targets: &ProbeTargets, bound: Duration) -> Vec<EgressAttempt> {
    let kubernetes = targets
        .kubernetes_service_host
        .map(|host| SocketAddr::new(host, KUBERNETES_SERVICE_PORT));
    let nameserver = read_resolv_conf(&targets.resolv_conf)
        .as_deref()
        .and_then(nameserver)
        .map(|host| SocketAddr::new(host, NAMESERVER_PORT));
    let egress = Some(targets.egress_probe_address);
    std::thread::scope(|scope| {
        let attempts = [
            scope.spawn(move || attempt_tcp(EgressTarget::KubernetesService, kubernetes, bound)),
            scope.spawn(move || attempt_dns(nameserver, bound)),
            scope.spawn(move || attempt_tcp(EgressTarget::EgressProbe, egress, bound)),
        ];
        attempts
            .into_iter()
            .zip(EgressTarget::ALL)
            .map(|(attempt, target)| {
                attempt.join().unwrap_or(EgressAttempt {
                    target,
                    address: None,
                    outcome: EgressOutcome::Failed,
                    errno: None,
                    elapsed_ms: 0,
                })
            })
            .collect()
    })
}

/// Starts the heartbeat grandchild, then makes the three egress attempts while it watches the
/// heartbeat move. The fork comes first, while this process has one thread.
#[cfg(target_os = "linux")]
pub fn observe(targets: &ProbeTargets) -> Observation {
    let path = Path::new(&targets.heartbeat);
    let started = start_heartbeat(path);
    std::thread::scope(|scope| {
        let watched = scope.spawn(|| started && heartbeat_moves(path, HEARTBEAT_SEEN_WITHIN));
        let egress = attempt_egress(targets, ATTEMPT_BOUND);
        Observation {
            heartbeat: watched.join().unwrap_or(false),
            egress,
        }
    })
}

/// Forks the heartbeat grandchild; `false` when the fork failed. Call it only while the process
/// has one thread: the grandchild runs ordinary code after the fork.
#[cfg(target_os = "linux")]
fn start_heartbeat(path: &Path) -> bool {
    // SAFETY: fork takes no arguments; the caller holds the process to one thread, so the
    // grandchild's copy has no lock another thread held.
    match unsafe { libc::fork() } {
        0 => beat(path),
        pid => pid > 0,
    }
}

/// The grandchild: leaves the child's session and process group, ignores `TERM`, `HUP` and
/// `INT`, lets go of the child's standard streams so the run's pipes close when the child ends,
/// and rewrites `path` every [`HEARTBEAT_EVERY`] with its pid and a beat counter, through a
/// rename, until it is killed.
#[cfg(target_os = "linux")]
fn beat(path: &Path) -> ! {
    // SAFETY: setsid, signal, open, dup2 and close take integers and a static C string; each
    // touches only this process's own session, dispositions and descriptors.
    unsafe {
        libc::setsid();
        for signal in [libc::SIGTERM, libc::SIGHUP, libc::SIGINT] {
            libc::signal(signal, libc::SIG_IGN);
        }
        let null = libc::open(c"/dev/null".as_ptr(), libc::O_RDWR | libc::O_CLOEXEC);
        for stream in 0..=2 {
            if null >= 0 {
                libc::dup2(null, stream);
            } else {
                libc::close(stream);
            }
        }
        if null > 2 {
            libc::close(null);
        }
    }
    let pid = std::process::id();
    let mut next = path.as_os_str().to_owned();
    next.push(".next");
    let next = std::path::PathBuf::from(next);
    let mut count: u64 = 0;
    loop {
        let written = std::fs::write(&next, format!("{pid} {count}\n"))
            .and_then(|()| std::fs::rename(&next, path));
        drop(written);
        count = count.wrapping_add(1);
        std::thread::sleep(HEARTBEAT_EVERY);
    }
}

/// Whether two readings of `path` within `within` differ: the grandchild wrote it at least twice.
#[cfg(target_os = "linux")]
fn heartbeat_moves(path: &Path, within: Duration) -> bool {
    let deadline = Instant::now() + within;
    let mut first: Option<Vec<u8>> = None;
    loop {
        if let Ok(now) = std::fs::read(path) {
            match &first {
                None => first = Some(now),
                Some(first) if *first != now => return true,
                Some(_) => {}
            }
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(HEARTBEAT_EVERY / 5);
    }
}

/// The resolver configuration's first [`RESOLV_CONF_BYTES`] as text, or `None`.
fn read_resolv_conf(path: &str) -> Option<String> {
    let mut text = String::new();
    std::fs::File::open(path)
        .ok()?
        .take(RESOLV_CONF_BYTES)
        .read_to_string(&mut text)
        .ok()?;
    Some(text)
}

/// One NS question for the root over UDP to `address`, waiting for any answer until `deadline`.
fn query_dns(address: SocketAddr, deadline: Instant) -> io::Result<()> {
    let local: SocketAddr = if address.is_ipv4() {
        (Ipv4Addr::UNSPECIFIED, 0).into()
    } else {
        (Ipv6Addr::UNSPECIFIED, 0).into()
    };
    let socket = UdpSocket::bind(local)?;
    socket.connect(address)?;
    let id = std::process::id().to_be_bytes();
    // Header: id, a recursion-desired query, one question; then the root name, type NS, class IN.
    let query = [
        id[2], id[3], 0x01, 0x00, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 2, 0, 1,
    ];
    socket.send(&query)?;
    let left = deadline.saturating_duration_since(Instant::now());
    if left.is_zero() {
        return Err(io::ErrorKind::TimedOut.into());
    }
    socket.set_read_timeout(Some(left))?;
    let mut answer = [0_u8; 512];
    socket.recv(&mut answer).map(drop)
}

fn unread(target: EgressTarget) -> EgressAttempt {
    EgressAttempt {
        target,
        address: None,
        outcome: EgressOutcome::Unread,
        errno: None,
        elapsed_ms: 0,
    }
}

fn attempted(
    target: EgressTarget,
    address: SocketAddr,
    started: Instant,
    result: io::Result<()>,
) -> EgressAttempt {
    let elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    let (outcome, errno) = match result {
        Ok(()) => (EgressOutcome::Reached, None),
        Err(error) => (classify(&error), error.raw_os_error()),
    };
    EgressAttempt {
        target,
        address: Some(address),
        outcome,
        errno,
        elapsed_ms,
    }
}

fn classify(error: &io::Error) -> EgressOutcome {
    match error.raw_os_error() {
        Some(libc::ECONNREFUSED) => return EgressOutcome::Refused,
        Some(libc::ETIMEDOUT | libc::EAGAIN) => return EgressOutcome::TimedOut,
        Some(libc::ENETUNREACH | libc::EHOSTUNREACH) => return EgressOutcome::Unreachable,
        Some(libc::EPERM | libc::EACCES) => return EgressOutcome::Denied,
        Some(_) => return EgressOutcome::Failed,
        None => {}
    }
    match error.kind() {
        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock => EgressOutcome::TimedOut,
        _ => EgressOutcome::Failed,
    }
}

#[cfg(test)]
mod tests {
    use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, TcpListener, UdpSocket};
    use std::time::Duration;

    use super::{
        ArgvReport, EgressAttempt, EgressOutcome, EgressTarget, Observation, PROBE_ARGUMENTS,
        ProbeTargets, attempt_dns, attempt_tcp, nameserver, report,
    };

    const SHORT: Duration = Duration::from_millis(300);

    #[test]
    fn the_report_is_one_json_line_that_round_trips_every_argument() {
        let argv = ["/x", "probe-child", "$(HOME)", "$$", "a b", "\u{e9}"];
        let line = report(&argv);
        let body = line.strip_suffix('\n').expect("one newline at the end");
        assert!(!body.contains('\n'));
        let parsed: ArgvReport = serde_json::from_str(body).expect("the line parses");
        assert_eq!(parsed.argv, argv);
        assert_eq!(PROBE_ARGUMENTS, argv[2..]);
    }

    #[test]
    fn the_nameserver_is_the_first_nameserver_line_whose_address_parses() {
        let resolv = "# kubelet\nsearch ns.svc.cluster.local svc.cluster.local\n\
                      nameserver not-an-address\nnameserver 10.96.0.10\nnameserver 192.0.2.53\n\
                      options ndots:5\n";
        assert_eq!(
            nameserver(resolv),
            Some(Ipv4Addr::new(10, 96, 0, 10).into())
        );
        assert_eq!(
            nameserver("nameserver\tfd00::a\n"),
            Some(Ipv6Addr::new(0xfd00, 0, 0, 0, 0, 0, 0, 0xa).into())
        );
        assert_eq!(nameserver("search cluster.local\n"), None);
        assert_eq!(nameserver("nameservers 10.0.0.1\n"), None);
        assert_eq!(nameserver(""), None);
    }

    #[test]
    fn a_tcp_attempt_is_reached_on_a_listener_and_refused_on_a_closed_port() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("a listener");
        let open = listener.local_addr().expect("its address");
        let reached = attempt_tcp(EgressTarget::EgressProbe, Some(open), SHORT);
        assert_eq!(reached.target, EgressTarget::EgressProbe);
        assert_eq!(reached.address, Some(open));
        assert_eq!(reached.outcome, EgressOutcome::Reached, "{reached:?}");
        assert_eq!(reached.errno, None);

        let closed = {
            let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("a listener");
            listener.local_addr().expect("its address")
        };
        let refused = attempt_tcp(EgressTarget::KubernetesService, Some(closed), SHORT);
        assert_eq!(refused.outcome, EgressOutcome::Refused, "{refused:?}");
        assert_eq!(refused.errno, Some(libc::ECONNREFUSED));
        assert!(
            !refused.outcome.blocked(),
            "a reset came from beyond the Pod"
        );
    }

    #[test]
    fn a_dns_attempt_is_reached_on_an_answer_timed_out_on_silence_and_refused_on_a_closed_port() {
        let answering = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).expect("a nameserver");
        let address = answering.local_addr().expect("its address");
        let server = std::thread::spawn(move || {
            let mut query = [0_u8; 512];
            let (length, from) = answering.recv_from(&mut query).expect("a query");
            answering
                .send_to(&query[..length], from)
                .expect("an answer");
            query[..length].to_vec()
        });
        let reached = attempt_dns(Some(address), SHORT);
        assert_eq!(reached.target, EgressTarget::Nameserver);
        assert_eq!(reached.outcome, EgressOutcome::Reached, "{reached:?}");
        let query = server.join().expect("the nameserver answered");
        assert_eq!(query.len(), 17, "one header and the root's NS question");
        assert_eq!(&query[4..6], [0, 1], "one question");

        let silent = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).expect("a silent nameserver");
        let started = std::time::Instant::now();
        let timed_out = attempt_dns(Some(silent.local_addr().expect("its address")), SHORT);
        assert_eq!(timed_out.outcome, EgressOutcome::TimedOut, "{timed_out:?}");
        assert!(timed_out.outcome.blocked());
        assert!(started.elapsed() >= SHORT, "the attempt waited its bound");
        assert!(started.elapsed() < SHORT * 4, "and not much longer");

        let closed = {
            let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).expect("a socket");
            socket.local_addr().expect("its address")
        };
        let refused = attempt_dns(Some(closed), SHORT);
        assert_eq!(refused.outcome, EgressOutcome::Refused, "{refused:?}");
    }

    #[test]
    fn an_attempt_without_a_target_is_unread_and_not_blocked() {
        for attempt in [
            attempt_tcp(EgressTarget::KubernetesService, None, SHORT),
            attempt_dns(None, SHORT),
        ] {
            assert_eq!(attempt.outcome, EgressOutcome::Unread, "{attempt:?}");
            assert_eq!(attempt.address, None);
            assert!(!attempt.outcome.blocked());
        }
    }

    #[test]
    fn only_a_timeout_no_route_or_a_local_denial_is_blocked() {
        let blocked = [
            EgressOutcome::TimedOut,
            EgressOutcome::Unreachable,
            EgressOutcome::Denied,
        ];
        for outcome in [
            EgressOutcome::Reached,
            EgressOutcome::Refused,
            EgressOutcome::TimedOut,
            EgressOutcome::Unreachable,
            EgressOutcome::Denied,
            EgressOutcome::Failed,
            EgressOutcome::Unread,
        ] {
            assert_eq!(outcome.blocked(), blocked.contains(&outcome), "{outcome:?}");
        }
    }

    #[test]
    fn the_targets_and_the_observation_are_closed_and_round_trip() {
        let targets = ProbeTargets {
            heartbeat: "/workspace/substrate-probe-heartbeat".to_owned(),
            kubernetes_service_host: Some(Ipv4Addr::new(10, 96, 0, 1).into()),
            resolv_conf: "/etc/resolv.conf".to_owned(),
            egress_probe_address: SocketAddr::from((Ipv4Addr::new(192, 0, 2, 10), 443)),
        };
        assert_eq!(
            ProbeTargets::parse(&targets.encode()).expect("parses"),
            targets
        );
        let mut open: serde_json::Value = serde_json::from_str(&targets.encode()).expect("JSON");
        open["shell"] = serde_json::json!("/bin/sh");
        assert!(ProbeTargets::parse(&open.to_string()).is_err());

        let observation = Observation {
            heartbeat: true,
            egress: vec![EgressAttempt {
                target: EgressTarget::Nameserver,
                address: Some(SocketAddr::from((Ipv4Addr::new(10, 96, 0, 10), 53))),
                outcome: EgressOutcome::TimedOut,
                errno: None,
                elapsed_ms: 3_000,
            }],
        };
        let line = observation.line();
        assert_eq!(
            line,
            "{\"heartbeat\":true,\"egress\":[{\"target\":\"nameserver\",\
             \"address\":\"10.96.0.10:53\",\"outcome\":\"timed-out\",\"elapsed_ms\":3000}]}\n"
        );
        assert_eq!(Observation::parse_line(&line), Some(observation));
        assert_eq!(
            Observation::parse_line(line.trim_end()),
            None,
            "a whole line"
        );
        assert_eq!(Observation::parse_line(&format!("{line}{line}")), None);
    }
}
