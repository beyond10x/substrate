//! The public site's derived tables, held to the sources they are derived from: the confinement
//! domain's enums and the declared sets its authored scenarios open adapters with, and the
//! command-line binding the client is generated from.
use crate::docs::{Rendered, read_public};
use anyhow::{Context, Result, ensure};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

const DOMAIN: &str = "spec/confinement/confinement.yaml";
const SUITE: &str = "spec/confinement-suite.json";
const BINDING: &str = "spec/generated/cli/binding.json";
const AUTHORED: &str = "substrate.confinement/authored/";
const ADAPTERS: &str = "concepts/adapters";
const CONTRACTS: &str = "reference/contracts";
const CLI: &str = "guides/cli";

/// One adapter column: an adapter, and for `systemd` the adapter inside it.
type Column = (String, Option<String>);

/// What every opened adapter declared, over all authored scenarios that open it.
#[derive(Debug, Default)]
struct Declared {
    /// Members some open declared.
    union: BTreeSet<String>,
    /// Members every open declared.
    intersection: Option<BTreeSet<String>>,
}

/// Every failure in the derived tables, one line each.
pub(crate) fn check(root: &Path, rendered: &BTreeMap<&str, Rendered>) -> Result<Vec<String>> {
    let domain = read_public(root, DOMAIN)?;
    let suite: Value = serde_json::from_str(&read_public(root, SUITE)?)?;
    let binding: Value = serde_json::from_str(&read_public(root, BINDING)?)?;
    let enums = Enums {
        families: variants(&domain, "substrate.confinement.ContractFamily")?,
        facts: variants(&domain, "substrate.confinement.Fact")?,
        adapters: variants(&domain, "substrate.confinement.AdapterName")?,
        inner: variants(&domain, "substrate.confinement.InnerAdapter")?,
    };
    let page = |name: &str| {
        rendered
            .get(name)
            .with_context(|| format!("{name} was not rendered"))
    };
    let mut failures = Vec::new();
    let (families, facts) = declared(&suite)?;
    matrix(
        page(ADAPTERS)?,
        "families-by-adapter",
        &enums.families,
        &enums.columns(),
        &families,
        &mut failures,
    );
    matrix(
        page(ADAPTERS)?,
        "facts-by-adapter",
        &enums.facts,
        &enums.columns(),
        &facts,
        &mut failures,
    );
    first_column(
        page(CONTRACTS)?,
        "the-ten-families",
        &enums.families,
        &mut failures,
    );
    first_column(
        page(CONTRACTS)?,
        "the-eleven-facts",
        &enums.facts,
        &mut failures,
    );
    lanes(page(CONTRACTS)?, &suite, &mut failures)?;
    cli(page(CLI)?, &binding, &mut failures)?;
    Ok(failures)
}

struct Enums {
    families: Vec<String>,
    facts: Vec<String>,
    adapters: Vec<String>,
    inner: Vec<String>,
}
impl Enums {
    /// The adapter columns in enum order, `systemd` once per adapter it may wrap.
    fn columns(&self) -> Vec<Column> {
        self.adapters
            .iter()
            .flat_map(|adapter| {
                if adapter == "systemd" {
                    self.inner
                        .iter()
                        .map(|inner| (adapter.clone(), Some(inner.clone())))
                        .collect()
                } else {
                    vec![(adapter.clone(), None)]
                }
            })
            .collect()
    }
}
fn header((adapter, inner): &Column) -> String {
    match inner {
        Some(inner) => format!("{adapter} ({inner})"),
        None => adapter.clone(),
    }
}

/// The variants of one enum in the domain source, in order. The source is ESS YAML; an enum reads
/// `- name: <type>`, `kind: enum`, then `variants: [a, b]` or one `- variant` per line.
fn variants(domain: &str, name: &str) -> Result<Vec<String>> {
    let mut lines = domain
        .lines()
        .skip_while(|line| line.trim() != format!("- name: {name}"))
        .skip(1)
        .take_while(|line| !line.trim_start().starts_with("- name:"));
    let mut found = Vec::new();
    while let Some(line) = lines.next() {
        let Some(rest) = line.trim().strip_prefix("variants:") else {
            continue;
        };
        let rest = rest.trim();
        if let Some(inline) = rest.strip_prefix('[').and_then(|r| r.strip_suffix(']')) {
            found.extend(inline.split(',').map(|v| v.trim().to_owned()));
        } else {
            ensure!(rest.is_empty(), "{name}: unexpected variants syntax");
            found.extend(
                lines
                    .by_ref()
                    .map_while(|line| line.trim().strip_prefix("- "))
                    .map(|v| v.trim().to_owned()),
            );
        }
        break;
    }
    ensure!(!found.is_empty(), "{DOMAIN} declares no variants of {name}");
    Ok(found)
}

