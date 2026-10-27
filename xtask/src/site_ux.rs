//! The public site's per-merge UX gate: static, hermetic measurements held to lower-is-better
//! ratchets in `website/ux/baseline.json`, and the finding records' rules.
//!
//! The browser measurements of a round (layout at each viewport, axe-core, keyboard, the
//! screen-only bot at each viewport) run outside the repository; their results land in the round
//! files under `website/ux/rounds/`. This gate measures two things. The reading load of each page
//! in words per text block needs only the Markdown, so `cargo xtask check-docs` holds it on every
//! merge. How many link clicks each task in `website/ux/tasks.json` takes from the start page needs
//! the built HTML, so `cargo xtask docs-provenance` holds it after the docs build.
use crate::docs::{BASE, Rendered, attribute_list, attribute_values, read_public};
use anyhow::{Context, Result, ensure};
use clap::Args;
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

pub(crate) const BASELINE: &str = "website/ux/baseline.json";
const TASKS: &str = "website/ux/tasks.json";
const FINDINGS: &str = "website/ux/findings";
const START: &str = BASE;
const CLICKS: &str = "screen_only.actions";
const BLOCK_MAX: &str = "reading_load.max";
const BLOCK_MEDIAN: &str = "reading_load.median";
/// The scenarios measured from the Markdown, and those measured from the built site.
const PAGE_SCOPE: &str = "page:";
const TASK_SCOPE: &str = "task:";
const CLASSES: [&str; 7] = [
    "layout",
    "cue",
    "affordance",
    "copy",
    "noise",
    "engine",
    "input",
];
const SEVERITIES: [&str; 3] = ["S1", "S2", "S3"];
const STATUSES: [&str; 3] = ["open", "fixed", "known-loss"];
const FIELDS: [&str; 12] = [
    "id",
    "title",
    "class",
    "severity",
    "metric",
    "status",
    "found_by",
    "evidence_recording",
    "evidence_screenshot",
    "evidence_lines",
    "regression_asset",
    "round",
];

/// Every measured value: scenario, then metric key, then value. Lower is better for all of them.
type Measured = BTreeMap<String, BTreeMap<&'static str, f64>>;

#[derive(Debug, Args)]
pub struct BaselineArgs {
    /// The decision that admits a value worse than the current baseline.
    #[arg(long)]
    decision: Option<String>,
    /// A built site (`website/build`) to measure the task clicks from; without it the committed
    /// task values are kept.
    #[arg(long)]
    site: Option<PathBuf>,
}

/// Each page's reading load, from its Markdown.
fn measure_pages(rendered: &BTreeMap<&str, Rendered>) -> Measured {
    let mut measured = Measured::new();
    for (page, page_rendered) in rendered {
        let mut blocks = page_rendered.blocks.clone();
        blocks.sort_unstable();
        let max = count(blocks.last().copied().unwrap_or_default());
        let median = match blocks.len() {
            0 => 0.0,
            n if n % 2 == 1 => count(blocks[n / 2]),
            n => count(blocks[n / 2 - 1] + blocks[n / 2]) / 2.0,
        };
        measured.insert(
            format!("{PAGE_SCOPE}{page}"),
            [(BLOCK_MAX, max), (BLOCK_MEDIAN, median)].into(),
        );
    }
    measured
}

/// Each task's click count from the start page of the built site, whose pages are keyed by
/// their `/x/` route.
fn measure_tasks(root: &Path, pages: &BTreeMap<String, String>) -> Result<Measured> {
    let tasks: Value = serde_json::from_str(&read_public(root, TASKS)?)?;
    let mut measured = Measured::new();
    for task in tasks.as_array().context("tasks.json is a list")? {
        let id = task["id"].as_str().context("a task has an id")?;
        let target = task["route"].as_str().context("a task has a route")?;
        let anchor = task["anchor"].as_str().unwrap_or_default();
        let page = pages
            .get(target)
            .with_context(|| format!("task {id}: no published route {target}"))?;
        ensure!(
            anchor.is_empty() || attribute_values(page, "id").contains(anchor),
            "task {id}: {target} has no #{anchor}"
        );
        let clicks = clicks(pages, target)
            .with_context(|| format!("task {id}: {target} cannot be reached from {START}"))?;
        measured.insert(
            format!("{TASK_SCOPE}{id}"),
            [(CLICKS, count(clicks))].into(),
        );
    }
    Ok(measured)
}

/// A count as a ratchet value; no page holds 2^32 words.
fn count(n: usize) -> f64 {
    f64::from(u32::try_from(n).unwrap_or(u32::MAX))
}

