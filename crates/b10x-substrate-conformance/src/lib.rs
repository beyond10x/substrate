//! The clean-room conformance runner (design 31): certifies one deployed Substrate endpoint over
//! HTTPS and WSS from outside its process, container and checkout, and writes `report.json` and
//! `report.json.sha256`. Exit codes: 0 passed, 1 failed, 2 invalid input, 3 harness failure,
//! 4 incomplete.

mod cases;
mod cases_exec;
mod cli;
mod harden;
mod probe;
mod registry;
mod report;
mod run;
mod token;

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, SecondsFormat, Utc};
use clap::Parser as _;
use rustls::RootCertStore;
use rustls::pki_types::ServerName;

use crate::probe::{Probe, Reply, http_authority, https_authority, sha256_hex};
use crate::registry::{PINNED_CONTRACT, PINNED_SHA256, Registry};
use crate::report::{
    Bounds, CapabilitySnapshot, Contract, Declared, Endpoint, FORMAT, Report, Run, Runner,
    SnapshotRead,
};
use crate::run::{CORE_GROUPS, Ctx, GROUPS, Hook, Settings, State};
use crate::token::{Purpose, Reason, SharedSource, TokenSource};

/// The largest trust-root bundle the runner reads.
const MAX_TRUST_ROOTS: u64 = 1024 * 1024;

/// A stop before any case: invalid input (2) or a harness failure (3), with one closed code.
enum Stop {
    Invalid(&'static str),
    Harness(&'static str),
}

impl Stop {
    fn exit(self) -> ExitCode {
        match self {
            Self::Invalid(code) => {
                eprintln!("b10x-substrate-conformance: invalid input: {code}");
                ExitCode::from(2)
            }
            Self::Harness(code) => {
                eprintln!("b10x-substrate-conformance: harness failure: {code}");
                ExitCode::from(3)
            }
        }
    }
}

/// Runs the binary.
#[must_use]
pub fn main() -> ExitCode {
    if harden::harden().is_err() {
        return Stop::Harness("hardening-failed").exit();
    }
    let args = match cli::Args::try_parse() {
        Ok(args) => args,
        Err(error) => error.exit(),
    };
    let input = match Input::check(args) {
        Ok(input) => input,
        Err(stop) => return stop.exit(),
    };
    let Ok(runtime) = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
    else {
        return Stop::Harness("runtime-unavailable").exit();
    };
    match runtime.block_on(certify(input)) {
        Ok(code) => code,
        Err(stop) => stop.exit(),
    }
}

/// The checked command line.
struct Input {
    args: cli::Args,
    authority: String,
    roots: RootCertStore,
    roots_sha256: String,
    server_name: String,
    only: Option<BTreeSet<String>>,
    ops_authority: Option<String>,
}

fn absolute(path: &Path) -> bool {
    path.is_absolute()
}

fn environment(names: &[String]) -> Result<Vec<(String, OsString)>, Stop> {
    let mut pairs = Vec::new();
    for name in names {
        if !cli::is_environment_name(name) {
            return Err(Stop::Invalid("environment-name-invalid"));
        }
        if let Some(value) = std::env::var_os(name) {
            pairs.push((name.clone(), value));
        }
    }
    Ok(pairs)
}

impl Input {
    fn check(args: cli::Args) -> Result<Self, Stop> {
        let (authority, host) =
            https_authority(&args.endpoint).ok_or(Stop::Invalid("endpoint-invalid"))?;
        let mut pem = Vec::new();
        std::fs::File::open(&args.trust_roots)
            .and_then(|file| file.take(MAX_TRUST_ROOTS + 1).read_to_end(&mut pem))
            .map_err(|_| Stop::Invalid("trust-roots-unreadable"))?;
        if pem.len() as u64 > MAX_TRUST_ROOTS {
            return Err(Stop::Invalid("trust-roots-too-large"));
        }
        let roots = probe::trust_roots(&pem).ok_or(Stop::Invalid("trust-roots-invalid"))?;
        let server_name = args.server_name.clone().unwrap_or(host);
        ServerName::try_from(server_name.clone())
            .map_err(|_| Stop::Invalid("server-name-invalid"))?;
        for name in &args.forbidden_names {
            ServerName::try_from(name.clone())
                .map_err(|_| Stop::Invalid("forbidden-name-invalid"))?;
        }
        if args
            .token_command
            .as_deref()
            .is_some_and(|path| !absolute(path))
        {
            return Err(Stop::Invalid("token-command-not-absolute"));
        }
        if args
            .restart_hook
            .as_deref()
            .is_some_and(|path| !absolute(path))
        {
            return Err(Stop::Invalid("restart-hook-not-absolute"));
        }
        if !args.probe_shell.starts_with('/') {
            return Err(Stop::Invalid("probe-shell-not-absolute"));
        }
        environment(&args.token_env)?;
        environment(&args.hook_env)?;
        let ops_authority = match &args.ops_origin {
            Some(origin) => {
                Some(http_authority(origin).ok_or(Stop::Invalid("ops-origin-invalid"))?)
            }
            None => None,
        };
        let scopes: BTreeSet<&str> = args.scopes.iter().map(String::as_str).collect();
        if scopes.is_empty()
            || scopes.len() != args.scopes.len()
            || !scopes
                .iter()
                .all(|scope| cli::HOSTED_SCOPES.contains(scope))
        {
            return Err(Stop::Invalid("scopes-invalid"));
        }
        let only = if args.only.is_empty() {
            None
        } else {
            if !args
                .only
                .iter()
                .all(|group| GROUPS.contains(&group.as_str()))
            {
                return Err(Stop::Invalid("only-group-unknown"));
            }
            Some(args.only.iter().cloned().collect())
        };
        if args.out.symlink_metadata().is_ok() {
            return Err(Stop::Invalid("out-exists"));
        }
        Ok(Self {
            authority,
            roots,
            roots_sha256: sha256_hex(&pem),
            server_name,
            only,
            ops_authority,
            args,
        })
    }
}

fn timestamp(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(SecondsFormat::AutoSi, true)
}

/// A server-supplied word the closed report can carry, or `unrepresentable`.
fn word(value: &str) -> String {
    let fits = value.len() <= 64
        && value
            .bytes()
            .next()
            .is_some_and(|first| first.is_ascii_alphanumeric())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'+' | b'_' | b'-'));
    if fits {
        value.to_owned()
    } else {
        "unrepresentable".to_owned()
    }
}