/// The families and facts each adapter column declared in every authored scenario that opens it.
/// `model-*` scenarios exercise the admission model with invented sets, so they are not evidence
/// of what an adapter declares.
fn declared(suite: &Value) -> Result<(BTreeMap<Column, Declared>, BTreeMap<Column, Declared>)> {
    let mut families: BTreeMap<Column, Declared> = BTreeMap::new();
    let mut facts: BTreeMap<Column, Declared> = BTreeMap::new();
    let scenarios = suite["scenarios"]
        .as_object()
        .context("suite has no scenarios")?;
    for (key, scenario) in scenarios {
        let Some(name) = key.strip_prefix(AUTHORED) else {
            continue;
        };
        if name.starts_with("model-") {
            continue;
        }
        let steps = scenario["steps"]
            .as_array()
            .context("scenario has no steps")?;
        for pair in steps.windows(2) {
            let (open, outcome) = (&pair[0], &pair[1]);
            if open["command"] != "substrate.confinement.OpenAdapter"
                || outcome["outcome"]["outcome"] != "opened"
            {
                continue;
            }
            let input = &open["input"];
            let literal = |field: &str| &input[field]["value"];
            let adapter = literal("adapter").as_str().context("adapter")?.to_owned();
            let inner = (adapter == "systemd")
                .then(|| literal("inner").as_str().map(str::to_owned))
                .flatten();
            let column = (adapter, inner);
            for (into, field) in [(&mut families, "served_families"), (&mut facts, "facts")] {
                let set: BTreeSet<String> = literal(field)
                    .as_array()
                    .with_context(|| format!("{key}: {field}"))?
                    .iter()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect();
                let entry = into.entry(column.clone()).or_default();
                entry.union.extend(set.iter().cloned());
                entry.intersection = Some(match entry.intersection.take() {
                    Some(all) => all.intersection(&set).cloned().collect(),
                    None => set,
                });
            }
        }
    }
    Ok((families, facts))
}

fn table<'a>(page: &'a Rendered, heading: &str) -> Option<&'a Vec<Vec<String>>> {
    page.tables
        .iter()
        .find(|(id, _)| id == heading)
        .map(|(_, rows)| rows)
}

/// A `yes` cell is declared by every open of that column, `yes` and `probed` together are exactly
/// what some open declared, and `—` is never declared.
fn matrix(
    page: &Rendered,
    heading: &str,
    rows: &[String],
    columns: &[Column],
    declared: &BTreeMap<Column, Declared>,
    failures: &mut Vec<String>,
) {
    let Some(table) = table(page, heading) else {
        failures.push(format!("{ADAPTERS}: no table under #{heading}"));
        return;
    };
    let headers: Vec<String> = columns.iter().map(header).collect();
    if table[0].get(1..) != Some(headers.as_slice()) {
        failures.push(format!(
            "{ADAPTERS}#{heading}: columns {:?}, the domain's adapters are {headers:?}",
            table[0].get(1..).unwrap_or_default()
        ));
        return;
    }
    let names: Vec<&String> = table[1..].iter().filter_map(|row| row.first()).collect();
    if names != rows.iter().collect::<Vec<_>>() {
        failures.push(format!(
            "{ADAPTERS}#{heading}: rows {names:?}, the domain's order is {rows:?}"
        ));
        return;
    }
    for row in &table[1..] {
        for (column, cell) in columns.iter().zip(&row[1..]) {
            let member = &row[0];
            let Some(set) = declared.get(column) else {
                failures.push(format!(
                    "{ADAPTERS}#{heading}: no authored scenario opens {}",
                    header(column)
                ));
                continue;
            };
            let always = set
                .intersection
                .as_ref()
                .is_some_and(|s| s.contains(member));
            let ever = set.union.contains(member);
            let holds = match cell.as_str() {
                "yes" => always,
                "probed" => ever,
                "—" => !ever,
                _ => false,
            };
            if !holds {
                let truth = if always {
                    "every open declares it"
                } else if ever {
                    "some opens declare it"
                } else {
                    "no open declares it"
                };
                failures.push(format!(
                    "{ADAPTERS}#{heading}: {member} under {} says {cell:?}, but {truth}",
                    header(column)
                ));
            }
        }
    }
}

/// The first column of a definition table lists the enum's variants in its order.
fn first_column(page: &Rendered, heading: &str, rows: &[String], failures: &mut Vec<String>) {
    let Some(table) = table(page, heading) else {
        failures.push(format!("{CONTRACTS}: no table under #{heading}"));
        return;
    };
    let names: Vec<&String> = table[1..].iter().filter_map(|row| row.first()).collect();
    if names != rows.iter().collect::<Vec<_>>() {
        failures.push(format!(
            "{CONTRACTS}#{heading}: rows {names:?}, the domain's order is {rows:?}"
        ));
    }
}

