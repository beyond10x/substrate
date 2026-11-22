//! Print the slowest tests of a cargo-nextest run, read from the `JUnit` report it wrote.
//!
//! The gate's test step has a budget (AGENTS.md, section *The gate*): the step ends within two
//! minutes and no test runs longer than 60 s. cargo-nextest enforces the second half and prints no
//! ranking, so this verb reads the report of the run that just ended and names the tests that
//! took longest, whether the run passed or failed.

use std::fmt::Write as _;
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context as _, Result, anyhow};

/// `cargo xtask slowest-tests --junit <file>`.
#[derive(Debug, clap::Args)]
pub struct Args {
    /// The `JUnit` report cargo-nextest wrote, `target/nextest/<profile>/junit.xml`.
    #[arg(long, value_name = "FILE")]
    junit: PathBuf,
    /// How many tests to print.
    #[arg(long, value_name = "N", default_value_t = 10)]
    count: usize,
}

pub fn run(args: &Args) -> Result<ExitCode> {
    let xml = std::fs::read_to_string(&args.junit).with_context(|| {
        format!(
            "no JUnit report at {}: the test run wrote none",
            args.junit.display()
        )
    })?;
    let cases = parse(&xml).map_err(|error| anyhow!("{}: {error}", args.junit.display()))?;
    print!("{}", render(&cases, args.count));
    Ok(ExitCode::SUCCESS)
}

/// One `<testcase>` of the report.
#[derive(Debug, Clone, PartialEq)]
struct TestCase {
    /// The test binary, as nextest names it: `<package>::<target>`.
    binary: String,
    name: String,
    seconds: f64,
    /// Whether the case carries a `<failure>` or an `<error>`; a test killed at the per-test
    /// bound is one of these.
    failed: bool,
}

/// Every test case of a `JUnit` report, in document order.
fn parse(xml: &str) -> Result<Vec<TestCase>, String> {
    let mut cases: Vec<TestCase> = Vec::new();
    let mut root = false;
    // Whether the last case read is still open, so that a `<failure>` belongs to it.
    let mut open = false;
    let mut rest = xml;
    while let Some(at) = rest.find('<') {
        rest = &rest[at..];
        if let Some(after) = rest.strip_prefix("<!--") {
            rest = skip_past(after, "-->", "a comment")?;
        } else if let Some(after) = rest.strip_prefix("<![CDATA[") {
            rest = skip_past(after, "]]>", "a CDATA section")?;
        } else if let Some(after) = rest.strip_prefix("<?") {
            rest = skip_past(after, "?>", "a processing instruction")?;
        } else if let Some(after) = rest.strip_prefix("<!") {
            rest = skip_past(after, ">", "a declaration")?;
        } else {
            let (tag, after) = Tag::read(&rest[1..])?;
            rest = after;
            match (tag.name, tag.closing) {
                ("testsuites", false) => root = true,
                ("testcase", false) => {
                    cases.push(tag.test_case()?);
                    open = !tag.empty;
                }
                ("testcase", true) => open = false,
                ("failure" | "error", false) if open => {
                    if let Some(case) = cases.last_mut() {
                        case.failed = true;
                    }
                }
                _ => {}
            }
        }
    }
    if !root {
        return Err("no <testsuites> element: this is not a JUnit report".to_owned());
    }
    Ok(cases)
}

/// What follows `end` in `text`, or the refusal that `what` never closes.
fn skip_past<'xml>(text: &'xml str, end: &str, what: &str) -> Result<&'xml str, String> {
    text.find(end)
        .map(|at| &text[at + end.len()..])
        .ok_or_else(|| format!("{what} that never closes"))
}

/// One start, end or empty-element tag.
struct Tag<'xml> {
    name: &'xml str,
    /// `</name>`.
    closing: bool,
    /// `<name/>`.
    empty: bool,
    attributes: Vec<(&'xml str, String)>,
}

