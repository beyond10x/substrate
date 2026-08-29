//! Package a substrate-wire contract bundle as a deterministic OCI image layout.
//!
//! ```console
//! cargo xtask package-bundle 0.4.0 --out <dir>
//! ```
//!
//! Reads `contracts/substrate-wire/<version>/` READ-ONLY and writes an OCI Image Layout
//! (`oci-layout`, `index.json`, `blobs/sha256/<hex>`) at `--out`. It never writes into
//! `contracts/` and never rewrites a bundle byte: AGENTS.md invariant 6 freezes every released
//! bundle directory, so the packager reads the bundle and the bundle learns nothing about the
//! packager. Publishing and signing the layout is separate release work and is not done here.
//!
//! This is the Rust successor of the Python packager retired by
//! `story:tooling-moves-to-cargo-xtask`, byte-identical in what it emits: the same layout, the
//! same canonical JSON, the same ustar bytes, the same refusals.
//!
//! # Layout choice
//!
//! One manifest, `application/vnd.oci.image.manifest.v1+json`:
//!
//! * **config = `bundle.json`, verbatim.** The config blob is the bundle's own `bundle.json`
//!   bytes, with media type `application/vnd.b10x.substrate-wire.bundle.v1+json`. Per OCI
//!   image-spec 1.1 a manifest with no explicit `artifactType` takes its artifact type from
//!   `config.mediaType`, so that media type is also the artifact type. The outer manifest digest
//!   pins `bundle.json`, which in turn carries the media type, byte length and digest of every
//!   other bundle path — no recursive self-hash, every distributed byte covered
//!   (`docs/design/07-specification-and-conformance.md` § 1).
//! * **One layer per bundle file**, not one tar layer, so each layer descriptor's digest equals
//!   the `sha256` that `bundle.json` already lists for that path and the two can be compared
//!   without unpacking anything. Each layer carries the file's declared media type and one
//!   annotation, `org.opencontainers.image.title`, holding the bundle-relative POSIX path.
//!   `bundle.json` itself is the config and is not repeated as a layer.
//! * **One final layer holding the declared source archive**, appended after the per-file layers
//!   so no existing layer descriptor moves. `packaging.json` declares the form the bundle is
//!   distributed in and this layer *is* that archive. Media type
//!   `application/vnd.b10x.substrate-wire.bundle.tar` — deliberately **not**
//!   `application/vnd.oci.image.layer.v1.tar`, which would claim rootfs semantics and invite a
//!   runtime to union it over the per-file layers instead of materialising it as one file.
//!
//! # The source archive
//!
//! `packaging.json.archive` is the specification; every field is honoured literally:
//!
//! * **ustar** — POSIX.1-1988: fixed 512-byte headers, no pax extended headers, no GNU sparse or
//!   long-name records, no per-archive globals. The headers are written here by hand, field by
//!   field, because nothing in `std` writes tar; they are byte-identical to what the Python
//!   predecessor's `tarfile.USTAR_FORMAT` wrote.
//! * **Directory entries are included**, each 0755, ahead of their contents, because
//!   `mode: files-0644-directories-0755` declares a directory mode.
//! * `uid` = `gid` = 0, `uname` = `gname` = `""`, files 0644 — no build account leaks into the
//!   bytes.
//! * every `mtime` = `SOURCE_DATE_EPOCH` = the author seconds of the last commit touching
//!   `contracts/substrate-wire/<version>/`, printed on stdout. `--source-date-epoch <int>`
//!   overrides it; with neither, the packager refuses rather than reaching for the clock.
//! * entry order is UTF-8 bytewise path order, which also puts each directory ahead of its
//!   contents; no compression.
//!
//! The archive holds `bundle.json` too — it is the source form of the whole directory, not of the
//! manifest's layer set — so extracting it reproduces the bundle directory byte for byte.
//!
//! # Determinism
//!
//! Two runs over identical bundle bytes produce byte-identical output: bytewise path order, no
//! timestamps, no build host, no absolute paths, no environment in any emitted document; canonical
//! JSON everywhere (sorted keys, two-space indent, one trailing newline, non-ASCII escaped);
//! fixed annotations only; blobs 0644 and directories 0755, and only referenced blobs exist.
//!
//! # Refusals (exit 2)
//!
//! `--out` inside any `contracts/` tree or a parent of one; `--out` non-empty without `--force`,
//! or holding anything that is not a previous layout; an unknown `<version>`; a symlink inside the
//! bundle; a file set disagreeing with `bundle.json`'s `files` list; no `SOURCE_DATE_EPOCH`; a
//! bundle path too long for a ustar header.
//!
//! Byte-level agreement between `bundle.json` and the files it lists is
//! `scripts/check-contract-bundle-<version>.py`'s job: a disagreement is reported on stderr and
//! the descriptors follow the actual bytes, so a changed byte always changes the manifest digest.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::{self, Display, Write as _};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, ExitCode};

use anyhow::Result;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::repo;

const BUNDLE_NAME: &str = "substrate-wire";

const CONFIG_MEDIA_TYPE: &str = "application/vnd.b10x.substrate-wire.bundle.v1+json";
const ARCHIVE_MEDIA_TYPE: &str = "application/vnd.b10x.substrate-wire.bundle.tar";
const MANIFEST_MEDIA_TYPE: &str = "application/vnd.oci.image.manifest.v1+json";
const INDEX_MEDIA_TYPE: &str = "application/vnd.oci.image.index.v1+json";
const TITLE_ANNOTATION: &str = "org.opencontainers.image.title";
const VERSION_ANNOTATION: &str = "org.opencontainers.image.version";
const REF_ANNOTATION: &str = "org.opencontainers.image.ref.name";
const STATUS_ANNOTATION: &str = "dev.b10x.contract.status";
const BUNDLE_STATUS: &str = "development";

/// The only names a `--force`d `--out` may hold: a previous layout and nothing else.
const LAYOUT_ENTRIES: [&str; 3] = ["blobs", "index.json", "oci-layout"];

const FILE_MODE: u32 = 0o644;
const DIRECTORY_MODE: u32 = 0o755;

/// The exit status of a refusal, as the Python predecessor's.
const REFUSAL_EXIT: u8 = 2;

/// ustar block size, and the record size a tar is padded up to.
const BLOCK: usize = 512;
const RECORD: usize = 10240;

/// The longest `name` and `prefix` a ustar header holds.
const LENGTH_NAME: usize = 100;
const LENGTH_PREFIX: usize = 155;

/// A named refusal: the packager declines and changes nothing.
#[derive(Debug)]
pub struct Refusal(String);

impl Refusal {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl Display for Refusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// `cargo xtask package-bundle <version> --out <dir>`.
#[derive(Debug, clap::Args)]
pub struct Args {
    /// Bundle version, e.g. `0.4.0`.
    pub version: String,
    /// Output directory for the OCI image layout.
    #[arg(long, value_name = "DIR")]
    pub out: String,
    /// `contracts/` tree to read (default: this repository's; tests point it at a copy so the
    /// checked-in bundle is never touched).
    #[arg(long, value_name = "DIR")]
    pub contracts_root: Option<String>,
    /// Overwrite a non-empty `--out` that holds a previous layout.
    #[arg(long)]
    pub force: bool,
    /// Every archive mtime, in seconds since the epoch (default: the author time of the last
    /// commit touching the bundle directory; without either the packager refuses rather than
    /// reading the clock).
    ///
    /// Hyphens are allowed through so that a negative value reaches the packager's own named
    /// refusal, exactly as it reached the predecessor's `argparse` `type=int`.
    #[arg(long, value_name = "SECONDS", allow_hyphen_values = true)]
    pub source_date_epoch: Option<i64>,
}

/// What one packaging run produced.
#[derive(Debug)]
pub struct Outcome {
    manifest: String,
    archive: String,
    archive_bytes: usize,
    source_date_epoch: i64,
    disagreeing: Vec<String>,
}

impl Outcome {
    /// The single stdout line: the manifest digest first, then `key=value` fields.
    pub fn line(&self) -> String {
        format!(
            "{} archive={} archive_bytes={} source_date_epoch={}",
            self.manifest, self.archive, self.archive_bytes, self.source_date_epoch
        )
    }

