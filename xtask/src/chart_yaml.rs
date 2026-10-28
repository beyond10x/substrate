//! The block-YAML subset `helm template` produces from this repository's chart, read into JSON.
//!
//! No YAML crate is in the locked graph, and adding one is a licence and advisory decision of its
//! own. The chart lane reads only what its own templates render: block mappings and sequences,
//! plain, single- and double-quoted scalars, JSON flow collections (`toYaml` writes `{}` and `[]`),
//! and `|`/`|-` literal blocks. Anything else — anchors, tags, folded or multi-line plain scalars,
//! inline comments — is refused with its line number, never guessed at. Plain scalars are typed
//! the way the Kubernetes API reads them: `true`/`false`, `null`/`~`, decimal integers, a leading
//! `0` as octal (so `defaultMode: 0440` is 288), and decimal fractions.

use anyhow::{Result, bail};
use serde_json::{Map, Number, Value};

/// Every non-empty document of a `---`-separated stream.
pub fn documents(text: &str) -> Result<Vec<Value>> {
    let mut documents = Vec::new();
    let mut current: Vec<(usize, &str)> = Vec::new();
    for (index, line) in text.lines().enumerate() {
        if line.trim_end() == "---" {
            if let Some(document) = document(&current)? {
                documents.push(document);
            }
            current.clear();
        } else {
            current.push((index + 1, line));
        }
    }
    if let Some(document) = document(&current)? {
        documents.push(document);
    }
    Ok(documents)
}

#[derive(Clone)]
struct Line {
    number: usize,
    indent: usize,
    text: String,
    raw: String,
}

fn document(lines: &[(usize, &str)]) -> Result<Option<Value>> {
    let mut parsed = Vec::new();
    for (number, raw) in lines {
        if raw.contains('\t') {
            bail!("line {number}: a tab character is outside the YAML subset the lane reads");
        }
        let text = raw.trim_start();
        let indent = raw.len() - text.len();
        parsed.push(Line {
            number: *number,
            indent,
            text: text.trim_end().to_owned(),
            raw: (*raw).to_owned(),
        });
    }
    let mut parser = Parser {
        lines: parsed,
        at: 0,
    };
    parser.skip_insignificant();
    if parser.at >= parser.lines.len() {
        return Ok(None);
    }
    let indent = parser.lines[parser.at].indent;
    let value = parser.node(indent)?;
    parser.skip_insignificant();
    if let Some(line) = parser.lines.get(parser.at) {
        bail!(
            "line {}: unexpected content at indent {}",
            line.number,
            line.indent
        );
    }
    Ok(Some(value))
}

struct Parser {
    lines: Vec<Line>,
    at: usize,
}

fn insignificant(line: &Line) -> bool {
    line.text.is_empty() || line.text.starts_with('#')
}

fn is_sequence_item(text: &str) -> bool {
    text == "-" || text.starts_with("- ")
}

impl Parser {
    fn skip_insignificant(&mut self) {
        while self.lines.get(self.at).is_some_and(insignificant) {
            self.at += 1;
        }
    }

    fn peek(&mut self) -> Option<Line> {
        self.skip_insignificant();
        self.lines.get(self.at).cloned()
    }

    fn node(&mut self, indent: usize) -> Result<Value> {
        let Some(line) = self.peek() else {
            return Ok(Value::Null);
        };
        if is_sequence_item(&line.text) {
            self.sequence(indent)
        } else {
            self.mapping(indent)
        }
    }

    fn sequence(&mut self, indent: usize) -> Result<Value> {
        let mut items = Vec::new();
        while let Some(line) = self.peek() {
            if line.indent < indent {
                break;
            }
            if line.indent > indent {
                bail!("line {}: over-indented sequence content", line.number);
            }
            if !is_sequence_item(&line.text) {
                break;
            }
            let rest = line.text[1..].trim_start().to_owned();
            if rest.is_empty() {
                self.at += 1;
                items.push(self.nested(indent, false)?);
                continue;
            }
            // `- key: value` opens a mapping whose first entry sits two columns in; rewrite the
            // line as that entry and read the mapping at its real indent.
            let inner = indent + (line.text.len() - rest.len());
            self.lines[self.at].indent = inner;
            self.lines[self.at].text.clone_from(&rest);
            if is_sequence_item(&rest) {
                items.push(self.sequence(inner)?);
            } else if split_key(&rest, line.number)?.is_some() {
                items.push(self.mapping(inner)?);
            } else {
                self.at += 1;
                items.push(self.scalar_or_block(&rest, inner, line.number)?);
            }
        }
        Ok(Value::Array(items))
    }