impl<'xml> Tag<'xml> {
    /// Reads a tag from the text after its `<`, and returns it with the text after its `>`.
    fn read(text: &'xml str) -> Result<(Self, &'xml str), String> {
        const NEVER_CLOSES: &str = "a tag that never closes";
        let (closing, text) = match text.strip_prefix('/') {
            Some(text) => (true, text),
            None => (false, text),
        };
        let name_end = text
            .find(|character: char| character.is_whitespace() || matches!(character, '/' | '>'))
            .ok_or(NEVER_CLOSES)?;
        let name = &text[..name_end];
        let mut attributes: Vec<(&str, String)> = Vec::new();
        let mut rest = text[name_end..].trim_start();
        loop {
            if let Some(after) = rest.strip_prefix("/>") {
                return Ok((
                    Self {
                        name,
                        closing,
                        empty: true,
                        attributes,
                    },
                    after,
                ));
            }
            if let Some(after) = rest.strip_prefix('>') {
                return Ok((
                    Self {
                        name,
                        closing,
                        empty: false,
                        attributes,
                    },
                    after,
                ));
            }
            let equals = rest
                .find(['=', '>', '<'])
                .filter(|at| rest[*at..].starts_with('='))
                .ok_or(NEVER_CLOSES)?;
            let attribute = rest[..equals].trim_end();
            let value = rest[equals + 1..].trim_start();
            let quote = value
                .chars()
                .next()
                .filter(|quote| matches!(quote, '"' | '\''))
                .ok_or_else(|| format!("<{name}> has an attribute value that is not quoted"))?;
            let end = value[1..].find(quote).ok_or(NEVER_CLOSES)?;
            if attributes.iter().any(|(seen, _)| *seen == attribute) {
                return Err(format!("<{name}> has the attribute {attribute} twice"));
            }
            attributes.push((attribute, unescape(&value[1..=end])?));
            rest = value[end + 2..].trim_start();
        }
    }

    fn attribute(&self, name: &str) -> Option<&str> {
        self.attributes
            .iter()
            .find(|(attribute, _)| *attribute == name)
            .map(|(_, value)| value.as_str())
    }

    /// The test case a `<testcase>` start tag states.
    fn test_case(&self) -> Result<TestCase, String> {
        let name = self
            .attribute("name")
            .ok_or("a <testcase> without a name")?;
        let shown = format!("<testcase name=\"{name}\">");
        let binary = self
            .attribute("classname")
            .ok_or_else(|| format!("{shown} without a classname"))?;
        let time = self
            .attribute("time")
            .ok_or_else(|| format!("{shown} without a time"))?;
        let seconds = time
            .parse::<f64>()
            .ok()
            .filter(|seconds| seconds.is_finite() && *seconds >= 0.0)
            .ok_or_else(|| format!("{shown} has the time \"{time}\", which is no duration"))?;
        Ok(TestCase {
            binary: binary.to_owned(),
            name: name.to_owned(),
            seconds,
            failed: false,
        })
    }
}