    /// The stderr note when `bundle.json` disagrees with the bytes it lists.
    fn warning(&self, version: &str) -> Option<String> {
        if self.disagreeing.is_empty() {
            return None;
        }
        let shown: Vec<&str> = self
            .disagreeing
            .iter()
            .take(5)
            .map(String::as_str)
            .collect();
        let ellipsis = if self.disagreeing.len() > 5 {
            " …"
        } else {
            ""
        };
        Some(format!(
            "warning: bundle.json disagrees with the bytes of {} path(s): {}{ellipsis}; \
             descriptors follow the bytes. \
             scripts/check-contract-bundle-{version}.py is the authority.",
            self.disagreeing.len(),
            shown.join(", ")
        ))
    }
}

pub fn run(args: &Args) -> Result<ExitCode> {
    let default_contracts = repo::root()?.join("contracts");
    match package(args, &default_contracts) {
        Ok(outcome) => {
            if let Some(warning) = outcome.warning(&args.version) {
                eprintln!("{warning}");
            }
            println!("{}", outcome.line());
            Ok(ExitCode::SUCCESS)
        }
        Err(refusal) => {
            eprintln!("package-bundle: {refusal}");
            Ok(ExitCode::from(REFUSAL_EXIT))
        }
    }
}

/// Package `args.version` out of `args.contracts_root`, defaulting to `default_contracts`.
pub fn package(args: &Args, default_contracts: &Path) -> Result<Outcome, Refusal> {
    let contracts_root = match &args.contracts_root {
        Some(raw) => resolve(&expand_user(raw)),
        None => resolve(default_contracts),
    };
    let roots = [contracts_root.clone(), resolve(default_contracts)];
    let out = resolve_out(&args.out, &roots)?;
    build(
        &args.version,
        &contracts_root,
        &out,
        args.force,
        args.source_date_epoch,
    )
}

fn build(
    version: &str,
    contracts_root: &Path,
    out: &Path,
    force: bool,
    epoch_override: Option<i64>,
) -> Result<Outcome, Refusal> {
    let bundle = contracts_root.join(BUNDLE_NAME).join(version);
    if !bundle.is_dir() {
        return Err(Refusal::new(format!(
            "no bundle at {}: unknown version {version}",
            bundle.display()
        )));
    }
    let manifest_json = bundle.join("bundle.json");
    if !manifest_json.is_file() {
        return Err(Refusal::new(format!(
            "no bundle.json in {}",
            bundle.display()
        )));
    }
    let config_bytes = read(&manifest_json)?;
    let listed = listed_files(&manifest_json, &config_bytes)?;

    let tree = bundle_tree(&bundle)?;
    let present: Vec<String> = tree
        .iter()
        .filter(|(path, is_directory)| !is_directory && path != "bundle.json")
        .map(|(path, _)| path.clone())
        .collect();
    describes(&manifest_json, &bundle, &listed, &present)?;

    // Resolved before anything is written: no SOURCE_DATE_EPOCH is a refusal, and a refusal must
    // leave --force's target directory as it found it.
    let epoch = source_date_epoch(&bundle, epoch_override)?;
    let archive = build_archive(&bundle, &tree, epoch)?;

    prepare_out(out, force)?;

    let (mut layers, disagreeing) = write_file_layers(&bundle, out, &listed, &present)?;
    // The declared source archive, last: appending it leaves every per-file layer descriptor
    // above exactly where it was.
    let archive_digest = write_blob(out, &archive)?;
    layers.push(descriptor(
        &format!("{version}.tar"),
        &archive_digest,
        ARCHIVE_MEDIA_TYPE,
        archive.len(),
    ));
    let manifest = write_metadata(out, version, &config_bytes, layers)?;

    Ok(Outcome {
        manifest,
        archive: archive_digest,
        archive_bytes: archive.len(),
        source_date_epoch: epoch,
        disagreeing,
    })
}

/// One `bundle.json` `files` entry, as far as the packager reads it.
struct Listed {
    media_type: String,
    sha256: Option<String>,
    byte_length: Option<u64>,
}

fn listed_files(
    manifest_json: &Path,
    config_bytes: &[u8],
) -> Result<BTreeMap<String, Listed>, Refusal> {
    let declared: Value = serde_json::from_slice(config_bytes).map_err(|error| {
        Refusal::new(format!("{} is not JSON: {error}", manifest_json.display()))
    })?;
    let entries = match declared.get("files").and_then(Value::as_array) {
        Some(entries) if !entries.is_empty() => entries,
        _ => {
            return Err(Refusal::new(format!(
                "{} lists no files",
                manifest_json.display()
            )));
        }
    };
    let mut listed = BTreeMap::new();
    for entry in entries {
        let (Some(path), Some(media_type)) = (
            entry.get("path").and_then(Value::as_str),
            entry.get("media_type").and_then(Value::as_str),
        ) else {
            return Err(Refusal::new(format!(
                "{}: files entry without path/media_type",
                manifest_json.display()
            )));
        };
        listed.insert(
            path.to_owned(),
            Listed {
                media_type: media_type.to_owned(),
                sha256: entry
                    .get("sha256")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned),
                byte_length: entry.get("byte_length").and_then(Value::as_u64),
            },
        );
    }
    Ok(listed)
}

fn describes(
    manifest_json: &Path,
    bundle: &Path,
    listed: &BTreeMap<String, Listed>,
    present: &[String],
) -> Result<(), Refusal> {
    let here: BTreeSet<&String> = present.iter().collect();
    let named: BTreeSet<&String> = listed.keys().collect();
    let missing: Vec<String> = named
        .difference(&here)
        .map(|path| (*path).clone())
        .collect();
    let extra: Vec<String> = here
        .difference(&named)
        .map(|path| (*path).clone())
        .collect();
    if missing.is_empty() && extra.is_empty() {
        return Ok(());
    }
    Err(Refusal::new(format!(
        "{} does not describe {}: listed-but-absent={} present-but-unlisted={}",
        manifest_json.display(),
        bundle.display(),
        python_list(&missing),
        python_list(&extra)
    )))
}

/// A Python list repr, so a refusal reads exactly as the predecessor's did.
fn python_list(values: &[String]) -> String {
    if values.is_empty() {
        return "[]".to_owned();
    }
    let items: Vec<String> = values.iter().map(|value| format!("'{value}'")).collect();
    format!("[{}]", items.join(", "))
}

