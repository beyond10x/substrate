//! Adversarial property for the rule that a text edit replaces `old_text` only where the file
//! holds it at exactly one offset (the v2 changes of one file, design 01 § 3.1).
//!
//! Over generated texts of a small alphabet with a two-byte character, line breaks and spaces,
//! the answer is held to a count of every offset at which `old_text` starts, overlapping
//! occurrences included: one offset is replaced there and nowhere else, two or more are
//! `workspace.edit-ambiguous` under both policies, none is `workspace.edit-not-found` under the
//! exact policy.

use substrate_wire::{FileEditInput, TextMatchPolicy};

use super::apply_text_edit;

struct Generator(u64);

impl Generator {
    fn below(&mut self, bound: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        usize::try_from(self.0 % u64::try_from(bound).expect("bound")).expect("below bound")
    }

    fn text(&mut self, least: usize, most: usize) -> String {
        const ALPHABET: [char; 6] = ['a', 'b', '\u{e9}', '\n', ' ', '\t'];
        let length = least + self.below(most - least + 1);
        (0..length)
            .map(|_| ALPHABET[self.below(ALPHABET.len())])
            .collect()
    }
}

#[test]
fn a_generated_text_edit_is_replaced_only_at_exactly_one_offset() {
    let mut generator = Generator(0x9e37_79b9_7f4a_7c15);
    let (mut replaced, mut ambiguous, mut absent) = (0, 0, 0);
    for case in 0..40_000 {
        let source = generator.text(0, 9);
        let old_text = generator.text(1, 3);
        let new_text = generator.text(0, 2);
        let offsets: Vec<usize> = source
            .char_indices()
            .map(|(offset, _)| offset)
            .filter(|offset| source[*offset..].starts_with(&old_text))
            .collect();
        for match_policy in [
            TextMatchPolicy::Exact,
            TextMatchPolicy::LineWhitespaceNormalized,
        ] {
            let answer = apply_text_edit(
                &source,
                &FileEditInput {
                    expected_sha256: String::new(),
                    old_text: old_text.clone(),
                    new_text: new_text.clone(),
                    match_policy,
                },
            );
            let shown = format!("case {case} {match_policy:?}: {old_text:?} in {source:?}");
            match offsets.as_slice() {
                [at] => {
                    replaced += 1;
                    let wanted = format!(
                        "{}{new_text}{}",
                        &source[..*at],
                        &source[at + old_text.len()..]
                    );
                    assert_eq!(answer.expect(&shown), wanted, "{shown}");
                }
                [] if match_policy == TextMatchPolicy::Exact => {
                    absent += 1;
                    assert_eq!(
                        answer.expect_err(&shown).code,
                        "workspace.edit-not-found",
                        "{shown}"
                    );
                }
                // The normalized policy decides this by whole lines; not this property's.
                [] => {}
                _ => {
                    ambiguous += 1;
                    assert_eq!(
                        answer.expect_err(&shown).code,
                        "workspace.edit-ambiguous",
                        "{shown}"
                    );
                }
            }
        }
    }
    assert!(replaced > 1_000 && ambiguous > 1_000 && absent > 1_000);
}