/// The route a link on the site reaches, in the `/x/` form pages are keyed by: `/substrate/docs/x`,
/// `/substrate/docs/x/` and `/substrate/docs/x#a` all reach `/substrate/docs/x/`.
fn reached(href: &str) -> Option<String> {
    let path = href.split(['#', '?']).next().unwrap_or_default();
    let path = path.strip_prefix("pathname://").unwrap_or(path);
    if !path.starts_with(BASE) {
        return None;
    }
    Some(if path.ends_with('/') {
        path.to_owned()
    } else {
        format!("{path}/")
    })
}

/// The fewest link clicks from the start page to `target`, following every link a page shows.
fn clicks(pages: &BTreeMap<String, String>, target: &str) -> Option<usize> {
    let mut seen = BTreeSet::from([START.to_owned()]);
    let mut queue = VecDeque::from([(START.to_owned(), 0)]);
    while let Some((at, depth)) = queue.pop_front() {
        if at == target {
            return Some(depth);
        }
        for href in attribute_list(&pages[&at], "href") {
            let Some(path) = reached(&href) else { continue };
            if pages.contains_key(&path) && seen.insert(path.clone()) {
                queue.push_back((path, depth + 1));
            }
        }
    }
    None
}

fn entry<'a>(baseline: &'a Value, scenario: &str, metric: &str) -> Option<&'a Value> {
    baseline["scenarios"][scenario].get(metric)
}
fn tolerance(value: &Value) -> f64 {
    value["tolerance"].as_f64().unwrap_or_default()
}

/// The ratchet over the baseline scenarios whose names start with `scope`: a measured value may
/// be neither worse nor better than its baseline, every value has a baseline and every baseline a
/// value, and a baseline worse than `main`'s names a decision.
fn ratchet(
    measured: &Measured,
    baseline: &Value,
    main: Option<&Value>,
    scope: &str,
) -> Vec<String> {
    let mut failures = Vec::new();
    for (scenario, metrics) in measured {
        for (metric, value) in metrics {
            let Some(base) = entry(baseline, scenario, metric) else {
                failures.push(format!("{BASELINE}: no {scenario} {metric} (measured {value}); run `cargo xtask docs-baseline`"));
                continue;
            };
            let Some(limit) = base["value"].as_f64() else {
                failures.push(format!("{BASELINE}: {scenario} {metric} has no value"));
                continue;
            };
            if *value > limit + tolerance(base) {
                failures.push(format!(
                    "{scenario} {metric} is {value}, worse than the baseline {limit}"
                ));
            } else if *value < limit - tolerance(base) {
                failures.push(format!(
                    "{scenario} {metric} improved to {value} from {limit}; take it into the baseline with `cargo xtask docs-baseline`"
                ));
            }
        }
    }
    let scoped = baseline["scenarios"]
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(scenario, _)| scenario.starts_with(scope));
    for (scenario, metrics) in scoped {
        for (metric, base) in metrics.as_object().into_iter().flatten() {
            if !measured
                .get(scenario)
                .is_some_and(|m| m.contains_key(metric.as_str()))
            {
                failures.push(format!(
                    "{BASELINE}: {scenario} {metric} is no longer measured"
                ));
            }
            let Some(before) = main.and_then(|main| entry(main, scenario, metric)) else {
                continue;
            };
            let (now, then) = (base["value"].as_f64(), before["value"].as_f64());
            if let (Some(now), Some(then)) = (now, then)
                && now > then + tolerance(before)
                && base["decision"].as_str().is_none_or(str::is_empty)
            {
                failures.push(format!(
                    "{BASELINE}: {scenario} {metric} moved from {then} on main to {now} without a decision"
                ));
            }
        }
    }
    failures
}

/// The baseline on `origin/main`, when the checkout has it.
fn main_baseline(root: &Path) -> Option<Value> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["show", &format!("origin/main:{BASELINE}")])
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| serde_json::from_slice(&output.stdout).ok())
        .flatten()
}