/// Every directory and regular file, bundle-relative, sorted UTF-8 bytewise.
///
/// The flag is true for a directory. Bytewise order over the slash-bearing path puts a directory
/// immediately ahead of everything inside it, so this one order serves both the layer list and the
/// tar (`packaging.json.archive.path_order`).
fn bundle_tree(bundle: &Path) -> Result<Vec<(String, bool)>, Refusal> {
    let mut entries: Vec<(String, bool)> = Vec::new();
    let mut pending = vec![String::new()];
    while let Some(relative) = pending.pop() {
        let directory = if relative.is_empty() {
            bundle.to_path_buf()
        } else {
            bundle.join(&relative)
        };
        let listing = fs::read_dir(&directory).map_err(|error| {
            Refusal::new(format!("cannot read {}: {error}", directory.display()))
        })?;
        for entry in listing {
            let entry = entry.map_err(|error| {
                Refusal::new(format!("cannot read {}: {error}", directory.display()))
            })?;
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                return Err(Refusal::new(format!(
                    "refusing to package {}: path is not UTF-8",
                    entry.path().display()
                )));
            };
            let path = if relative.is_empty() {
                name.to_owned()
            } else {
                format!("{relative}/{name}")
            };
            let metadata = fs::symlink_metadata(entry.path()).map_err(|error| {
                Refusal::new(format!("cannot read {}: {error}", entry.path().display()))
            })?;
            if metadata.file_type().is_symlink() {
                return Err(Refusal::new(format!("refusing to package {path}: symlink")));
            }
            if metadata.is_dir() {
                entries.push((path.clone(), true));
                pending.push(path);
            } else if metadata.is_file() {
                entries.push((path, false));
            } else {
                return Err(Refusal::new(format!(
                    "refusing to package {path}: not a regular file or directory"
                )));
            }
        }
    }
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(entries)
}

/// `SOURCE_DATE_EPOCH`: the override, else the bundle's source commit.
///
/// `packaging.json.archive.source_date_epoch` is `source-commit-author-seconds`: the author time
/// of the last commit that touched the bundle directory. No commit and no override is a refusal —
/// the clock is never an input.
fn source_date_epoch(bundle: &Path, override_seconds: Option<i64>) -> Result<i64, Refusal> {
    if let Some(seconds) = override_seconds {
        if seconds < 0 {
            return Err(Refusal::new(format!(
                "--source-date-epoch must not be negative: {seconds}"
            )));
        }
        return Ok(seconds);
    }
    let output = Command::new("git")
        .arg("-C")
        .arg(bundle)
        .args(["log", "-1", "--format=%at", "--", "."])
        .output();
    let output = match output {
        Ok(output) => output,
        Err(error) => {
            return Err(Refusal::new(format!(
                "cannot run git to date {} ({error}); pass --source-date-epoch <int>",
                bundle.display()
            )));
        }
    };
    let seconds = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if !output.status.success() || seconds.is_empty() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        let detail = if stderr.is_empty() {
            "no commit touches this directory".to_owned()
        } else {
            stderr
        };
        return Err(Refusal::new(format!(
            "no source commit dates {} ({detail}); pass --source-date-epoch <int>",
            bundle.display()
        )));
    }
    seconds.parse::<i64>().map_err(|_| {
        Refusal::new(format!(
            "git returned a non-integer author time '{seconds}'"
        ))
    })
}

/// The `posix-tar` source archive `packaging.json` declares, as bytes.
fn build_archive(
    bundle: &Path,
    entries: &[(String, bool)],
    epoch: i64,
) -> Result<Vec<u8>, Refusal> {
    let mut archive: Vec<u8> = Vec::new();
    for (path, is_directory) in entries {
        if *is_directory {
            let header = ustar_header(&format!("{path}/"), DIRECTORY_MODE, 0, epoch, b'5')
                .map_err(|error| archive_refusal(bundle, &error))?;
            archive.extend_from_slice(&header);
            continue;
        }
        let data = read(&bundle.join(path))?;
        let header = ustar_header(path, FILE_MODE, length(data.len()), epoch, b'0')
            .map_err(|error| archive_refusal(bundle, &error))?;
        archive.extend_from_slice(&header);
        archive.extend_from_slice(&data);
        pad_to(&mut archive, BLOCK);
    }
    archive.resize(archive.len() + BLOCK * 2, 0);
    pad_to(&mut archive, RECORD);
    Ok(archive)
}

fn pad_to(buffer: &mut Vec<u8>, boundary: usize) {
    let remainder = buffer.len() % boundary;
    if remainder > 0 {
        buffer.resize(buffer.len() + boundary - remainder, 0);
    }
}

fn archive_refusal(bundle: &Path, error: &str) -> Refusal {
    Refusal::new(format!(
        "cannot write a ustar archive of {}: {error} (packaging.json declares format posix-tar; \
         a path that no longer fits a ustar header is a bundle decision, not a format fallback)",
        bundle.display()
    ))
}

/// One 512-byte ustar header, field by field.
///
/// The layout is POSIX.1-1988 and matches what `tarfile.USTAR_FORMAT` writes byte for byte:
/// numbers are zero-padded octal in `digits - 1` characters followed by NUL, `devmajor` and
/// `devminor` are NUL-filled rather than zero-numbered (a non-device entry has no device
/// numbers), and the checksum is the unsigned sum of all 512 bytes with the checksum field itself
/// read as eight spaces, written as six octal digits, a NUL and the space that was already there.
fn ustar_header(
    name: &str,
    mode: u32,
    size: u64,
    mtime: i64,
    typeflag: u8,
) -> Result<[u8; BLOCK], String> {
    let (prefix, name) = split_name(name)?;
    let mtime = u64::try_from(mtime).map_err(|_| "overflow in number field".to_owned())?;
    let mut header = [0u8; BLOCK];
    put(&mut header, 0, name.as_bytes());
    put(&mut header, 100, &itn(u64::from(mode & 0o7777), 8)?);
    put(&mut header, 108, &itn(0, 8)?);
    put(&mut header, 116, &itn(0, 8)?);
    put(&mut header, 124, &itn(size, 12)?);
    put(&mut header, 136, &itn(mtime, 12)?);
    put(&mut header, 148, b"        ");
    header[156] = typeflag;
    put(&mut header, 257, b"ustar\0");
    put(&mut header, 263, b"00");
    put(&mut header, 345, prefix.as_bytes());
    let checksum: u32 = header.iter().map(|byte| u32::from(*byte)).sum();
    put(&mut header, 148, format!("{checksum:06o}\0").as_bytes());
    Ok(header)
}

fn put(header: &mut [u8; BLOCK], offset: usize, value: &[u8]) {
    header[offset..offset + value.len()].copy_from_slice(value);
}

/// A ustar numeric field: `digits - 1` octal characters, zero-padded, then NUL.
fn itn(value: u64, digits: usize) -> Result<Vec<u8>, String> {
    if u128::from(value) >= 1u128 << (3 * (digits - 1)) {
        return Err("overflow in number field".to_owned());
    }
    let mut field = format!("{value:0width$o}", width = digits - 1).into_bytes();
    field.push(0);
    Ok(field)
}

/// The `prefix`/`name` split a ustar header needs, or a refusal.
///
/// Every bundle path fits the 100-byte `name` field today (the longest is 58 bytes), so nothing
/// needs the split; a path that no longer fits and cannot be split is a refusal, not a silent
/// upgrade to pax or GNU long names.
fn split_name(name: &str) -> Result<(String, String), String> {
    if name.len() <= LENGTH_NAME {
        return Ok((String::new(), name.to_owned()));
    }
    let components: Vec<&str> = name.split('/').collect();
    for index in 1..components.len() {
        let prefix = components[..index].join("/");
        let rest = components[index..].join("/");
        if prefix.len() <= LENGTH_PREFIX && rest.len() <= LENGTH_NAME {
            return Ok((prefix, rest));
        }
    }
    Err("name is too long".to_owned())
}

