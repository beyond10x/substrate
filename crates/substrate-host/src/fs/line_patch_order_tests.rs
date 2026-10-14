//! A line patch addresses every edit against the original file, so the order the edits arrive in
//! carries no meaning: every permutation of one non-overlapping edit set must produce the same
//! bytes, and those bytes are the ascending application written out below as a literal. An
//! overlapping set is refused, with the same code, in every order.

use std::panic::{AssertUnwindSafe, catch_unwind};

use sha2::{Digest as _, Sha256};
use substrate_wire::{FilePatchInput, LinePatchEdit};
use tempfile::tempdir;

use super::{GuardedFilesystem, apply_line_patch};

#[derive(Debug, Clone, PartialEq, Eq)]
enum Outcome {
    Bytes(String),
    Refused(&'static str),
    Panicked(String),
}

fn outcome(source: &str, edits: &[LinePatchEdit]) -> Outcome {
    match catch_unwind(AssertUnwindSafe(|| apply_line_patch(source, edits))) {
        Ok(Ok(text)) => Outcome::Bytes(text),
        Ok(Err(error)) => Outcome::Refused(error.code),
        Err(panic) => Outcome::Panicked(
            panic
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| panic.downcast_ref::<&str>().map(|text| (*text).to_owned()))
                .unwrap_or_default(),
        ),
    }
}

/// Every ordering of `items`, by Heap's algorithm, starting with the given order.
fn permutations<T: Clone>(items: &[T]) -> Vec<Vec<T>> {
    fn heap<T: Clone>(k: usize, items: &mut Vec<T>, out: &mut Vec<Vec<T>>) {
        if k <= 1 {
            out.push(items.clone());
            return;
        }
        heap(k - 1, items, out);
        for i in 0..k - 1 {
            if k % 2 == 0 {
                items.swap(i, k - 1);
            } else {
                items.swap(0, k - 1);
            }
            heap(k - 1, items, out);
        }
    }
    let mut work = items.to_vec();
    let mut out = Vec::new();
    heap(work.len(), &mut work, &mut out);
    out
}

/// Asserts that every permutation of `edits` yields `expected`. Reports each permutation that does
/// not, so a red run names the orders that went wrong rather than only the first.
fn assert_every_order(source: &str, edits: &[LinePatchEdit], expected: &Outcome) {
    let orders = permutations(edits);
    let mut wrong = Vec::new();
    for order in &orders {
        let got = outcome(source, order);
        if &got != expected {
            wrong.push(format!("{order:?}\n    => {got:?}"));
        }
    }
    assert!(
        wrong.is_empty(),
        "{} of {} orders differ from {expected:?}:\n  {}",
        wrong.len(),
        orders.len(),
        wrong.join("\n  ")
    );
}

fn before(line: u32, text: &str) -> LinePatchEdit {
    LinePatchEdit::InsertBefore {
        line,
        text: text.to_owned(),
    }
}

fn after(line: u32, text: &str) -> LinePatchEdit {
    LinePatchEdit::InsertAfter {
        line,
        text: text.to_owned(),
    }
}

fn replace(start_line: u32, end_line: u32, text: &str) -> LinePatchEdit {
    LinePatchEdit::ReplaceRange {
        start_line,
        end_line,
        text: text.to_owned(),
    }
}

fn delete(start_line: u32, end_line: u32) -> LinePatchEdit {
    LinePatchEdit::DeleteRange {
        start_line,
        end_line,
    }
}

fn bytes(text: &str) -> Outcome {
    Outcome::Bytes(text.to_owned())
}

const SIX: &str = "L1\nL2\nL3\nL4\nL5\nL6\n";

#[test]
fn the_reported_two_edit_patch_lands_on_the_same_lines_in_either_order() {
    assert_every_order(
        SIX,
        &[replace(5, 5, "X"), before(2, "A\nB")],
        &bytes("L1\nA\nB\nL2\nL3\nL4\nX\nL6\n"),
    );
}

#[test]
fn an_insert_after_a_line_and_a_replace_of_the_next_line_commute() {
    assert_every_order(
        SIX,
        &[replace(3, 3, "X"), after(2, "A")],
        &bytes("L1\nL2\nA\nX\nL4\nL5\nL6\n"),
    );
}

