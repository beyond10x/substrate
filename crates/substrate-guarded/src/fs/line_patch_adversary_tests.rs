//! Adversarial property for the rule that a line patch changes only the lines it names
//! (the v2 changes of one file, design 01 § 3.1).
//!
//! Over generated files (LF, CRLF and a carriage return inside a line, with and without a line
//! ending at the end) and generated edit lists (inserts before line 1 and after the last line,
//! adjacent ranges, the whole file): a line no edit names keeps its bytes and its line ending, a
//! written line ends as the line its edit names, the file ends without a line ending exactly
//! when it did before, and a file whose every line is deleted is empty.

use substrate_wire::LinePatchEdit;

use super::apply_line_patch;

struct Generator(u64);

impl Generator {
    fn below(&mut self, bound: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        usize::try_from(self.0 % u64::try_from(bound).expect("bound")).expect("below bound")
    }
}

fn split(piece: &str) -> (&str, &str) {
    if let Some(text) = piece.strip_suffix("\r\n") {
        (text, "\r\n")
    } else if let Some(text) = piece.strip_suffix('\n') {
        (text, "\n")
    } else {
        (piece, "")
    }
}

#[test]
#[allow(clippy::too_many_lines)] // One generated property: its generator, its oracle, its checks.
fn a_generated_line_patch_changes_only_the_lines_it_names() {
    let mut generator = Generator(0x2545_f491_4f6c_dd1d);
    let mut written_count = 0_usize;
    for case in 0..20_000 {
        // The file: lines `<L1>` to `<Ln>`, each with its own ending.
        let count = 1 + generator.below(8);
        let unterminated = generator.below(3) == 0;
        let mut pieces = Vec::new();
        for line in 1..=count {
            let inside = if generator.below(5) == 0 {
                "\rtail"
            } else {
                ""
            };
            let ending = if line == count && unterminated {
                ""
            } else if generator.below(2) == 0 {
                "\n"
            } else {
                "\r\n"
            };
            pieces.push(format!("<L{line}>{inside}{ending}"));
        }
        let source = pieces.concat();
        let ending_above_the_last = if count >= 2 {
            split(&pieces[count - 2]).1
        } else {
            "\n"
        };
        let ending_named = |line: usize| match split(&pieces[line - 1]).1 {
            "" => ending_above_the_last,
            ending => ending,
        };

        // The edits, and beside them what the rule says the file holds afterwards: for every
        // line, its marker and the ending it must have while a line follows it.
        let mut edits = Vec::new();
        let mut expected: Vec<(String, String)> = Vec::new();
        let mut text_of =
            |generator: &mut Generator, expected: &mut Vec<(String, String)>, ending: &str| {
                let lines = generator.below(3);
                let separator = if generator.below(2) == 0 {
                    "\n"
                } else {
                    "\r\n"
                };
                let mut text = String::new();
                for _ in 0..lines {
                    written_count += 1;
                    let marker = format!("<W{written_count}>");
                    text.push_str(&marker);
                    text.push_str(separator);
                    expected.push((marker, ending.to_owned()));
                }
                if lines > 0 && generator.below(2) == 0 {
                    text.truncate(text.len() - separator.len());
                }
                text
            };
        let mut line = 1;
        while line <= count {
            let end = (line + generator.below(3)).min(count);
            let original = |line: usize| {
                let (text, ending) = split(&pieces[line - 1]);
                let ending = if ending.is_empty() {
                    ending_above_the_last
                } else {
                    ending
                };
                (text.to_owned(), ending.to_owned())
            };
            let number = |line: usize| u32::try_from(line).expect("line");
            match generator.below(6) {
                0 => {
                    let text = text_of(&mut generator, &mut expected, ending_named(line));
                    expected.push(original(line));
                    edits.push(LinePatchEdit::InsertBefore {
                        line: number(line),
                        text,
                    });
                    line += 1;
                }
                1 => {
                    expected.push(original(line));
                    let text = text_of(&mut generator, &mut expected, ending_named(line));
                    edits.push(LinePatchEdit::InsertAfter {
                        line: number(line),
                        text,
                    });
                    line += 1;
                }
                2 => {
                    let text = text_of(&mut generator, &mut expected, ending_named(line));
                    edits.push(LinePatchEdit::ReplaceRange {
                        start_line: number(line),
                        end_line: number(end),
                        text,
                    });
                    line = end + 1;
                }
                3 => {
                    edits.push(LinePatchEdit::DeleteRange {
                        start_line: number(line),
                        end_line: number(end),
                    });
                    line = end + 1;
                }
                _ => {
                    expected.push(original(line));
                    line += 1;
                }
            }
        }
        let mut wanted = String::new();
        for (at, (text, ending)) in expected.iter().enumerate() {
            wanted.push_str(text);
            // The file ends without a line ending exactly when it did before.
            if at + 1 < expected.len() || !unterminated {
                wanted.push_str(ending);
            }
        }
        // Request order must not matter either.
        if generator.below(2) == 0 {
            edits.reverse();
        }
        let result = apply_line_patch(&source, &edits)
            .unwrap_or_else(|refusal| panic!("case {case}: {source:?} {edits:?}: {refusal:?}"));
        assert_eq!(
            result, wanted,
            "case {case}: the patch {edits:?} of {source:?} changed a byte it does not name"
        );
    }
}