fn write_file_layers(
    bundle: &Path,
    out: &Path,
    listed: &BTreeMap<String, Listed>,
    present: &[String],
) -> Result<(Vec<Value>, Vec<String>), Refusal> {
    let mut layers = Vec::with_capacity(present.len());
    let mut disagreeing = Vec::new();
    for path in present {
        let entry = &listed[path];
        let data = read(&bundle.join(path))?;
        let digest = write_blob(out, &data)?;
        let hex = digest.strip_prefix("sha256:").unwrap_or(&digest);
        if entry.sha256.as_deref() != Some(hex) || entry.byte_length != Some(length(data.len())) {
            disagreeing.push(path.clone());
        }
        layers.push(descriptor(path, &digest, &entry.media_type, data.len()));
    }
    Ok((layers, disagreeing))
}

fn write_metadata(
    out: &Path,
    version: &str,
    config_bytes: &[u8],
    layers: Vec<Value>,
) -> Result<String, Refusal> {
    let annotations = vec![
        (STATUS_ANNOTATION, Value::from(BUNDLE_STATUS)),
        (VERSION_ANNOTATION, Value::from(version)),
    ];
    let config = object(vec![
        ("digest", Value::from(write_blob(out, config_bytes)?)),
        ("mediaType", Value::from(CONFIG_MEDIA_TYPE)),
        ("size", Value::from(length(config_bytes.len()))),
    ]);
    let manifest = object(vec![
        ("annotations", object(annotations.clone())),
        ("config", config),
        ("layers", Value::Array(layers)),
        ("mediaType", Value::from(MANIFEST_MEDIA_TYPE)),
        ("schemaVersion", Value::from(2)),
    ]);
    let manifest_bytes = canonical_json(&manifest);
    let manifest_digest = write_blob(out, &manifest_bytes)?;

    let mut entry_annotations = annotations;
    entry_annotations.push((REF_ANNOTATION, Value::from(version)));
    let entry = object(vec![
        ("annotations", object(entry_annotations)),
        ("digest", Value::from(manifest_digest.clone())),
        ("mediaType", Value::from(MANIFEST_MEDIA_TYPE)),
        ("size", Value::from(length(manifest_bytes.len()))),
    ]);
    let index = object(vec![
        ("manifests", Value::Array(vec![entry])),
        ("mediaType", Value::from(INDEX_MEDIA_TYPE)),
        ("schemaVersion", Value::from(2)),
    ]);
    let layout = object(vec![("imageLayoutVersion", Value::from("1.0.0"))]);
    write_layout_file(out, "oci-layout", &canonical_json(&layout))?;
    write_layout_file(out, "index.json", &canonical_json(&index))?;
    Ok(manifest_digest)
}

fn descriptor(title: &str, digest: &str, media_type: &str, size: usize) -> Value {
    object(vec![
        (
            "annotations",
            object(vec![(TITLE_ANNOTATION, Value::from(title))]),
        ),
        ("digest", Value::from(digest)),
        ("mediaType", Value::from(media_type)),
        ("size", Value::from(length(size))),
    ])
}

/// A JSON object; `serde_json`'s map is a `BTreeMap`, so the keys come out sorted.
fn object(pairs: Vec<(&str, Value)>) -> Value {
    let mut map = Map::new();
    for (key, value) in pairs {
        map.insert(key.to_owned(), value);
    }
    Value::Object(map)
}

/// Sorted keys, two-space indent, no trailing whitespace, one final newline, non-ASCII escaped.
///
/// The last of those is what `json.dumps(..., indent=2, sort_keys=True)` did: `ensure_ascii` is on
/// by default, so every code point above `~` leaves as `\uXXXX`. Non-ASCII can only occur inside a
/// JSON string, so escaping it after the fact is the same document.
fn canonical_json(value: &Value) -> Vec<u8> {
    let text = serde_json::to_string_pretty(value).unwrap_or_default();
    let mut escaped = escape_non_ascii(&text);
    escaped.push('\n');
    escaped.into_bytes()
}

fn escape_non_ascii(text: &str) -> String {
    if text.bytes().all(|byte| byte.is_ascii() && byte != 0x7f) {
        return text.to_owned();
    }
    let mut escaped = String::with_capacity(text.len());
    for character in text.chars() {
        if character.is_ascii() && character != '\u{7f}' {
            escaped.push(character);
            continue;
        }
        let mut units = [0u16; 2];
        for unit in character.encode_utf16(&mut units) {
            let _ = write!(escaped, "\\u{unit:04x}");
        }
    }
    escaped
}

fn write_blob(out: &Path, data: &[u8]) -> Result<String, Refusal> {
    let digest = sha256_hex(data);
    let blobs = out.join("blobs");
    let directory = blobs.join("sha256");
    fs::create_dir_all(&directory)
        .map_err(|error| Refusal::new(format!("cannot create {}: {error}", directory.display())))?;
    set_mode(&blobs, DIRECTORY_MODE)?;
    set_mode(&directory, DIRECTORY_MODE)?;
    let blob = directory.join(&digest);
    fs::write(&blob, data)
        .map_err(|error| Refusal::new(format!("cannot write {}: {error}", blob.display())))?;
    set_mode(&blob, FILE_MODE)?;
    Ok(format!("sha256:{digest}"))
}

fn write_layout_file(out: &Path, name: &str, data: &[u8]) -> Result<(), Refusal> {
    let target = out.join(name);
    fs::write(&target, data)
        .map_err(|error| Refusal::new(format!("cannot write {}: {error}", target.display())))?;
    set_mode(&target, FILE_MODE)
}

fn set_mode(path: &Path, mode: u32) -> Result<(), Refusal> {
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
        .map_err(|error| Refusal::new(format!("cannot chmod {}: {error}", path.display())))
}

fn read(path: &Path) -> Result<Vec<u8>, Refusal> {
    fs::read(path).map_err(|error| Refusal::new(format!("cannot read {}: {error}", path.display())))
}

fn sha256_hex(data: &[u8]) -> String {
    hex::encode(Sha256::digest(data))
}

fn length(value: usize) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}

fn resolve_out(raw: &str, contracts_roots: &[PathBuf]) -> Result<PathBuf, Refusal> {
    let out = resolve(&expand_user(raw));
    for root in contracts_roots {
        if out.starts_with(root) {
            return Err(Refusal::new(format!(
                "refusing to write into contracts/: {} is inside {} \
                 (AGENTS.md invariant 6 — a released bundle directory is immutable)",
                out.display(),
                root.display()
            )));
        }
        if root.starts_with(&out) {
            return Err(Refusal::new(format!(
                "refusing to write to {}: it contains the contracts/ tree at {}",
                out.display(),
                root.display()
            )));
        }
    }
    Ok(out)
}

fn prepare_out(out: &Path, force: bool) -> Result<(), Refusal> {
    if out.exists() && !out.is_dir() {
        return Err(Refusal::new(format!(
            "refusing to write to {}: not a directory",
            out.display()
        )));
    }
    if out.is_dir() {
        let mut entries: Vec<String> = fs::read_dir(out)
            .map_err(|error| Refusal::new(format!("cannot read {}: {error}", out.display())))?
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        entries.sort();
        if !entries.is_empty() && !force {
            return Err(Refusal::new(format!(
                "refusing to overwrite non-empty {} ({} entries); pass --force",
                out.display(),
                entries.len()
            )));
        }
        let foreign: Vec<&str> = entries
            .iter()
            .map(String::as_str)
            .filter(|name| !LAYOUT_ENTRIES.contains(name))
            .collect();
        if !foreign.is_empty() {
            return Err(Refusal::new(format!(
                "refusing to --force {}: it holds entries that are not part of an OCI image \
                 layout ({})",
                out.display(),
                foreign.join(", ")
            )));
        }
        for name in &entries {
            let target = out.join(name);
            let removed = if target.is_dir() {
                fs::remove_dir_all(&target)
            } else {
                fs::remove_file(&target)
            };
            removed.map_err(|error| {
                Refusal::new(format!("cannot remove {}: {error}", target.display()))
            })?;
        }
    }
    fs::create_dir_all(out)
        .map_err(|error| Refusal::new(format!("cannot create {}: {error}", out.display())))?;
    set_mode(out, DIRECTORY_MODE)
}