/// The finding records: every header field present, known values, and no closed S1 or S2 finding
/// without a regression asset; an asset written `xtask/src/<file>.rs::<test>` must name a test that
/// exists.
fn findings(root: &Path) -> Result<Vec<String>> {
    let mut failures = Vec::new();
    let dir = root.join(FINDINGS);
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .with_context(|| format!("{FINDINGS} is missing"))?
        .map(|entry| Ok(entry?.file_name().to_string_lossy().into_owned()))
        .collect::<Result<_>>()?;
    names.sort();
    for name in names {
        let Some(id) = name.strip_suffix(".md").filter(|id| {
            id.strip_prefix("UXF-")
                .is_some_and(|n| n.len() == 3 && n.bytes().all(|b| b.is_ascii_digit()))
        }) else {
            failures.push(format!("{FINDINGS}/{name}: not a UXF-nnn.md record"));
            continue;
        };
        let text = read_public(root, &format!("{FINDINGS}/{name}"))?;
        failures.extend(record(id, &text, |asset| asset_exists(root, asset)));
    }
    Ok(failures)
}
fn record(id: &str, text: &str, exists: impl Fn(&str) -> bool) -> Vec<String> {
    let header: BTreeMap<&str, &str> = text
        .lines()
        .take_while(|line| !line.trim().is_empty())
        .filter_map(|line| line.split_once(": "))
        .map(|(key, value)| (key.trim(), value.trim()))
        .collect();
    let mut failures = Vec::new();
    let mut fail = |why: String| failures.push(format!("{FINDINGS}/{id}.md: {why}"));
    for field in FIELDS {
        if header.get(field).is_none_or(|v| v.is_empty()) {
            fail(format!("no {field}"));
        }
    }
    let field = |name: &str| header.get(name).copied().unwrap_or_default();
    if field("id") != id {
        fail(format!("id {} is not the file name", field("id")));
    }
    for (name, allowed) in [
        ("class", &CLASSES[..]),
        ("severity", &SEVERITIES[..]),
        ("status", &STATUSES[..]),
    ] {
        if !allowed.contains(&field(name)) {
            fail(format!("{name} {} is not one of {allowed:?}", field(name)));
        }
    }
    let asset = field("regression_asset");
    if field("status") == "fixed" && matches!(field("severity"), "S1" | "S2") {
        if asset.is_empty() || asset == "none" {
            fail("a fixed S1 or S2 finding needs a regression asset".into());
        } else if asset.contains("::") && !exists(asset) {
            fail(format!("regression asset {asset} does not exist"));
        }
    }
    failures
}
fn asset_exists(root: &Path, asset: &str) -> bool {
    let Some((file, test)) = asset.split_once("::") else {
        return false;
    };
    file.starts_with("xtask/src/")
        && Path::new(file).extension().is_some_and(|e| e == "rs")
        && read_public(root, file).is_ok_and(|source| source.contains(&format!("fn {test}(")))
}

fn committed_baseline(root: &Path) -> Result<Value> {
    Ok(serde_json::from_str(&read_public(root, BASELINE)?)?)
}

/// The Markdown half of the UX gate: the reading-load ratchet against the committed baseline and
/// `main`'s, and the findings.
pub(crate) fn check(root: &Path, rendered: &BTreeMap<&str, Rendered>) -> Result<Vec<String>> {
    let measured = measure_pages(rendered);
    let mut failures = ratchet(
        &measured,
        &committed_baseline(root)?,
        main_baseline(root).as_ref(),
        PAGE_SCOPE,
    );
    failures.extend(findings(root)?);
    Ok(failures)
}

/// The built-site half of the UX gate: the click ratchet of every task, on the pages the docs
/// build wrote.
pub(crate) fn check_site(root: &Path, pages: &BTreeMap<String, String>) -> Result<Vec<String>> {
    let measured = measure_tasks(root, pages)?;
    Ok(ratchet(
        &measured,
        &committed_baseline(root)?,
        main_baseline(root).as_ref(),
        TASK_SCOPE,
    ))
}