    fn mapping(&mut self, indent: usize) -> Result<Value> {
        let mut map = Map::new();
        while let Some(line) = self.peek() {
            if line.indent < indent || (line.indent == indent && is_sequence_item(&line.text)) {
                break;
            }
            if line.indent > indent {
                bail!("line {}: over-indented mapping content", line.number);
            }
            let Some((key, rest)) = split_key(&line.text, line.number)? else {
                bail!(
                    "line {}: expected `key:` in a mapping, found {:?}",
                    line.number,
                    line.text
                );
            };
            self.at += 1;
            let value = if rest.is_empty() {
                self.nested(indent, true)?
            } else {
                self.scalar_or_block(&rest, indent, line.number)?
            };
            if map.insert(key.clone(), value).is_some() {
                bail!("line {}: duplicate key {key:?}", line.number);
            }
        }
        Ok(Value::Object(map))
    }

    /// The value of a `key:` or `-` with nothing after it: a deeper block, a same-indent sequence
    /// (only after a mapping key, as `toYaml` writes it), or null.
    fn nested(&mut self, indent: usize, after_key: bool) -> Result<Value> {
        let Some(next) = self.peek() else {
            return Ok(Value::Null);
        };
        if next.indent > indent {
            return self.node(next.indent);
        }
        if after_key && next.indent == indent && is_sequence_item(&next.text) {
            return self.sequence(indent);
        }
        Ok(Value::Null)
    }

    fn scalar_or_block(&mut self, text: &str, indent: usize, number: usize) -> Result<Value> {
        match text {
            "|" | "|-" => {
                let keep = text == "|";
                let mut body: Vec<String> = Vec::new();
                let mut block_indent = None;
                while let Some(line) = self.lines.get(self.at) {
                    if line.text.is_empty() {
                        body.push(String::new());
                        self.at += 1;
                        continue;
                    }
                    if line.indent <= indent {
                        break;
                    }
                    let width = *block_indent.get_or_insert(line.indent);
                    if line.indent < width {
                        break;
                    }
                    body.push(line.raw[width..].trim_end().to_owned());
                    self.at += 1;
                }
                while body.last().is_some_and(String::is_empty) {
                    body.pop();
                }
                let mut joined = body.join("\n");
                if keep && !joined.is_empty() {
                    joined.push('\n');
                }
                Ok(Value::String(joined))
            }
            _ if text.starts_with(['>', '&', '*', '!', '%', '@', '`']) => {
                bail!("line {number}: {text:?} is outside the YAML subset the lane reads")
            }
            _ => scalar(text, number),
        }
    }
}

/// `key: rest` or `key:`, with an optionally quoted key; `None` when the text is not an entry.
fn split_key(text: &str, number: usize) -> Result<Option<(String, String)>> {
    let (key, after) = if let Some(quote) = text.chars().next().filter(|c| *c == '"' || *c == '\'')
    {
        let Some(end) = closing_quote(text, quote) else {
            bail!("line {number}: unterminated quoted key");
        };
        let key = scalar(&text[..=end], number)?;
        let Value::String(key) = key else {
            bail!("line {number}: a quoted key is not a string");
        };
        let after = &text[end + 1..];
        if !(after.starts_with(": ") || after == ":") {
            return Ok(None);
        }
        (key, after)
    } else if text.starts_with(['{', '[']) {
        return Ok(None);
    } else {
        let colon = text
            .find(": ")
            .or_else(|| text.strip_suffix(':').map(str::len));
        let Some(colon) = colon else {
            return Ok(None);
        };
        let key = &text[..colon];
        if key.ends_with(char::is_whitespace) {
            bail!(
                "line {number}: whitespace before the colon of key {key:?} is outside the YAML subset the lane reads"
            );
        }
        if key.starts_with([
            '&', '!', '*', '?', '|', '>', '%', '@', '`', '[', '{', ',', '#',
        ]) || key == "<<"
            || key.contains(" #")
        {
            bail!("line {number}: key {key:?} is outside the YAML subset the lane reads");
        }
        if !matches!(scalar(key, number)?, Value::String(_)) {
            bail!("line {number}: key {key:?} is not a plain string to the Kubernetes decoder");
        }
        (key.to_owned(), &text[colon..])
    };
    let rest = after[1..].trim().to_owned();
    if key.is_empty() {
        bail!("line {number}: empty mapping key");
    }
    Ok(Some((key, rest)))
}

fn closing_quote(text: &str, quote: char) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut index = 1;
    while index < bytes.len() {
        let byte = bytes[index];
        if quote == '"' && byte == b'\\' {
            index += 2;
            continue;
        }
        if byte == quote as u8 {
            if quote == '\'' && bytes.get(index + 1) == Some(&b'\'') {
                index += 2;
                continue;
            }
            return Some(index);
        }
        index += 1;
    }
    None
}