fn source_revision() -> String {
    option_env!("SUBSTRATE_CONFORMANCE_SOURCE")
        .filter(|revision| {
            revision.len() == 40
                && revision
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        })
        .unwrap_or("unknown")
        .to_owned()
}

/// The token source the command line declares.
fn token_source(args: &cli::Args) -> Result<TokenSource, Stop> {
    match (&args.token_command, args.token_fd) {
        (Some(program), None) => Ok(TokenSource::Command {
            program: program.clone(),
            arguments: args.token_command_argv.clone(),
            environment: environment(&args.token_env)?,
        }),
        (None, Some(descriptor)) => TokenSource::adopt_socket(descriptor).map_err(Stop::Invalid),
        _ => Err(Stop::Invalid("token-provider-invalid")),
    }
}

/// The first `GET /v1/machine`: the raw body, judged before any case runs (§ 3, step 1).
async fn handshake(probe: &Probe, tokens: &SharedSource, scopes: &[String]) -> Result<Reply, Stop> {
    let mut reason = Reason::Request;
    loop {
        let token = tokens
            .fetch(reason, Purpose::Primary, scopes)
            .await
            .map_err(|_| Stop::Harness("token-unavailable"))?;
        let reply = probe
            .exchange("GET", "/v1/machine", Some(&token), None)
            .await
            .map_err(|fault| Stop::Harness(fault.code()))?;
        drop(token);
        if !reply.contract_pinned() {
            let shown = |value: Option<&str>| value.map_or_else(|| "none".to_owned(), word);
            eprintln!(
                "b10x-substrate-conformance: advertised contract {} {} is not {PINNED_CONTRACT} {PINNED_SHA256}",
                shown(reply.contract.as_deref()),
                shown(reply.contract_sha256.as_deref()),
            );
            return Err(Stop::Invalid("contract-unknown"));
        }
        if reply.status == 401
            && reason == Reason::Request
            && reply.error_code().as_deref() == Some(substrate_wire::AUTH_AUTHORITY_INVALID)
        {
            reason = Reason::Refresh;
            continue;
        }
        if reply.status != 200 {
            return Err(Stop::Harness("machine-refused"));
        }
        return Ok(reply);
    }
}