/// `~` and `~/…` against `HOME`, as `Path.expanduser()` did for the argv the packager reads.
fn expand_user(raw: impl AsRef<Path>) -> PathBuf {
    let raw = raw.as_ref();
    let Ok(text) = raw.strip_prefix("~") else {
        return raw.to_path_buf();
    };
    match std::env::var_os("HOME") {
        Some(home) => PathBuf::from(home).join(text),
        None => raw.to_path_buf(),
    }
}

/// `Path.resolve()`: absolute, symlink-free, `..`-free, and defined for a path that does not exist.
///
/// `std`'s `canonicalize` refuses a path with a missing component, and `--out` names a directory
/// that is usually not there yet, so the walk is done here: each component is appended, a symlink
/// is expanded in place, `..` pops, and a missing component is simply kept.
fn resolve(path: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("/"))
            .join(path)
    };
    let mut pending: Vec<PathBuf> = Vec::new();
    push_components(&absolute, &mut pending);
    let mut resolved = PathBuf::from("/");
    let mut budget = 64u32;
    while let Some(part) = pending.pop() {
        if part.as_os_str() == ".." {
            resolved.pop();
            continue;
        }
        resolved.push(&part);
        if budget == 0 {
            continue;
        }
        if let Ok(target) = fs::read_link(&resolved) {
            budget -= 1;
            resolved.pop();
            if target.is_absolute() {
                resolved = PathBuf::from("/");
            }
            push_components(&target, &mut pending);
        }
    }
    resolved
}

/// Push `path`'s normal components onto `pending` in reverse, so popping walks them in order.
fn push_components(path: &Path, pending: &mut Vec<PathBuf>) {
    let mut parts: Vec<PathBuf> = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(name) => parts.push(PathBuf::from(name)),
            Component::ParentDir => parts.push(PathBuf::from("..")),
            Component::RootDir | Component::CurDir | Component::Prefix(_) => {}
        }
    }
    parts.reverse();
    pending.extend(parts);
}

#[cfg(test)]
mod tests {
    use super::{Args, Outcome, Refusal, package, sha256_hex};
    use crate::repo;
    use serde_json::Value;
    use std::collections::{BTreeMap, BTreeSet};
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use tempfile::TempDir;

    const VERSION: &str = "0.4.0";
    const ARCHIVE_MEDIA_TYPE: &str = "application/vnd.b10x.substrate-wire.bundle.tar";
    const CONFIG_MEDIA_TYPE: &str = "application/vnd.b10x.substrate-wire.bundle.v1+json";
    const MANIFEST_MEDIA_TYPE: &str = "application/vnd.oci.image.manifest.v1+json";
    const TITLE_ANNOTATION: &str = "org.opencontainers.image.title";

    /// A bundle copied under `$TMPDIR` carries no git history, so tests that read a scratch
    /// contracts root pin `SOURCE_DATE_EPOCH` instead of deriving it.
    const SCRATCH_EPOCH: i64 = 1_700_000_000;

    fn contracts() -> PathBuf {
        repo::root().expect("workspace root").join("contracts")
    }

    fn bundle() -> PathBuf {
        contracts().join("substrate-wire").join(VERSION)
    }

    fn scratch(prefix: &str) -> TempDir {
        tempfile::Builder::new()
            .prefix(prefix)
            .tempdir()
            .expect("temporary directory")
    }

    fn args(out: &Path) -> Args {
        Args {
            version: VERSION.to_owned(),
            out: out.display().to_string(),
            contracts_root: None,
            force: false,
            source_date_epoch: None,
        }
    }

    /// The same arguments against a scratch copy, which has no git history to date it.
    fn scratch_args(out: &Path, contracts_root: &Path) -> Args {
        Args {
            contracts_root: Some(contracts_root.display().to_string()),
            source_date_epoch: Some(SCRATCH_EPOCH),
            ..args(out)
        }
    }

    fn pack(args: &Args) -> Outcome {
        package(args, &contracts()).unwrap_or_else(|refusal| panic!("packaging refused: {refusal}"))
    }

    fn refuse(args: &Args) -> Refusal {
        match package(args, &contracts()) {
            Ok(outcome) => panic!("expected a refusal, got {}", outcome.line()),
            Err(refusal) => refusal,
        }
    }

    fn copy_bundle(into: &Path) -> PathBuf {
        let contracts_root = into.join("contracts");
        let target = contracts_root.join("substrate-wire").join(VERSION);
        fs::create_dir_all(&target).expect("scratch bundle directory");
        copy_tree(&bundle(), &target);
        contracts_root
    }

    fn copy_tree(from: &Path, to: &Path) {
        for entry in fs::read_dir(from).expect("read source") {
            let entry = entry.expect("source entry");
            let target = to.join(entry.file_name());
            if entry.path().is_dir() {
                fs::create_dir_all(&target).expect("create directory");
                copy_tree(&entry.path(), &target);
            } else {
                fs::copy(entry.path(), &target).expect("copy file");
            }
        }
    }

    /// Every path under `root`, mapped to its bytes (`None` for a directory).
    fn tree_of(root: &Path) -> BTreeMap<String, Option<Vec<u8>>> {
        let mut tree = BTreeMap::new();
        let mut pending = vec![root.to_path_buf()];
        while let Some(directory) = pending.pop() {
            for entry in fs::read_dir(&directory).expect("read directory") {
                let path = entry.expect("entry").path();
                let relative = path
                    .strip_prefix(root)
                    .expect("relative")
                    .display()
                    .to_string();
                if path.is_dir() {
                    tree.insert(relative, None);
                    pending.push(path);
                } else {
                    tree.insert(relative, Some(fs::read(&path).expect("read file")));
                }
            }
        }
        tree
    }

    fn read_json(path: &Path) -> Value {
        serde_json::from_slice(&fs::read(path).expect("read json")).expect("parse json")
    }

    fn blob_path(layout: &Path, digest: &str) -> PathBuf {
        let (algorithm, hex) = digest.split_once(':').expect("algorithm:hex digest");
        layout.join("blobs").join(algorithm).join(hex)
    }

    fn manifest_digest(layout: &Path) -> String {
        let index = read_json(&layout.join("index.json"));
        let manifests = index["manifests"].as_array().expect("manifests").clone();
        assert_eq!(manifests.len(), 1, "{manifests:?}");
        manifests[0]["digest"].as_str().expect("digest").to_owned()
    }

    fn read_manifest(layout: &Path) -> Value {
        read_json(&blob_path(layout, &manifest_digest(layout)))
    }

    fn layers(manifest: &Value) -> Vec<Value> {
        manifest["layers"].as_array().expect("layers").clone()
    }

    fn archive_layers(manifest: &Value) -> Vec<Value> {
        layers(manifest)
            .into_iter()
            .filter(|layer| layer["mediaType"] == ARCHIVE_MEDIA_TYPE)
            .collect()
    }

    fn file_layers(manifest: &Value) -> Vec<Value> {
        layers(manifest)
            .into_iter()
            .filter(|layer| layer["mediaType"] != ARCHIVE_MEDIA_TYPE)
            .collect()
    }

