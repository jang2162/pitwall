//! `git blame --incremental` 출력 해석: 나눠 받아도 같은 결과, 따옴표 경로, 긴 줄, 깨진 출력.

use crate::blame::incremental::{Event, Parser, unquote};

const A: &str = "1111111111111111111111111111111111111111";
const B: &str = "2222222222222222222222222222222222222222";

fn sample() -> String {
    format!(
        "{A} 3 1 2\nauthor Ada\nauthor-mail <ada@x>\nauthor-time 1700000000\nauthor-tz +0000\ncommitter C\nsummary first\nboundary\nfilename \"docs/\\353\\251\\224\\353\\252\\250 n\\\"q\\\".txt\"\n\
         {B} 1 3 1\nauthor Bob\nauthor-mail <bob@x>\nauthor-time 1700000060\nsummary second\nprevious {A} old name.txt\nfilename new.txt\n\
         {A} 9 4 1\nprevious {B} p\nfilename a.txt\n"
    )
}

fn parse_all(bytes: &[u8], step: usize) -> Vec<Event> {
    let mut parser = Parser::default();
    let mut out = Vec::new();
    for piece in bytes.chunks(step) {
        parser.feed(piece, &mut out).unwrap();
    }
    parser.finish().unwrap();
    out
}

#[test]
fn entries_parse_the_same_however_the_output_is_split() {
    let text = sample();
    let whole = parse_all(text.as_bytes(), text.len());
    for step in [1, 2, 7, 64] {
        assert_eq!(parse_all(text.as_bytes(), step), whole, "step {step}");
    }
    assert_eq!(whole.len(), 5);
    let Event::Commit(a) = &whole[0] else {
        panic!("{whole:?}")
    };
    assert_eq!(
        (
            a.author_name.as_str(),
            a.author_email.as_str(),
            a.author_time,
            a.subject.as_str(),
            a.boundary
        ),
        ("Ada", "ada@x", 1_700_000_000, "first", true)
    );
    let Event::Range(r) = &whole[1] else {
        panic!("{whole:?}")
    };
    assert_eq!((r.line, r.count, r.source_line), (1, 2, 3));
    assert_eq!(r.source_path, "docs/메모 n\"q\".txt");
    assert_eq!(r.previous, None);
    let Event::Range(r) = &whole[3] else {
        panic!("{whole:?}")
    };
    let previous = r.previous.as_ref().unwrap();
    assert_eq!(
        (previous.commit.as_str(), previous.path.as_str()),
        (A, "old name.txt")
    );
    // 두 번째로 나온 커밋은 정보 없이 범위만 온다.
    assert!(matches!(&whole[4], Event::Range(r) if r.commit == A && r.line == 4));
}

#[test]
fn quoted_paths_unquote_like_git() {
    assert_eq!(unquote(b"plain name.txt"), "plain name.txt");
    assert_eq!(unquote(br#""a\tb\nc\\d\"e""#), "a\tb\nc\\d\"e");
    assert_eq!(unquote(br#""\303\251""#), "é");
    assert_eq!(unquote(br#""\a\b\v\f\r""#), "\x07\x08\x0b\x0c\r");
    // 따옴표가 한쪽뿐이면 그대로.
    assert_eq!(unquote(br#""half"#), "\"half");
}

#[test]
fn an_overlong_line_is_clipped_not_buffered() {
    let mut parser = Parser::default();
    let mut out = Vec::new();
    parser
        .feed(format!("{A} 1 1 1\nsummary ").as_bytes(), &mut out)
        .unwrap();
    let chunk = vec![b'x'; 64 * 1024];
    for _ in 0..32 {
        parser.feed(&chunk, &mut out).unwrap();
    }
    assert!(
        parser.buffered() <= 64 * 1024,
        "{} bytes held",
        parser.buffered()
    );
    parser.feed(b"\nfilename f\n", &mut out).unwrap();
    parser.finish().unwrap();
    let Event::Commit(c) = &out[0] else {
        panic!("{out:?}")
    };
    assert!(c.subject.chars().count() <= crate::log::commit::MAX_SUBJECT + 1);
}

#[test]
fn broken_output_is_an_error() {
    let mut out = Vec::new();
    for bad in [
        "zz 1 1 1\n",
        &format!("{A} 1 1\n"),
        &format!("{A} 0 1 1\n"),
        &format!("{A} 1 1 x\n"),
        &format!("{A} 1 1 1\nprevious {A}\n"),
    ] {
        assert!(
            Parser::default().feed(bad.as_bytes(), &mut out).is_err(),
            "{bad:?}"
        );
    }
    let mut p = Parser::default();
    p.feed(format!("{A} 1 1 1\nauthor x\n").as_bytes(), &mut out)
        .unwrap();
    assert!(p.finish().is_err(), "ended in the middle of an entry");
}