fn snapshot_read(snapshot: &substrate_wire::CapabilitySnapshot) -> SnapshotRead {
    SnapshotRead {
        snapshot: snapshot.snapshot.clone(),
        probed_at: timestamp(snapshot.probed_at),
    }
}

#[allow(clippy::too_many_lines, reason = "the run is one ordered procedure")]
async fn certify(input: Input) -> Result<ExitCode, Stop> {
    let started_at = Utc::now();
    let args = &input.args;
    let tokens: SharedSource = Arc::new(token_source(args)?);
    let io = Duration::from_secs(args.io_timeout);
    let server_name = ServerName::try_from(input.server_name.clone())
        .map_err(|_| Stop::Invalid("server-name-invalid"))?;
    let probe = Probe::new(
        input.authority.clone(),
        server_name,
        input.roots.clone(),
        io,
    );
    let first = handshake(&probe, &tokens, &args.scopes).await?;
    let first = cases::decode_machine(&first).ok_or(Stop::Harness("machine-undecodable"))?;
    let tls = probe
        .endpoint_tls()
        .await
        .map_err(|fault| Stop::Harness(fault.code()))?;
    let hook = args
        .restart_hook
        .as_ref()
        .map(|program| -> Result<Hook, Stop> {
            Ok(Hook {
                program: program.clone(),
                arguments: args.restart_hook_argv.clone(),
                environment: environment(&args.hook_env)?,
            })
        });
    let hook = hook.transpose()?;
    let settings = Settings {
        scopes: args.scopes.clone(),
        only: input.only.clone(),
        io,
        deadline: tokio::time::Instant::now() + Duration::from_secs(args.deadline),
        restart_timeout: Duration::from_secs(args.restart_timeout),
        probe_shell: args.probe_shell.clone(),
        max_generated_events: args.max_generated_events,
        hook,
        ops_authority: input.ops_authority.clone(),
        forbidden_names: args.forbidden_names.clone(),
        label: format!(
            "run-{}",
            ulid::Ulid::generate().to_string().to_ascii_lowercase()
        ),
        endpoint: format!("https://{}/", input.authority),
        trust_roots: args.trust_roots.clone(),
        server_name: input.server_name.clone(),
    };
    let client = tokio::time::timeout(io, run::connect(&settings, &tokens))
        .await
        .map_err(|_| Stop::Harness("io-timeout"))?
        .map_err(|_| Stop::Harness("sdk-connect-failed"))?;
    let facts =
        serde_json::to_value(&first.facts).map_err(|_| Stop::Harness("machine-undecodable"))?;
    let mut ctx = Ctx {
        registry: Registry::pinned(),
        facts,
        snapshot: first.snapshot.clone(),
        probe,
        tokens: Arc::clone(&tokens),
        client,
        settings,
        state: State::default(),
        cases: Vec::new(),
    };

    cases::machine(&mut ctx).await;
    cases::tls(&mut ctx).await;
    cases::auth(&mut ctx).await;
    cases::workspace(&mut ctx).await;
    cases::unserved(&mut ctx).await;
    cases::recovery(&mut ctx).await;
    cases_exec::exec(&mut ctx).await;
    cases_exec::resources(&mut ctx).await;
    cases_exec::sessions(&mut ctx).await;
    cases::events(&mut ctx).await;
    cases_exec::restart(&mut ctx).await;
    cases_exec::ops(&mut ctx).await;
    let last = match ctx.raw("GET", "/v1/machine", None).await {
        Ok(reply) if reply.status == 200 => cases::decode_machine(&reply),
        _ => None,
    };
    cases::snapshot_stable(
        &mut ctx,
        &first.snapshot,
        last.as_ref().map(|last| last.snapshot.as_str()),
    )
    .await;
    cases_exec::cleanup(&mut ctx).await;

    let partial = input.only.is_some();
    let (totals, verdict) = report::judge(&ctx.cases, &CORE_GROUPS, partial);
    for case in &ctx.cases {
        let status = match case.status {
            report::Status::Passed => "passed",
            report::Status::Failed => "failed",
            report::Status::Absent => "absent",
        };
        let detail = match (&case.absent, case.diagnostics.is_empty()) {
            (Some(absent), _) => format!(
                " ({})",
                serde_json::to_value(absent.reason)
                    .ok()
                    .and_then(|value| value.as_str().map(ToOwned::to_owned))
                    .unwrap_or_default()
            ),
            (None, false) => format!(" ({})", case.diagnostics.join(", ")),
            (None, true) => String::new(),
        };
        println!("{status} {}{detail}", case.id);
    }
    let verdict_word = match verdict {
        report::Verdict::Passed => "passed",
        report::Verdict::Failed => "failed",
        report::Verdict::Incomplete => "incomplete",
    };
    println!(
        "verdict: {verdict_word} (passed {}, failed {}, absent {})",
        totals.passed, totals.failed, totals.absent
    );
    let last = last.unwrap_or_else(|| first.clone());
    let report = Report {
        format: FORMAT.to_owned(),
        runner: Runner {
            version: env!("CARGO_PKG_VERSION").to_owned(),
            source_revision: source_revision(),
        },
        run: Run {
            started_at: timestamp(started_at),
            finished_at: timestamp(Utc::now()),
            label: ctx.settings.label.clone(),
            partial,
            only: input.only.iter().flatten().cloned().collect(),
        },
        endpoint: Endpoint {
            origin_sha256: sha256_hex(format!("https://{}/", input.authority).as_bytes()),
            server_name_sha256: sha256_hex(input.server_name.as_bytes()),
            tls_version: tls.tls_version,
            alpn: tls.alpn,
            leaf_certificate_sha256: tls.leaf_sha256,
            trust_roots_sha256: input.roots_sha256.clone(),
            leaf_not_after: tls.leaf_not_after,
        },
        contract: Contract {
            name: PINNED_CONTRACT.to_owned(),
            sha256: PINNED_SHA256.to_owned(),
        },
        capability_snapshot: CapabilitySnapshot {
            first: snapshot_read(&first),
            last: snapshot_read(&last),
            driver: serde_json::to_value(first.driver)
                .ok()
                .and_then(|value| value.as_str().map(word))
                .unwrap_or_else(|| "unrepresentable".to_owned()),
            driver_version: word(&first.driver_version),
            config_generation: first.config_generation,
        },
        declared: Declared {
            scopes: args
                .scopes
                .iter()
                .cloned()
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect(),
            token_provider: if args.token_fd.is_some() {
                "fd"
            } else {
                "command"
            }
            .to_owned(),
            restart_hook: args.restart_hook.is_some(),
            ops_origin: args.ops_origin.is_some(),
            forbidden_names: args.forbidden_names.len() as u64,
            bounds: Bounds {
                deadline_s: args.deadline,
                io_timeout_s: args.io_timeout,
                restart_timeout_s: args.restart_timeout,
                max_generated_events: args.max_generated_events,
            },
        },
        cases: ctx.cases,
        not_covered: report::not_covered(),
        totals,
        verdict,
    };
    let bytes = report::render(&report).map_err(|_| Stop::Harness("report-unrenderable"))?;
    let out: PathBuf = args.out.clone();
    report::create_out(&out).map_err(|_| Stop::Harness("out-uncreatable"))?;
    report::write(&out, &bytes).map_err(|_| Stop::Harness("report-unwritable"))?;
    Ok(ExitCode::from(verdict.exit_code()))
}