    /// The single source-archive layer descriptor and its blob bytes.
    fn archive_of(layout: &Path) -> (Value, Vec<u8>) {
        let manifest = read_manifest(layout);
        let found = archive_layers(&manifest);
        assert_eq!(
            found.len(),
            1,
            "{} carries no single {ARCHIVE_MEDIA_TYPE} layer; packaging.json declares a \
             posix-tar source archive and the artifact must ship it",
            layout.display()
        );
        let digest = found[0]["digest"].as_str().expect("digest").to_owned();
        let blob = fs::read(blob_path(layout, &digest)).expect("archive blob");
        (found[0].clone(), blob)
    }

    fn blob_names(layout: &Path) -> BTreeSet<String> {
        fs::read_dir(layout.join("blobs").join("sha256"))
            .expect("blobs")
            .map(|entry| {
                entry
                    .expect("blob")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect()
    }

    /// The packager's one stdout line: a bare digest, then `key=value` fields.
    fn stdout_fields(line: &str) -> BTreeMap<String, String> {
        line.split_whitespace()
            .skip(1)
            .filter_map(|token| token.split_once('='))
            .map(|(key, value)| (key.to_owned(), value.to_owned()))
            .collect()
    }

    /// One ustar member, as read back out of the archive the packager wrote.
    #[derive(Debug)]
    struct Member {
        name: String,
        mode: u32,
        uid: u64,
        gid: u64,
        mtime: i64,
        typeflag: u8,
        uname: String,
        gname: String,
        data: Vec<u8>,
    }

    impl Member {
        fn is_directory(&self) -> bool {
            self.typeflag == b'5'
        }
    }

    fn field(block: &[u8], offset: usize, size: usize) -> String {
        let raw = &block[offset..offset + size];
        let end = raw.iter().position(|byte| *byte == 0).unwrap_or(raw.len());
        String::from_utf8_lossy(&raw[..end]).trim().to_owned()
    }

    fn octal(block: &[u8], offset: usize, size: usize) -> u64 {
        let text = field(block, offset, size);
        if text.is_empty() {
            return 0;
        }
        u64::from_str_radix(&text, 8).expect("octal field")
    }

    /// A ustar reader, so the tests read the bytes back rather than trusting the writer.
    fn read_tar(archive: &[u8]) -> Vec<Member> {
        let mut members = Vec::new();
        let mut offset = 0;
        while offset + 512 <= archive.len() {
            let block = &archive[offset..offset + 512];
            if block.iter().all(|byte| *byte == 0) {
                break;
            }
            assert_eq!(&block[257..265], b"ustar\x0000", "not a ustar header");
            let size = usize::try_from(octal(block, 124, 12)).expect("size");
            let member = Member {
                name: format!("{}{}", field(block, 345, 155), field(block, 0, 100)),
                mode: u32::try_from(octal(block, 100, 8)).expect("mode"),
                uid: octal(block, 108, 8),
                gid: octal(block, 116, 8),
                mtime: i64::try_from(octal(block, 136, 12)).expect("mtime"),
                typeflag: block[156],
                uname: field(block, 265, 32),
                gname: field(block, 297, 32),
                data: archive[offset + 512..offset + 512 + size].to_vec(),
            };
            offset += 512 + size.div_ceil(512) * 512;
            members.push(member);
        }
        members
    }

    // ---- determinism ------------------------------------------------------------------------

    #[test]
    fn two_runs_are_byte_identical() {
        let work = scratch("pkg-bundle-determinism-");
        let (first, second) = (work.path().join("first"), work.path().join("second"));
        let one = pack(&args(&first));
        let two = pack(&args(&second));

        assert_eq!(
            fs::read(first.join("index.json")).expect("index"),
            fs::read(second.join("index.json")).expect("index"),
            "index.json differs between two runs"
        );
        assert_eq!(
            fs::read(first.join("oci-layout")).expect("marker"),
            fs::read(second.join("oci-layout")).expect("marker")
        );
        assert_eq!(one.line(), two.line());
        assert!(one.line().contains(&manifest_digest(&first)));

        let names = blob_names(&first);
        assert_eq!(names, blob_names(&second), "blob sets differ");
        for name in &names {
            let left = blob_path(&first, &format!("sha256:{name}"));
            let right = blob_path(&second, &format!("sha256:{name}"));
            assert_eq!(
                fs::read(&left).expect("blob"),
                fs::read(&right).expect("blob"),
                "blob {name} differs between runs"
            );
        }
    }

    #[test]
    fn a_one_byte_change_changes_the_manifest_digest() {
        let work = scratch("pkg-bundle-onebyte-");
        let pristine_root = copy_bundle(&work.path().join("pristine"));
        let mutated_root = copy_bundle(&work.path().join("mutated"));

        let first = work.path().join("out-pristine-1");
        let again = work.path().join("out-pristine-2");
        pack(&scratch_args(&first, &pristine_root));
        pack(&scratch_args(&again, &pristine_root));
        let baseline = manifest_digest(&first);
        assert_eq!(
            manifest_digest(&again),
            baseline,
            "two runs over identical bytes must yield the same manifest digest; a digest that \
             moves on its own cannot attribute a change to the bundle"
        );

        let target = mutated_root
            .join("substrate-wire")
            .join(VERSION)
            .join("runner.json");
        let original = fs::read(&target).expect("runner.json");
        assert_eq!(original.last(), Some(&b'\n'));
        let mut mutated = original.clone();
        let last = mutated.len() - 2;
        mutated[last] ^= 0x20;
        assert_eq!(mutated.len(), original.len());
        assert_eq!(
            original
                .iter()
                .zip(&mutated)
                .filter(|(a, b)| a != b)
                .count(),
            1
        );
        fs::write(&target, &mutated).expect("mutate the copy");

        let mutated_out = work.path().join("out-mutated");
        pack(&scratch_args(&mutated_out, &mutated_root));
        assert_ne!(
            manifest_digest(&mutated_out),
            baseline,
            "a one-byte change in a bundle file must change the manifest digest"
        );
        assert_eq!(
            fs::read(bundle().join("runner.json")).expect("checked-in runner.json"),
            original,
            "the checked-in bundle must be untouched"
        );
    }

    // ---- artifact shape ---------------------------------------------------------------------

    #[test]
    fn the_layout_marker_and_index_name_the_manifest() {
        let work = scratch("pkg-bundle-shape-");
        let layout = work.path().join("layout");
        let line = pack(&args(&layout)).line();

        assert_eq!(
            read_json(&layout.join("oci-layout")),
            serde_json::json!({ "imageLayoutVersion": "1.0.0" })
        );
        let index = read_json(&layout.join("index.json"));
        assert_eq!(index["schemaVersion"], 2);
        let entry = &index["manifests"][0];
        assert_eq!(entry["mediaType"], MANIFEST_MEDIA_TYPE);
        assert_eq!(
            entry["annotations"]["org.opencontainers.image.version"],
            VERSION
        );
        assert_eq!(
            entry["annotations"]["dev.b10x.contract.status"],
            "development"
        );
        assert_eq!(
            entry["annotations"]["org.opencontainers.image.ref.name"],
            VERSION
        );
        let digest = entry["digest"].as_str().expect("digest");
        let blob = fs::read(blob_path(&layout, digest)).expect("manifest blob");
        assert_eq!(entry["size"], blob.len());
        assert_eq!(digest, format!("sha256:{}", sha256_hex(&blob)));
        assert_eq!(
            line.split_whitespace().next(),
            Some(digest),
            "the manifest digest must stay the first field on stdout"
        );
    }

    #[test]
    fn the_config_is_the_bundle_json_verbatim() {
        let work = scratch("pkg-bundle-config-");
        let layout = work.path().join("layout");
        pack(&args(&layout));
        let manifest = read_manifest(&layout);
        let config = &manifest["config"];
        assert_eq!(config["mediaType"], CONFIG_MEDIA_TYPE);
        let digest = config["digest"].as_str().expect("digest");
        let blob = fs::read(blob_path(&layout, digest)).expect("config blob");
        assert_eq!(
            blob,
            fs::read(bundle().join("bundle.json")).expect("bundle.json"),
            "the artifact's bundle.json must be the bundle's own bytes"
        );
        assert_eq!(config["size"], blob.len());
        assert_eq!(digest, format!("sha256:{}", sha256_hex(&blob)));
    }

    #[test]
    fn every_bundle_json_entry_matches_the_bytes() {
        let work = scratch("pkg-bundle-entries-");
        let layout = work.path().join("layout");
        pack(&args(&layout));
        let manifest = read_manifest(&layout);
        let digest = manifest["config"]["digest"].as_str().expect("digest");
        let bundle_json = read_json(&blob_path(&layout, digest));
        let entries = bundle_json["files"].as_array().expect("files");
        assert!(!entries.is_empty());
        for entry in entries {
            let sha256 = entry["sha256"].as_str().expect("sha256");
            let blob = blob_path(&layout, &format!("sha256:{sha256}"));
            assert!(blob.is_file(), "no blob for {}", entry["path"]);
            let data = fs::read(&blob).expect("blob");
            assert_eq!(entry["byte_length"], data.len());
            assert_eq!(sha256, sha256_hex(&data));
        }
    }

    #[test]
    fn layers_are_one_per_bundle_path_in_sorted_order() {
        let work = scratch("pkg-bundle-layers-");
        let layout = work.path().join("layout");
        pack(&args(&layout));
        let manifest = read_manifest(&layout);
        let digest = manifest["config"]["digest"].as_str().expect("digest");
        let bundle_json = read_json(&blob_path(&layout, digest));
        let listed: BTreeMap<String, Value> = bundle_json["files"]
            .as_array()
            .expect("files")
            .iter()
            .map(|entry| {
                (
                    entry["path"].as_str().expect("path").to_owned(),
                    entry.clone(),
                )
            })
            .collect();

        let files = file_layers(&manifest);
        assert_eq!(
            files,
            layers(&manifest)[..files.len()],
            "the per-file layers must stay first; the archive layer is appended"
        );
        let titles: Vec<String> = files
            .iter()
            .map(|layer| {
                layer["annotations"][TITLE_ANNOTATION]
                    .as_str()
                    .expect("title")
                    .to_owned()
            })
            .collect();
        let mut sorted = titles.clone();
        sorted.sort();
        assert_eq!(titles, sorted, "layer order is not path-sorted");
        assert_eq!(
            titles.iter().cloned().collect::<BTreeSet<String>>(),
            listed.keys().cloned().collect::<BTreeSet<String>>(),
            "layers do not cover bundle.json's files"
        );
        assert!(
            !titles.iter().any(|title| title == "bundle.json"),
            "bundle.json is the config, not a layer"
        );
        for (layer, title) in files.iter().zip(&titles) {
            let entry = &listed[title];
            assert_eq!(
                layer["digest"],
                format!("sha256:{}", entry["sha256"].as_str().expect("sha"))
            );
            assert_eq!(layer["size"], entry["byte_length"]);
            assert_eq!(layer["mediaType"], entry["media_type"]);
        }
    }

    #[test]
    fn the_blob_set_is_exactly_the_manifest_config_and_layers() {
        let work = scratch("pkg-bundle-blobs-");
        let layout = work.path().join("layout");
        pack(&args(&layout));
        let manifest = read_manifest(&layout);
        let present: BTreeSet<String> = blob_names(&layout)
            .into_iter()
            .map(|name| format!("sha256:{name}"))
            .collect();
        let mut expected: BTreeSet<String> = BTreeSet::new();
        expected.insert(manifest_digest(&layout));
        expected.insert(
            manifest["config"]["digest"]
                .as_str()
                .expect("digest")
                .to_owned(),
        );
        for layer in layers(&manifest) {
            expected.insert(layer["digest"].as_str().expect("digest").to_owned());
        }
        assert_eq!(present, expected, "the layout carries unreferenced blobs");
    }

    #[test]
    fn no_absolute_path_or_timestamp_leaks_into_the_metadata() {
        let work = scratch("pkg-bundle-leak-");
        let layout = work.path().join("layout");
        pack(&args(&layout));
        let root = repo::root().expect("workspace root").display().to_string();
        for name in ["index.json", "oci-layout"] {
            let text = fs::read_to_string(layout.join(name)).expect("read metadata");
            assert!(!text.contains(&root), "{name} names the repository root");
            assert!(
                !text.contains(&layout.display().to_string()),
                "{name} names the output directory"
            );
        }
        let manifest =
            fs::read_to_string(blob_path(&layout, &manifest_digest(&layout))).expect("manifest");
        assert!(
            !manifest.contains(&root),
            "the manifest names the repository root"
        );
        assert!(
            !manifest.contains("created"),
            "the manifest carries a timestamp"
        );
    }

    // ---- the declared source archive --------------------------------------------------------

    #[test]
    fn two_runs_produce_a_byte_identical_archive() {
        let work = scratch("pkg-bundle-archive-determinism-");
        let (first, second) = (work.path().join("first"), work.path().join("second"));
        pack(&args(&first));
        pack(&args(&second));
        let (_, one) = archive_of(&first);
        let (_, two) = archive_of(&second);
        assert_eq!(one, two, "the source archive is not reproducible");
        assert_eq!(
            manifest_digest(&first),
            manifest_digest(&second),
            "adding the archive must not make the manifest digest move on its own"
        );
    }

    #[test]
    fn the_archive_descriptor_and_stdout_agree_with_the_blob() {
        let work = scratch("pkg-bundle-archive-descriptor-");
        let layout = work.path().join("layout");
        let line = pack(&args(&layout)).line();
        let (layer, blob) = archive_of(&layout);
        let reported = stdout_fields(&line);

        assert_eq!(
            layer["annotations"][TITLE_ANNOTATION],
            format!("{VERSION}.tar")
        );
        assert_eq!(layer["size"], blob.len());
        assert_eq!(layer["digest"], format!("sha256:{}", sha256_hex(&blob)));
        assert_eq!(
            reported["archive"],
            layer["digest"].as_str().expect("digest")
        );
        assert_eq!(reported["archive_bytes"], blob.len().to_string());
        assert_eq!(
            layers(&read_manifest(&layout)).last(),
            Some(&layer),
            "the archive must be the last layer, so no per-file descriptor moves"
        );
    }

    #[test]
    fn the_archive_headers_match_packaging_json() {
        let work = scratch("pkg-bundle-archive-headers-");
        let layout = work.path().join("layout");
        let line = pack(&args(&layout)).line();
        let epoch: i64 = stdout_fields(&line)["source_date_epoch"]
            .parse()
            .expect("epoch");
        let (_, blob) = archive_of(&layout);

        let declared = read_json(&bundle().join("packaging.json"));
        let archive = &declared["archive"];
        assert_eq!(archive["format"], "posix-tar");
        assert_eq!(archive["compression"], "none");
        assert_eq!(archive["mode"], "files-0644-directories-0755");
        assert_eq!(archive["path_order"], "utf8-bytewise");
        assert_eq!(archive["source_date_epoch"], "source-commit-author-seconds");

        // compression "none": the bytes read as a plain tar, and the ustar magic sits in the
        // first header. pax or GNU records would show up as extra member types below.
        assert_eq!(&blob[257..265], b"ustar\x0000", "not a ustar header");
        let members = read_tar(&blob);
        assert!(!members.is_empty());
        for member in &members {
            assert!(
                member.typeflag == b'0' || member.typeflag == b'5',
                "only files and directories belong in the archive: {member:?}"
            );
            assert_eq!(member.uid, archive["uid"].as_u64().expect("uid"));
            assert_eq!(member.gid, archive["gid"].as_u64().expect("gid"));
            assert_eq!(member.uname, archive["owner_name"].as_str().expect("owner"));
            assert_eq!(member.gname, archive["group_name"].as_str().expect("group"));
            assert_eq!(member.mtime, epoch);
            assert_eq!(
                member.mode,
                if member.is_directory() { 0o755 } else { 0o644 }
            );
        }

        let names: Vec<String> = members
            .iter()
            .map(|member| member.name.trim_end_matches('/').to_owned())
            .collect();
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(
            names, sorted,
            "archive entries are not in UTF-8 bytewise path order"
        );
        assert_eq!(
            names.iter().cloned().collect::<BTreeSet<String>>(),
            tree_of(&bundle())
                .keys()
                .cloned()
                .collect::<BTreeSet<String>>(),
            "the archive is not the whole bundle directory"
        );
        assert!(
            names.iter().any(|name| name == "bundle.json"),
            "the archive is the source form of the directory, bundle.json included"
        );
        assert!(
            members.iter().any(Member::is_directory),
            "packaging.json declares a directory mode, so directories are entries"
        );
    }

    #[test]
    fn the_archive_extracts_to_the_bundle_byte_for_byte() {
        let work = scratch("pkg-bundle-archive-extract-");
        let layout = work.path().join("layout");
        pack(&args(&layout));
        let (_, blob) = archive_of(&layout);

        let extracted = work.path().join("extracted");
        fs::create_dir_all(&extracted).expect("extraction root");
        for member in read_tar(&blob) {
            let target = extracted.join(member.name.trim_end_matches('/'));
            if member.is_directory() {
                fs::create_dir_all(&target).expect("extract directory");
            } else {
                if let Some(parent) = target.parent() {
                    fs::create_dir_all(parent).expect("extract parent");
                }
                fs::write(&target, &member.data).expect("extract file");
            }
        }
        assert_eq!(
            tree_of(&extracted),
            tree_of(&bundle()),
            "extracting the archive does not reproduce the bundle byte for byte"
        );
    }

    #[test]
    fn source_date_epoch_moves_only_the_archive_and_the_manifest() {
        let work = scratch("pkg-bundle-archive-epoch-");
        let (early, late) = (work.path().join("early"), work.path().join("late"));
        pack(&Args {
            source_date_epoch: Some(1_000_000_000),
            ..args(&early)
        });
        pack(&Args {
            source_date_epoch: Some(2_000_000_000),
            ..args(&late)
        });

        let (early_layer, early_blob) = archive_of(&early);
        let (late_layer, late_blob) = archive_of(&late);
        assert_ne!(early_blob, late_blob);
        assert_ne!(
            early_layer["digest"], late_layer["digest"],
            "a different SOURCE_DATE_EPOCH must change the archive digest"
        );
        assert_ne!(
            manifest_digest(&early),
            manifest_digest(&late),
            "the manifest pins the archive, so its digest must move too"
        );
        assert_eq!(
            file_layers(&read_manifest(&early)),
            file_layers(&read_manifest(&late)),
            "no per-file layer may depend on SOURCE_DATE_EPOCH"
        );
        for (layout, seconds) in [(&early, 1_000_000_000), (&late, 2_000_000_000)] {
            let (_, blob) = archive_of(layout);
            let seen: BTreeSet<i64> = read_tar(&blob).iter().map(|member| member.mtime).collect();
            assert_eq!(seen, BTreeSet::from([seconds]));
        }
    }

    #[test]
    fn the_default_epoch_is_the_bundle_source_commit() {
        let expected = Command::new("git")
            .args(["log", "-1", "--format=%at", "--"])
            .arg(bundle())
            .current_dir(repo::root().expect("workspace root"))
            .output()
            .expect("run git");
        let expected = String::from_utf8_lossy(&expected.stdout).trim().to_owned();
        if expected.is_empty() {
            return; // No git history dates the bundle in this tree.
        }
        let work = scratch("pkg-bundle-archive-git-epoch-");
        let layout = work.path().join("layout");
        let line = pack(&args(&layout)).line();
        assert_eq!(
            stdout_fields(&line)["source_date_epoch"],
            expected,
            "SOURCE_DATE_EPOCH must be the source commit's author seconds \
             (packaging.json: source-commit-author-seconds)"
        );
    }

    #[test]
    fn a_tree_without_git_and_without_an_epoch_is_refused() {
        let work = scratch("pkg-bundle-archive-noepoch-");
        let contracts_root = copy_bundle(work.path());
        let out = work.path().join("layout");
        let refused = refuse(&Args {
            source_date_epoch: None,
            ..scratch_args(&out, &contracts_root)
        });
        assert!(
            refused.to_string().contains("--source-date-epoch"),
            "{refused}"
        );
        assert!(!out.exists(), "a refusal must write nothing");
        pack(&scratch_args(&out, &contracts_root));
    }

    // ---- refusals ---------------------------------------------------------------------------

    #[test]
    fn an_out_inside_contracts_is_refused() {
        let target = bundle().join("oci");
        let refused = refuse(&args(&target));
        assert!(refused.to_string().contains("contracts/"), "{refused}");
        assert!(
            !target.exists(),
            "the packager created a path under contracts/"
        );
    }

    #[test]
    fn the_contracts_root_itself_is_refused_as_out() {
        let refused = refuse(&args(&contracts()));
        assert!(refused.to_string().contains("contracts/"), "{refused}");
    }

    #[test]
    fn an_out_inside_a_scratch_contracts_root_is_refused() {
        let work = scratch("pkg-bundle-refusal-");
        let contracts_root = copy_bundle(work.path());
        let target = contracts_root.join("oci");
        let refused = refuse(&scratch_args(&target, &contracts_root));
        assert!(refused.to_string().contains("contracts"), "{refused}");
        assert!(!target.exists());
    }

    #[test]
    fn a_non_empty_out_requires_force() {
        let work = scratch("pkg-bundle-force-");
        let out = work.path().join("layout");
        let first = pack(&args(&out)).line();
        let refused = refuse(&args(&out));
        assert!(refused.to_string().contains("--force"), "{refused}");
        let again = pack(&Args {
            force: true,
            ..args(&out)
        })
        .line();
        assert_eq!(first, again);
    }

    #[test]
    fn force_refuses_a_directory_that_is_not_a_layout() {
        let work = scratch("pkg-bundle-foreign-");
        let out = work.path().join("not-a-layout");
        fs::create_dir_all(&out).expect("create out");
        let precious = out.join("precious.txt");
        fs::write(&precious, "do not delete me\n").expect("write precious");
        let refused = refuse(&Args {
            force: true,
            ..args(&out)
        });
        assert!(refused.to_string().contains("precious.txt"), "{refused}");
        assert!(
            precious.is_file(),
            "the packager deleted a file it does not own"
        );
    }

    #[test]
    fn an_unknown_version_is_refused() {
        let work = scratch("pkg-bundle-version-");
        let refused = refuse(&Args {
            version: "9.9.9".to_owned(),
            ..args(&work.path().join("layout"))
        });
        assert!(refused.to_string().contains("9.9.9"), "{refused}");
    }
}