fn scalar(text: &str, number: usize) -> Result<Value> {
    let text = text.trim();
    if text.starts_with('"') {
        if closing_quote(text, '"') != Some(text.len() - 1) {
            bail!("line {number}: content after a double-quoted scalar");
        }
        return serde_json::from_str(text).map_err(|error| {
            anyhow::anyhow!("line {number}: double-quoted scalar outside JSON escapes: {error}")
        });
    }
    if text.starts_with('\'') {
        if closing_quote(text, '\'') != Some(text.len() - 1) {
            bail!("line {number}: content after a single-quoted scalar");
        }
        return Ok(Value::String(text[1..text.len() - 1].replace("''", "'")));
    }
    if text.starts_with(['{', '[']) {
        return serde_json::from_str(text).map_err(|error| {
            anyhow::anyhow!("line {number}: a flow collection that is not JSON: {error}")
        });
    }
    if text.contains(" #") {
        bail!("line {number}: an inline comment is outside the YAML subset the lane reads");
    }
    let value = match text {
        "" | "null" | "~" => Value::Null,
        "true" => Value::Bool(true),
        "false" => Value::Bool(false),
        _ => integer(text)
            .or_else(|| fraction(text))
            .unwrap_or_else(|| Value::String(text.to_owned())),
    };
    if value.is_string() && typed_by_yaml_1_1(text) {
        bail!(
            "line {number}: the plain scalar {text:?} is a boolean, null or number to the Kubernetes decoder (YAML 1.1); quote it or spell it canonically"
        );
    }
    Ok(value)
}

/// A plain scalar that `sigs.k8s.io/yaml` (`gopkg.in/yaml.v2` resolution) reads as something other
/// than a string, in a spelling this reader would otherwise take as one: the YAML 1.1 booleans and
/// nulls in any case (`True`, `yes`, `ON`, `y`, `NULL`), infinities and NaN, and numbers spelled
/// with `_`, a `0x`/`0o`/`0b` prefix, an exponent, a bare fraction or trailing dot, a leading zero
/// before a non-octal digit, or more digits than an `i64` holds.
fn typed_by_yaml_1_1(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    if matches!(
        lower.as_str(),
        "y" | "yes" | "n" | "no" | "on" | "off" | "true" | "false" | "null"
    ) {
        return true;
    }
    let unsigned = lower.strip_prefix(['-', '+']).unwrap_or(&lower);
    if matches!(unsigned, ".inf" | ".nan") {
        return true;
    }
    if !text.starts_with(|c: char| c.is_ascii_digit() || matches!(c, '-' | '+' | '.')) {
        return false;
    }
    let plain = unsigned.replace('_', "");
    let prefixed = |prefix: &str, radix: u32| {
        plain
            .strip_prefix(prefix)
            .is_some_and(|digits| !digits.is_empty() && digits.chars().all(|c| c.is_digit(radix)))
    };
    prefixed("0x", 16) || prefixed("0o", 8) || prefixed("0b", 2) || yaml_float(&plain)
}

/// `gopkg.in/yaml.v2`'s float form, `^[-+]?(\.[0-9]+|[0-9]+(\.[0-9]*)?)([eE][-+]?[0-9]+)?$`, on an
/// unsigned text.
fn yaml_float(text: &str) -> bool {
    let (mantissa, exponent) = match text.split_once('e') {
        Some((mantissa, exponent)) => (mantissa, Some(exponent)),
        None => (text, None),
    };
    let digits = |part: &str| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit());
    let mantissa_ok = match mantissa.split_once('.') {
        None => digits(mantissa),
        Some(("", fraction)) => digits(fraction),
        Some((whole, fraction)) => {
            digits(whole) && fraction.bytes().all(|byte| byte.is_ascii_digit())
        }
    };
    let exponent_ok = exponent
        .is_none_or(|exponent| digits(exponent.strip_prefix(['-', '+']).unwrap_or(exponent)));
    mantissa_ok && exponent_ok
}

fn integer(text: &str) -> Option<Value> {
    let (negative, digits) = match text.as_bytes().first()? {
        b'-' => (true, &text[1..]),
        b'+' => (false, &text[1..]),
        _ => (false, text),
    };
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let magnitude = if digits.len() > 1 && digits.starts_with('0') {
        i64::from_str_radix(&digits[1..], 8).ok()?
    } else {
        digits.parse::<i64>().ok()?
    };
    Some(Value::Number(Number::from(if negative {
        -magnitude
    } else {
        magnitude
    })))
}