/// Write the measured values as the new baseline. A value worse than the baseline it replaces is
/// written only with `--decision`, which it then carries. Without `--site`, the task values are
/// carried over unchanged.
pub fn write_baseline(args: &BaselineArgs) -> Result<ExitCode> {
    let root = crate::repo::root()?;
    let before: Value = read_public(&root, BASELINE)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or(Value::Null);
    let mut measured = measure_pages(&crate::docs::rendered(&root)?);
    if let Some(site) = &args.site {
        measured.extend(measure_tasks(&root, &crate::docs::built_pages(site)?)?);
    } else {
        for (scenario, metrics) in before["scenarios"].as_object().into_iter().flatten() {
            if !scenario.starts_with(TASK_SCOPE) {
                continue;
            }
            let values = metrics
                .get(CLICKS)
                .and_then(|base| base["value"].as_f64())
                .map(|value| (CLICKS, value));
            measured.insert(scenario.clone(), values.into_iter().collect());
        }
    }
    let mut scenarios = Map::new();
    let mut worse = Vec::new();
    for (scenario, metrics) in &measured {
        let mut entries = Map::new();
        for (metric, value) in metrics {
            let mut written = json!({"value": value, "tolerance": 0});
            if let Some(base) = entry(&before, scenario, metric) {
                let limit = base["value"].as_f64().unwrap_or(f64::INFINITY);
                let decision = if *value > limit + tolerance(base) {
                    worse.push(format!("{scenario} {metric}: {limit} -> {value}"));
                    args.decision.clone()
                } else {
                    base["decision"].as_str().map(str::to_owned)
                };
                if let Some(decision) = decision {
                    written["decision"] = decision.into();
                }
            }
            entries.insert((*metric).to_owned(), written);
        }
        scenarios.insert(scenario.clone(), entries.into());
    }
    ensure!(
        worse.is_empty() || args.decision.is_some(),
        "these values are worse than the baseline; pass --decision <id> to admit them:\n{}",
        worse.join("\n")
    );
    let baseline = json!({
        "about": "Lower-is-better ratchets of the public site, written by `cargo xtask docs-baseline`. task:<id> counts the link clicks from the start page to the task's page on the built site, held by `cargo xtask docs-provenance`; page:<page> counts the words of the page's largest and median text block, held by `cargo xtask check-docs`.",
        "scenarios": scenarios,
        "tolerances": {"counts": 0},
    });
    let mut text = serde_json::to_string_pretty(&baseline)?;
    text.push('\n');
    std::fs::write(root.join(BASELINE), text)?;
    println!(
        "{BASELINE}: {} values written{}",
        measured.values().map(BTreeMap::len).sum::<usize>(),
        if worse.is_empty() {
            String::new()
        } else {
            format!(
                ", {} admitted worse under {}",
                worse.len(),
                args.decision.as_deref().unwrap_or_default()
            )
        }
    );
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn values(pairs: &[(&str, &'static str, f64)]) -> Measured {
        let mut measured = Measured::new();
        for (scenario, metric, value) in pairs {
            measured
                .entry((*scenario).to_owned())
                .or_default()
                .insert(*metric, *value);
        }
        measured
    }
    fn baseline(pairs: &[(&str, &str, f64, Option<&str>)]) -> Value {
        let mut scenarios = json!({});
        for (scenario, metric, value, decision) in pairs {
            let mut written = json!({"value": value, "tolerance": 0});
            if let Some(decision) = decision {
                written["decision"] = (*decision).into();
            }
            scenarios[*scenario][*metric] = written;
        }
        json!({"scenarios": scenarios})
    }

    #[test]
    fn the_committed_baseline_holds_the_rendered_site() {
        let root = crate::repo::root().unwrap();
        let measured = measure_pages(&crate::docs::rendered(&root).unwrap());
        let committed: Value =
            serde_json::from_str(&read_public(&root, BASELINE).unwrap()).unwrap();
        let failures = ratchet(&measured, &committed, None, PAGE_SCOPE);
        assert!(failures.is_empty(), "{failures:#?}");
    }
    #[test]
    fn the_ratchet_refuses_worse_untaken_better_missing_and_stale_values() {
        let base = baseline(&[("task:a", CLICKS, 2.0, None), ("task:b", CLICKS, 2.0, None)]);
        assert!(
            ratchet(
                &values(&[("task:a", CLICKS, 2.0), ("task:b", CLICKS, 2.0)]),
                &base,
                None,
                TASK_SCOPE
            )
            .is_empty()
        );
        let worse = ratchet(
            &values(&[("task:a", CLICKS, 3.0), ("task:b", CLICKS, 2.0)]),
            &base,
            None,
            TASK_SCOPE,
        );
        assert!(worse[0].contains("worse"), "{worse:?}");
        let better = ratchet(
            &values(&[("task:a", CLICKS, 1.0), ("task:b", CLICKS, 2.0)]),
            &base,
            None,
            TASK_SCOPE,
        );
        assert!(better[0].contains("improved"), "{better:?}");
        let missing = ratchet(
            &values(&[
                ("task:a", CLICKS, 2.0),
                ("task:b", CLICKS, 2.0),
                ("task:c", CLICKS, 1.0),
            ]),
            &base,
            None,
            TASK_SCOPE,
        );
        assert!(missing[0].contains("no task:c"), "{missing:?}");
        let stale = ratchet(&values(&[("task:a", CLICKS, 2.0)]), &base, None, TASK_SCOPE);
        assert!(stale[0].contains("no longer measured"), "{stale:?}");
    }
    #[test]
    fn a_baseline_worse_than_main_needs_a_decision() {
        let main = baseline(&[("task:a", CLICKS, 1.0, None)]);
        let now = values(&[("task:a", CLICKS, 2.0)]);
        let loosened = ratchet(
            &now,
            &baseline(&[("task:a", CLICKS, 2.0, None)]),
            Some(&main),
            TASK_SCOPE,
        );
        assert!(loosened[0].contains("without a decision"), "{loosened:?}");
        assert!(
            ratchet(
                &now,
                &baseline(&[("task:a", CLICKS, 2.0, Some("D1"))]),
                Some(&main),
                TASK_SCOPE
            )
            .is_empty()
        );
    }
    #[test]
    fn a_task_counts_the_fewest_clicks_and_an_orphan_is_unreachable() {
        let pages: BTreeMap<String, String> = [
            (
                START,
                "<a href=\"/substrate/docs/\">d</a><a href=\"/substrate/docs/b/#x\">b</a>",
            ),
            ("/substrate/docs/", "<a href=\"/substrate/docs/c/\">c</a>"),
            (
                "/substrate/docs/b/",
                "<p id=\"x\"></p><a href=\"/substrate/docs/c/\">c</a>",
            ),
            ("/substrate/docs/c/", ""),
            ("/substrate/docs/orphan/", ""),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect();
        assert_eq!(clicks(&pages, "/substrate/docs/b/"), Some(1));
        assert_eq!(clicks(&pages, "/substrate/docs/c/"), Some(2));
        assert_eq!(clicks(&pages, "/substrate/docs/orphan/"), None);
    }
    #[test]
    fn built_links_reach_their_route_in_every_written_form() {
        assert_eq!(
            reached("/substrate/docs/x").as_deref(),
            Some("/substrate/docs/x/")
        );
        assert_eq!(
            reached("/substrate/docs/x/").as_deref(),
            Some("/substrate/docs/x/")
        );
        assert_eq!(
            reached("/substrate/docs/x#a").as_deref(),
            Some("/substrate/docs/x/")
        );
        assert_eq!(
            reached("pathname:///substrate/").as_deref(),
            Some("/substrate/")
        );
        assert_eq!(reached("https://example.com/substrate/"), None);
        let pages: BTreeMap<String, String> = [
            (START, "<a class=b href=/substrate/docs>d</a>"),
            ("/substrate/docs/", "<a href=/substrate/docs/c#x>c</a>"),
            ("/substrate/docs/c/", ""),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect();
        assert_eq!(clicks(&pages, "/substrate/docs/c/"), Some(2));
    }
    #[test]
    fn each_half_of_the_gate_holds_only_its_own_scenarios() {
        let base = baseline(&[
            ("task:a", CLICKS, 2.0, None),
            ("page:p", BLOCK_MAX, 9.0, None),
        ]);
        let pages = values(&[("page:p", BLOCK_MAX, 9.0)]);
        assert!(ratchet(&pages, &base, None, PAGE_SCOPE).is_empty());
        let tasks = values(&[("task:a", CLICKS, 2.0)]);
        assert!(ratchet(&tasks, &base, None, TASK_SCOPE).is_empty());
        assert!(!ratchet(&tasks, &base, None, PAGE_SCOPE).is_empty());
    }
    #[test]
    fn a_closed_serious_finding_without_a_live_regression_asset_fails() {
        let text = |status: &str, asset: &str| {
            format!(
                "id: UXF-001\ntitle: t\nclass: layout\nseverity: S2\nmetric: m\nstatus: {status}\nfound_by: probe\nevidence_recording: none\nevidence_screenshot: s\nevidence_lines: l\nregression_asset: {asset}\nround: R-01\n\nBody.\n"
            )
        };
        let exists = |asset: &str| asset == "xtask/src/docs.rs::real";
        assert!(record("UXF-001", &text("fixed", "xtask/src/docs.rs::real"), exists).is_empty());
        assert!(record("UXF-001", &text("open", "none"), exists).is_empty());
        assert_eq!(record("UXF-001", &text("fixed", "none"), exists).len(), 1);
        assert_eq!(
            record("UXF-001", &text("fixed", "xtask/src/docs.rs::gone"), exists).len(),
            1
        );
        assert_eq!(
            record("UXF-002", &text("fixed", "xtask/src/docs.rs::real"), exists).len(),
            1
        );
        assert!(record("UXF-001", "id: UXF-001\n", exists).len() > 5);
    }
    #[test]
    fn the_committed_findings_are_well_formed() {
        let failures = findings(&crate::repo::root().unwrap()).unwrap();
        assert!(failures.is_empty(), "{failures:#?}");
    }
}