/// An attribute value with its character and predefined entity references replaced.
fn unescape(value: &str) -> Result<String, String> {
    let mut text = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(at) = rest.find('&') {
        text.push_str(&rest[..at]);
        let end = rest[at..]
            .find(';')
            .ok_or_else(|| format!("an entity that never closes in \"{value}\""))?;
        let entity = &rest[at + 1..at + end];
        let character = match entity {
            "lt" => Some('<'),
            "gt" => Some('>'),
            "amp" => Some('&'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => entity
                .strip_prefix("#x")
                .map(|hex| u32::from_str_radix(hex, 16))
                .or_else(|| entity.strip_prefix('#').map(str::parse::<u32>))
                .and_then(Result::ok)
                .and_then(char::from_u32),
        };
        text.push(character.ok_or_else(|| format!("the unknown entity &{entity};"))?);
        rest = &rest[at + end + 1..];
    }
    text.push_str(rest);
    Ok(text)
}

/// The `count` slowest cases, slowest first, under one line that states how many ran and what
/// their times sum to.
fn render(cases: &[TestCase], count: usize) -> String {
    if cases.is_empty() {
        return "slowest tests: the report holds no test\n".to_owned();
    }
    let mut ordered: Vec<&TestCase> = cases.iter().collect();
    ordered.sort_by(|left, right| {
        right
            .seconds
            .total_cmp(&left.seconds)
            .then_with(|| left.binary.cmp(&right.binary))
            .then_with(|| left.name.cmp(&right.name))
    });
    let shown = count.min(ordered.len());
    let all = ordered.len();
    let sum: f64 = cases.iter().map(|case| case.seconds).sum();
    let mut text =
        format!("slowest {shown} of {all} tests (the {all} test times sum to {sum:.3} s):\n");
    for case in &ordered[..shown] {
        let failed = if case.failed { "  FAILED" } else { "" };
        let _ = writeln!(
            text,
            "  {:>8.3} s  {}  {}{failed}",
            case.seconds, case.binary, case.name
        );
    }
    text
}

#[cfg(test)]
mod tests {
    use super::{TestCase, parse, render};

    fn case(binary: &str, name: &str, seconds: f64) -> TestCase {
        TestCase {
            binary: binary.to_owned(),
            name: name.to_owned(),
            seconds,
            failed: false,
        }
    }

    /// The shape cargo-nextest 0.9.143 writes: one suite per test binary, the binary as
    /// `classname`, seconds as `time`.
    const REPORT: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<testsuites name="nextest-run" tests="3" failures="1" errors="0" uuid="8f4f1b0e-0000-4000-8000-000000000000" timestamp="2026-11-21T20:00:00.000+00:00" time="61.250">
    <testsuite name="b10x-substrate-cli::adversary" tests="2" disabled="0" errors="0" failures="1">
        <testcase name="exec_memory_limit_refusal_is_named" classname="b10x-substrate-cli::adversary" timestamp="2026-11-21T20:00:00.100+00:00" time="60.004">
            <failure type="test timeout"/>
            <system-out>a child printed &lt;testcase name="not_a_case" time="999"&gt;</system-out>
        </testcase>
        <testcase name="empty_socket_variable_is_no_socket" classname="b10x-substrate-cli::adversary" timestamp="2026-11-21T20:00:00.100+00:00" time="0.021">
        </testcase>
    </testsuite>
    <testsuite name="xtask::bin/xtask" tests="1" disabled="0" errors="0" failures="0">
        <testcase name="json::tests::a_name_with_&quot;quotes&quot; &amp; an &#x3c;angle&#62;" classname="xtask::bin/xtask" time="1.5"/>
    </testsuite>
</testsuites>
"#;

    #[test]
    fn every_test_case_is_read_with_its_binary_its_time_and_its_outcome() {
        let cases = parse(REPORT).expect("the report parses");
        assert_eq!(
            cases,
            [
                TestCase {
                    failed: true,
                    ..case(
                        "b10x-substrate-cli::adversary",
                        "exec_memory_limit_refusal_is_named",
                        60.004
                    )
                },
                case(
                    "b10x-substrate-cli::adversary",
                    "empty_socket_variable_is_no_socket",
                    0.021
                ),
                case(
                    "xtask::bin/xtask",
                    "json::tests::a_name_with_\"quotes\" & an <angle>",
                    1.5
                ),
            ]
        );
    }

    #[test]
    fn markup_inside_character_data_a_comment_or_an_instruction_is_no_test_case() {
        let report = r#"<?xml version="1.0"?><?note <testcase name="a" classname="b" time="1"/> ?>
<testsuites>
  <!-- <testcase name="commented" classname="b" time="2"/> -->
  <testsuite name="b">
    <testcase name='real' classname='b' time='3'>
      <system-err><![CDATA[<testcase name="cdata" classname="b" time="4"/> ]] > ]]></system-err>
    </testcase>
  </testsuite>
</testsuites>"#;
        assert_eq!(parse(report).expect("parses"), [case("b", "real", 3.0)]);
    }

    #[test]
    fn an_error_child_counts_as_failed_and_a_later_case_does_not_inherit_it() {
        let report = r#"<testsuites><testsuite name="b">
<testcase name="first" classname="b" time="1"><error type="x">boom</error></testcase>
<testcase name="second" classname="b" time="2"></testcase>
</testsuite></testsuites>"#;
        let cases = parse(report).expect("parses");
        assert_eq!(
            cases
                .iter()
                .map(|case| (case.name.as_str(), case.failed))
                .collect::<Vec<_>>(),
            [("first", true), ("second", false)]
        );
    }

    #[test]
    fn a_report_that_is_not_what_nextest_writes_is_refused_by_name() {
        for (report, expected) in [
            ("", "no <testsuites> element"),
            ("<html></html>", "no <testsuites> element"),
            (
                r#"<testsuites><testcase classname="b" time="1"/></testsuites>"#,
                "a <testcase> without a name",
            ),
            (
                r#"<testsuites><testcase name="a" time="1"/></testsuites>"#,
                "<testcase name=\"a\"> without a classname",
            ),
            (
                r#"<testsuites><testcase name="a" classname="b"/></testsuites>"#,
                "<testcase name=\"a\"> without a time",
            ),
            (
                r#"<testsuites><testcase name="a" classname="b" time="soon"/></testsuites>"#,
                "<testcase name=\"a\"> has the time \"soon\"",
            ),
            (
                r#"<testsuites><testcase name="a" classname="b" time="-1"/></testsuites>"#,
                "<testcase name=\"a\"> has the time \"-1\"",
            ),
            (
                r#"<testsuites><testcase name="a" classname="b" time="NaN"/></testsuites>"#,
                "<testcase name=\"a\"> has the time \"NaN\"",
            ),
            (
                r#"<testsuites><testcase name="a" classname="b" time="1""#,
                "a tag that never closes",
            ),
            (
                r#"<testsuites><testcase name=a classname="b" time="1"/></testsuites>"#,
                "an attribute value that is not quoted",
            ),
            (
                r#"<testsuites><testcase name="a" classname="b" time="1" name="c"/></testsuites>"#,
                "the attribute name twice",
            ),
            (
                r#"<testsuites><testcase name="&bogus;" classname="b" time="1"/></testsuites>"#,
                "the unknown entity &bogus;",
            ),
            (
                "<testsuites><!-- never closed",
                "a comment that never closes",
            ),
        ] {
            let error = parse(report).expect_err(report);
            assert!(
                error.contains(expected),
                "{report:?}: got {error:?}, expected it to name {expected:?}"
            );
        }
    }

    #[test]
    fn the_slowest_are_listed_first_and_only_as_many_as_asked() {
        let cases = [
            case("b::fast", "one", 0.5),
            TestCase {
                failed: true,
                ..case("a::slow", "killed", 60.004)
            },
            case("b::fast", "two", 12.25),
            case("b::fast", "three", 0.001),
        ];
        assert_eq!(
            render(&cases, 2),
            "slowest 2 of 4 tests (the 4 test times sum to 72.755 s):\n\
             \x20   60.004 s  a::slow  killed  FAILED\n\
             \x20   12.250 s  b::fast  two\n"
        );
    }

    #[test]
    fn equal_times_are_ordered_by_binary_then_name_so_the_list_is_stable() {
        let cases = [
            case("b", "y", 1.0),
            case("b", "x", 1.0),
            case("a", "z", 1.0),
        ];
        assert_eq!(
            render(&cases, 10),
            "slowest 3 of 3 tests (the 3 test times sum to 3.000 s):\n\
             \x20    1.000 s  a  z\n\
             \x20    1.000 s  b  x\n\
             \x20    1.000 s  b  y\n"
        );
    }

    #[test]
    fn a_run_of_no_tests_says_so() {
        assert_eq!(render(&[], 10), "slowest tests: the report holds no test\n");
    }
}