fn fraction(text: &str) -> Option<Value> {
    let body = text.strip_prefix(['-', '+']).unwrap_or(text);
    let (whole, part) = body.split_once('.')?;
    if part.is_empty()
        || !whole.bytes().all(|byte| byte.is_ascii_digit())
        || !part.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    Number::from_f64(text.parse().ok()?).map(Value::Number)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::documents;

    #[test]
    fn the_chart_shapes_read_as_json() {
        let text = "---\n# Source: x\napiVersion: v1\nkind: Service\nmetadata:\n  name: lane\n  labels:\n    app.kubernetes.io/name: \"b10x\"\nspec:\n  clusterIP: None\n  ports:\n    - name: control\n      port: 8443\n  selector: {}\n---\n";
        assert_eq!(
            documents(text).unwrap(),
            vec![json!({
                "apiVersion": "v1",
                "kind": "Service",
                "metadata": {"name": "lane", "labels": {"app.kubernetes.io/name": "b10x"}},
                "spec": {"clusterIP": "None", "ports": [{"name": "control", "port": 8443}], "selector": {}}
            })]
        );
    }

    #[test]
    fn sequences_nest_at_and_beyond_their_key_and_items_may_open_below_a_dash() {
        let text = "a:\n- x\n- z\nb:\n  - -m\n  - \"0400\"\n  - --\nc:\n  -\n    k: v\n    l:\n    - 1\nd:\n";
        assert_eq!(
            documents(text).unwrap()[0],
            json!({"a": ["x", "z"], "b": ["-m", "0400", "--"], "c": [{"k": "v", "l": [1]}], "d": null})
        );
    }

    #[test]
    fn plain_scalars_are_typed_as_the_api_reads_them() {
        let text = "mode: 0440\nport: 9090\ncpu: 0.5\nenabled: true\nnone: ~\nflag: 'it''s'\naddress: 0.0.0.0:8443\nref: $(POD_NAME)\nids: 200000-200511\nlimit: 16MiB\ncidr: 10.0.0.0/8\nflag2: --state\n";
        assert_eq!(
            documents(text).unwrap()[0],
            json!({"mode": 288, "port": 9090, "cpu": 0.5, "enabled": true, "none": null, "flag": "it's", "address": "0.0.0.0:8443", "ref": "$(POD_NAME)", "ids": "200000-200511", "limit": "16MiB", "cidr": "10.0.0.0/8", "flag2": "--state"})
        );
    }

    /// The class behind adversary A-F5: every plain scalar `gopkg.in/yaml.v2` types differently
    /// from this reader is refused by line, as a value and as a key.
    #[test]
    fn plain_scalars_the_kubernetes_decoder_types_otherwise_are_refused_by_line() {
        for scalar in [
            "True",
            "TRUE",
            "yes",
            "Yes",
            "NO",
            "n",
            "Y",
            "on",
            "On",
            "OFF",
            "off",
            "Null",
            "NULL",
            ".inf",
            "-.Inf",
            "+.INF",
            ".nan",
            ".NaN",
            "0x1F",
            "0o17",
            "0b101",
            "1_000",
            "1e3",
            "1.5E-3",
            ".5e2",
            "1.",
            "09",
            "99999999999999999999",
        ] {
            for text in [
                format!("a: {scalar}\n"),
                format!("- {scalar}\n"),
                format!("{scalar}: x\n"),
            ] {
                let error = documents(&text).map(|value| value[0].clone());
                assert!(
                    error
                        .as_ref()
                        .is_err_and(|error| error.to_string().contains("line 1")),
                    "{text:?} read as {error:?}"
                );
            }
        }
    }

    /// The class behind adversary A-F6: a key the decoder reads with a different spelling.
    #[test]
    fn keys_spelled_with_space_anchor_tag_alias_or_merge_are_refused_by_line() {
        for text in [
            "a: 1\nprivileged : true\n",
            "a: 1\n&a privileged: true\n",
            "a: 1\n!!str privileged: true\n",
            "a: 1\n*a: true\n",
            "a: 1\n? privileged: true\n",
            "a: 1\n<<: {\"privileged\": true}\n",
            "a: 1\nb #c: d\n",
        ] {
            let error = documents(text).unwrap_err().to_string();
            assert!(error.contains("line 2"), "{text:?}: {error}");
        }
    }

    #[test]
    fn literal_blocks_keep_or_strip_their_final_newline() {
        let text = "a: |\n  one\n  two\nb: |-\n  three\nc: x\n";
        assert_eq!(
            documents(text).unwrap()[0],
            json!({"a": "one\ntwo\n", "b": "three", "c": "x"})
        );
    }

    #[test]
    fn what_the_subset_does_not_cover_is_refused_by_line() {
        for (text, line) in [
            ("a: &anchor x\n", "line 1"),
            ("a: >\n  folded\n", "line 1"),
            ("a: x # note\n", "line 1"),
            ("a: b\nb:\n    c: d\n   e: f\n", "line 4"),
            ("a: 1\na: 2\n", "line 2"),
            ("a: \"x\" y\n", "line 1"),
        ] {
            let error = documents(text).unwrap_err().to_string();
            assert!(error.contains(line), "{text:?}: {error}");
        }
    }
}