/// The lanes table counts the authored scenarios by the lane their name ends in, and the sentence
/// above it states the total.
fn lanes(page: &Rendered, suite: &Value, failures: &mut Vec<String>) -> Result<()> {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for key in suite["scenarios"]
        .as_object()
        .context("suite has no scenarios")?
        .keys()
    {
        if let Some(name) = key.strip_prefix(AUTHORED) {
            let lane = name
                .rsplit_once("-on-")
                .with_context(|| format!("authored scenario {name} names no lane"))?
                .1;
            *counts.entry(lane.to_owned()).or_default() += 1;
        }
    }
    let total: usize = counts.values().sum();
    if !page
        .html
        .contains(&format!("holds {total} authored scenarios"))
    {
        failures.push(format!(
            "{CONTRACTS}#scenarios-and-lanes: the specification holds {total} authored scenarios"
        ));
    }
    let Some(table) = table(page, "scenarios-and-lanes") else {
        failures.push(format!("{CONTRACTS}: no table under #scenarios-and-lanes"));
        return Ok(());
    };
    let shown: BTreeMap<String, usize> = table[1..]
        .iter()
        .filter_map(|row| Some((row.first()?.clone(), row.get(1)?.parse().ok()?)))
        .collect();
    if shown != counts {
        failures.push(format!(
            "{CONTRACTS}#scenarios-and-lanes: shows {shown:?}, the suite has {counts:?}"
        ));
    }
    Ok(())
}

/// The Commands table names every command of the binding and no other, and the page names every
/// option the binding declares.
fn cli(page: &Rendered, binding: &Value, failures: &mut Vec<String>) -> Result<()> {
    let mut paths = BTreeSet::new();
    let mut options: BTreeSet<String> = binding["globals"]
        .as_object()
        .context("binding has no globals")?
        .values()
        .filter_map(|v| v.as_str().map(str::to_owned))
        .collect();
    for command in binding["commands"]
        .as_array()
        .context("binding has no commands")?
    {
        let path: Vec<&str> = command["path"]
            .as_array()
            .context("command path")?
            .iter()
            .filter_map(Value::as_str)
            .collect();
        paths.insert(path.join(" "));
        for argument in command["arguments"].as_array().into_iter().flatten() {
            if let Some(long) = argument["source"]["long"].as_str() {
                options.insert(long.to_owned());
            }
        }
    }
    let Some(table) = table(page, "commands") else {
        failures.push(format!("{CLI}: no table under #commands"));
        return Ok(());
    };
    let shown: BTreeSet<String> = table[1..]
        .iter()
        .filter_map(|row| row.first())
        .map(|cell| {
            cell.split_whitespace()
                .take_while(|word| {
                    word.starts_with(|c: char| c.is_ascii_lowercase())
                        && word.chars().all(|c| c.is_ascii_lowercase() || c == '-')
                })
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect();
    if shown != paths {
        failures.push(format!(
            "{CLI}#commands: lists {shown:?}, the binding has {paths:?}"
        ));
    }
    for option in options {
        if !page.html.contains(&format!("--{option}")) {
            failures.push(format!("{CLI}: never names --{option}"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::docs::rendered;

    #[test]
    fn the_published_tables_match_the_specification() {
        let root = crate::repo::root().unwrap();
        let failures = check(&root, &rendered(&root).unwrap()).unwrap();
        assert!(failures.is_empty(), "{failures:#?}");
    }
    #[test]
    fn enum_variants_are_read_inline_and_as_a_list() {
        let domain = "types:\n  - name: a.B\n    kind: enum\n    variants: [x, y]\n  - name: a.C\n    kind: enum\n    variants:\n      - p\n      - q\n  - name: a.D\n";
        assert_eq!(variants(domain, "a.B").unwrap(), ["x", "y"]);
        assert_eq!(variants(domain, "a.C").unwrap(), ["p", "q"]);
        assert!(variants(domain, "a.D").is_err());
    }
    #[test]
    fn a_cell_that_overclaims_or_omits_a_declaration_fails() {
        let columns: Vec<Column> = vec![("linux".into(), None)];
        let mut declared = BTreeMap::new();
        declared.insert(
            columns[0].clone(),
            Declared {
                union: ["a".to_owned(), "b".to_owned()].into(),
                intersection: Some(["a".to_owned()].into()),
            },
        );
        let rows = vec!["a".to_owned(), "b".to_owned(), "c".to_owned()];
        let page = |cells: [&str; 3]| Rendered {
            tables: vec![(
                "m".into(),
                std::iter::once(vec!["F".to_owned(), "linux".to_owned()])
                    .chain(
                        rows.iter()
                            .zip(cells)
                            .map(|(r, c)| vec![r.clone(), c.to_owned()]),
                    )
                    .collect(),
            )],
            ..Rendered::default()
        };
        let run = |cells| {
            let mut failures = Vec::new();
            matrix(&page(cells), "m", &rows, &columns, &declared, &mut failures);
            failures
        };
        assert!(run(["yes", "probed", "—"]).is_empty());
        assert!(run(["probed", "probed", "—"]).is_empty());
        assert_eq!(
            run(["yes", "yes", "—"]).len(),
            1,
            "b is not always declared"
        );
        assert_eq!(run(["yes", "—", "—"]).len(), 1, "b is sometimes declared");
        assert_eq!(
            run(["yes", "probed", "probed"]).len(),
            1,
            "c is never declared"
        );
    }
}
