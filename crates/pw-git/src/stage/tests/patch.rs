//! 고른 줄 → 목표 내용 → 패치(git 없이). 한 구간 안에서 남는 옛 줄이 먼저 오고, 개행 없는 끝 줄 뒤에 줄이 오면 줄바꿈을 붙이고,
//! 패치의 이름은 C 꼴 따옴표로 감싼다.

use gix::diff::blob::Algorithm;

use crate::diff::hunks;
use crate::stage::lines::{Direction, Picked, patch, quote, target};

/// `old` → `new` 에서 고른 더한 줄(새 쪽 0 기준) · 뺀 줄(옛 쪽 0 기준)로 만든 목표.
fn build(old: &[u8], new: &[u8], removed: &[u32], added: &[u32], direction: Direction) -> Vec<u8> {
    let (o, n) = (hunks::lines(old), hunks::lines(new));
    let changes = hunks::changes(&o, &n, Algorithm::Myers, false);
    let picked = Picked {
        removed: removed.iter().copied().collect(),
        added: added.iter().copied().collect(),
    };
    target(&o, &n, &changes, &picked, direction)
}

#[test]
fn applying_or_reverting_everything_or_nothing_gives_one_side() {
    let old = b"1\n2\nthree\r\n4\n5\n6\n7\n8\n9\nten";
    let new = b"1\nTWO\nthree\r\n4\n5\nsix\n6\n7\n8\n9\n10\n";
    let (o, n) = (hunks::lines(old), hunks::lines(new));
    let changes = hunks::changes(&o, &n, Algorithm::Myers, false);
    let all = Picked {
        removed: changes.iter().flat_map(|c| c.before.clone()).collect(),
        added: changes.iter().flat_map(|c| c.after.clone()).collect(),
    };
    let none = Picked::default();
    assert_eq!(target(&o, &n, &changes, &all, Direction::Apply), new);
    assert_eq!(target(&o, &n, &changes, &none, Direction::Apply), old);
    assert_eq!(target(&o, &n, &changes, &all, Direction::Revert), old);
    assert_eq!(target(&o, &n, &changes, &none, Direction::Revert), new);
    // 고른 것을 되돌리는 것 == 고르지 않은 것을 더하는 것.
    let some = Picked {
        removed: [1].into(),
        added: [5].into(),
    };
    let rest = Picked {
        removed: all.removed.difference(&some.removed).copied().collect(),
        added: all.added.difference(&some.added).copied().collect(),
    };
    assert_eq!(
        target(&o, &n, &changes, &some, Direction::Revert),
        target(&o, &n, &changes, &rest, Direction::Apply)
    );
}

#[test]
fn a_kept_old_line_comes_before_an_added_line_in_the_same_change() {
    // 2 → two 를 바꾼 구간에서 더한 줄만 고르면 옛 줄이 남고 그 뒤에 새 줄이 온다.
    assert_eq!(
        build(b"1\n2\n3\n", b"1\ntwo\n3\n", &[], &[1], Direction::Apply),
        b"1\n2\ntwo\n3\n"
    );
    // 뺀 줄만 고르면 지우기만 담는다.
    assert_eq!(
        build(b"1\n2\n3\n", b"1\ntwo\n3\n", &[1], &[], Direction::Apply),
        b"1\n3\n"
    );
}

#[test]
fn a_last_line_without_newline_gets_one_when_a_line_follows_it() {
    // 개행 없는 끝 줄 `y` 를 남기고 그 뒤에 `z` 를 더하면 `y` 에 줄바꿈이 붙는다 — 이을 줄의 줄바꿈 꼴로.
    assert_eq!(
        build(b"x\ny", b"x\nY\nz", &[], &[2], Direction::Apply),
        b"x\ny\nz"
    );
    assert_eq!(
        build(b"x\r\ny", b"x\r\nY\r\nz\r\n", &[], &[2], Direction::Apply),
        b"x\r\ny\r\nz\r\n"
    );
    // 되돌리기도 같다: `y` 를 되살리고 고르지 않은 `Y` 를 남긴다.
    assert_eq!(
        build(b"x\ny", b"x\nY\n", &[1], &[], Direction::Revert),
        b"x\ny\nY\n"
    );
}

#[test]
fn a_patch_marks_a_missing_newline_and_counts_lines_like_git() {
    assert_eq!(
        String::from_utf8(patch("f", b"1\n2\n3\n", b"1\n2\nthree").unwrap()).unwrap(),
        "diff --git \"a/f\" \"b/f\"\n--- \"a/f\"\n+++ \"b/f\"\n@@ -1,3 +1,3 @@\n 1\n 2\n-3\n+three\n\\ No newline at end of file\n"
    );
    // 빈 쪽의 시작은 그 앞 줄 번호다.
    assert_eq!(
        String::from_utf8(patch("f", b"", b"a\n").unwrap()).unwrap(),
        "diff --git \"a/f\" \"b/f\"\n--- \"a/f\"\n+++ \"b/f\"\n@@ -0,0 +1,1 @@\n+a\n"
    );
    // 멀리 떨어진 변경은 hunk 를 나눈다(사이 같은 줄이 문맥 두 배 6줄을 넘는다).
    let before = b"0\n1\n2\n3\n4\n5\n6\n7\n8\n9\n";
    let after = b"zero\n1\n2\n3\n4\n5\n6\n7\n8\nnine\n";
    let text = String::from_utf8(patch("f", before, after).unwrap()).unwrap();
    assert_eq!(text.matches("\n@@ ").count(), 2, "{text}");
    assert!(
        text.contains(
            "@@ -1,4 +1,4 @@\n-0\n+zero\n 1\n 2\n 3\n@@ -7,4 +7,4 @@\n 6\n 7\n 8\n-9\n+nine\n"
        ),
        "{text}"
    );
    assert_eq!(patch("f", b"same\n", b"same\n"), None);
}

#[test]
fn names_are_c_quoted_for_git_apply() {
    assert_eq!(
        quote("a/dir/sp ace\t\"q\"\\é\n"),
        "\"a/dir/sp ace\\t\\\"q\\\"\\\\\\303\\251\\n\""
    );
}