#[test]
fn an_insert_and_a_replace_on_the_same_line_get_one_outcome_in_every_order() {
    // Whether this pair is refused or applied is the implementation's call; that the answer does
    // not depend on the order is not.
    for edits in [
        [before(3, "A"), replace(3, 3, "X")],
        [after(3, "A"), replace(3, 3, "X")],
        [before(3, "A"), delete(3, 3)],
    ] {
        let first = outcome(SIX, &edits);
        assert!(
            !matches!(first, Outcome::Panicked(_)),
            "{edits:?} panicked: {first:?}"
        );
        assert_every_order(SIX, &edits, &first);
    }
}

#[test]
fn adjacent_ranges_commute() {
    assert_every_order(
        SIX,
        &[replace(1, 2, "A"), delete(3, 4), replace(5, 6, "B")],
        &bytes("A\nB\n"),
    );
}

#[test]
fn a_delete_at_the_end_of_the_file_and_an_insert_after_the_line_before_it_commute() {
    assert_every_order(
        SIX,
        &[delete(6, 6), after(5, "X")],
        &bytes("L1\nL2\nL3\nL4\nL5\nX\n"),
    );
}

#[test]
fn an_insert_after_the_last_line_and_a_shrinking_replace_commute_without_panicking() {
    assert_every_order(
        "L1\nL2\nL3\n",
        &[after(3, "Z"), replace(1, 2, "A")],
        &bytes("A\nL3\nZ\n"),
    );
}

#[test]
fn an_insert_after_one_line_and_an_insert_before_the_next_keep_ascending_order() {
    // Both land in the gap between L2 and L3; ascending order of their original lines puts the
    // insert after L2 first.
    assert_every_order(
        SIX,
        &[before(3, "B"), after(2, "A")],
        &bytes("L1\nL2\nA\nB\nL3\nL4\nL5\nL6\n"),
    );
}

#[test]
fn an_empty_file_refuses_every_edit_in_every_order() {
    assert_every_order(
        "",
        &[before(1, "A"), after(1, "B")],
        &Outcome::Refused("workspace.patch-range"),
    );
}

#[test]
fn a_crlf_file_without_a_final_newline_keeps_both_in_every_order() {
    assert_every_order(
        "L1\r\nL2\r\nL3\r\nL4",
        &[delete(4, 4), before(1, "A"), replace(2, 2, "B")],
        &bytes("A\r\nL1\r\nB\r\nL3"),
    );
}

#[test]
fn four_edits_give_one_result_in_all_twenty_four_orders() {
    assert_every_order(
        "L1\nL2\nL3\nL4\nL5\nL6\nL7\nL8\n",
        &[
            before(1, "H"),
            replace(3, 4, "R1\nR2\nR3"),
            delete(6, 6),
            after(8, "T"),
        ],
        &bytes("H\nL1\nL2\nR1\nR2\nR3\nL5\nL7\nL8\nT\n"),
    );
}

#[test]
fn overlapping_sets_are_refused_in_every_order() {
    for edits in [
        vec![replace(2, 4, "X"), delete(4, 5)],
        vec![before(3, "A"), delete(2, 4)],
        vec![after(1, "A"), replace(4, 4, "X"), delete(4, 6)],
        vec![before(2, "A"), before(2, "B")],
    ] {
        assert_every_order(SIX, &edits, &Outcome::Refused("workspace.patch-overlap"));
    }
}

#[test]
fn the_patch_route_writes_identical_bytes_for_two_permutations_of_one_patch() {
    let source = SIX.as_bytes();
    let expected_sha256 = hex::encode(Sha256::digest(source));
    let ascending = vec![before(2, "A\nB"), replace(5, 5, "X")];
    let descending: Vec<_> = ascending.iter().rev().cloned().collect();
    let mut written = Vec::new();
    for edits in [ascending, descending] {
        let directory = tempdir().expect("tempdir");
        let filesystem = GuardedFilesystem::open(directory.path(), 1024 * 1024, 1024 * 1024, 100)
            .expect("guarded filesystem");
        assert!(
            filesystem.openat2_available(),
            "this case needs openat2; absent, it proves nothing and must not read as passed"
        );
        std::fs::create_dir(directory.path().join("ws")).expect("workspace root");
        std::fs::write(directory.path().join("ws/notes.txt"), source).expect("seed file");
        filesystem
            .patch_cas(
                "ws",
                "ws",
                "notes.txt",
                &FilePatchInput {
                    expected_sha256: expected_sha256.clone(),
                    edits,
                },
            )
            .expect("patch applies");
        written.push(std::fs::read(directory.path().join("ws/notes.txt")).expect("patched"));
    }
    assert_eq!(written[0], b"L1\nA\nB\nL2\nL3\nL4\nX\nL6\n");
    assert_eq!(written[1], written[0]);
}
