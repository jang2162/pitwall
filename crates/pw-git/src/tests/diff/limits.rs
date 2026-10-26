//! 응답 크기 · 메모리 상한과 파일 하나의 실패: hunk 가 파일 하나의 상한을 넘으면 `tooLarge`, 응답 전체의 상한에 닿으면 뒤 파일은
//! `hunksOmitted`(그 파일만 다시 부르면 받는다), 편집 상한을 넘는 파일은 `git.blob` 과 같은 차례를 기다리고, 내용을 못 읽은
//! 파일은 그 파일에만 `error` 가 달린다.

use std::time::Duration;

use super::*;
use crate::diff::blob::{BIG_BLOBS, BIG_BLOBS_AT_ONCE};
use crate::diff::changes::{DIFF_HUNKS_MAX, FILE_HUNKS_MAX};
use crate::diff::hunks::{self, LINE_COST};

/// `n` 줄, 줄마다 `tag` 와 번호.
fn tagged(tag: &str, n: usize) -> String {
    (0..n).map(|i| format!("{tag}{i:06}\n")).collect()
}

/// 모든 줄이 바뀐 파일의 hunk 크기(줄 수 × (글 + 줄 몫), hunk 하나).
fn changed_size(lines: usize, text: usize) -> u64 {
    2 * lines as u64 * (text as u64 + LINE_COST)
}

#[test]
fn a_file_whose_hunks_exceed_the_file_limit_is_too_large_and_keeps_its_counts() {
    // 6만 줄이 모두 바뀐다: 12만 줄 × (7 + 96) ≈ 11.8 MiB > 5 MiB. 파일 자체는 420 KiB 다.
    let n = 60_000;
    assert!(changed_size(n, 7) > FILE_HUNKS_MAX);
    let r = TestRepo::with_commit();
    r.write("many.txt", &tagged("a", n));
    r.write("small.txt", "small\n");
    r.commit_all("a");
    r.write("many.txt", &tagged("b", n));
    r.write("small.txt", "smaller\n");
    r.commit_all("b");
    let repo = Repo::new(r);

    let got = repo.diff(None, commit("HEAD"));
    let many = got.files.iter().find(|f| f.path == "many.txt").unwrap();
    assert!(
        many.too_large && !many.hunks_omitted && !many.binary && many.hunks.is_empty(),
        "{:?}",
        (many.too_large, many.hunks_omitted, many.binary)
    );
    assert_eq!((many.added, many.removed), (n as u32, n as u32));
    let small = got.files.iter().find(|f| f.path == "small.txt").unwrap();
    assert!(!small.too_large && !small.hunks.is_empty());

    // 그 파일만 불러도 같다(영구).
    let mut params = repo.params(None, commit("HEAD"));
    params.paths = Some(vec!["many.txt".to_owned()]);
    let alone = repo.f.git.diff(&params).unwrap();
    assert!(alone.files[0].too_large && alone.files[0].hunks.is_empty());
}

/// 응답 전체의 상한에 닿으면 그 뒤 파일은 hunk 없이 `hunksOmitted` 다(목록 · 줄 수는 남는다). 그 파일만 다시 부르면 hunk 를 받는다.
/// 응답 JSON 은 상한을 크게 넘지 않는다.
#[test]
fn files_past_the_response_limit_omit_their_hunks_and_can_be_fetched_alone() {
    // 파일마다 2만 줄이 바뀐다: 4만 줄 × (7 + 96) ≈ 3.9 MiB(< 5 MiB). 14개면 ≈ 55 MiB > 50 MiB.
    let (n, count) = (20_000, 14);
    let each = changed_size(n, 7);
    assert!(each < FILE_HUNKS_MAX && each * count > DIFF_HUNKS_MAX);
    let r = TestRepo::with_commit();
    for i in 0..count {
        r.write(&format!("f{i:02}.txt"), &tagged("a", n));
    }
    r.commit_all("a");
    for i in 0..count {
        r.write(&format!("f{i:02}.txt"), &tagged("b", n));
    }
    r.commit_all("b");
    let repo = Repo::new(r);

    let got = repo.diff(None, commit("HEAD"));
    assert_eq!(got.files.len(), count as usize);
    let omitted: Vec<&DiffFile> = got.files.iter().filter(|f| f.hunks_omitted).collect();
    let shown = got.files.iter().filter(|f| !f.hunks.is_empty()).count();
    assert!(!omitted.is_empty() && shown > 0, "{shown} shown");
    assert_eq!(shown + omitted.len(), count as usize);
    for f in &omitted {
        assert!(!f.too_large && f.hunks.is_empty() && (f.added, f.removed) == (n as u32, n as u32));
    }
    let json = serde_json::to_vec(&got).unwrap().len() as u64;
    assert!(json <= DIFF_HUNKS_MAX, "{json} bytes of JSON");

    let mut params = repo.params(None, commit("HEAD"));
    params.paths = Some(vec![omitted[0].path.clone()]);
    let alone = repo.f.git.diff(&params).unwrap();
    assert!(!alone.files[0].hunks_omitted && !alone.files[0].hunks.is_empty());
}

/// 셈([`hunks::size`])은 실제 JSON 크기의 위 한계다 — 이스케이프할 글자 · 제어 문자 · UTF-8 이 아닌 바이트 · 큰 줄 번호에서도.
#[test]
fn the_hunk_size_is_an_upper_bound_of_the_json() {
    // 긴 줄은 줄 고정 몫의 여유로 덮이지 않는다 — 제어 문자 하나가 6바이트, 깨진 바이트 하나가 3바이트다.
    let repeat = |unit: &[u8]| -> Vec<u8> {
        let mut line = unit.repeat(200);
        line.push(b'\n');
        line
    };
    let (ctrl, quotes, broken) = (repeat(b"\x01"), repeat(b"\""), repeat(b"\xff"));
    let pool: [&[u8]; 12] = [
        b"plain\n",
        b"\"quoted\" \\ back\n",
        b"\x01\x02\x1f ctrl\n",
        b"\xff\xfe not utf-8\n",
        b"tab\tcr\r\n",
        b"\xed\x95\x9c\xea\xb8\x80\n",
        b"\n",
        b"x",
        b"fn f() {\n",
        &ctrl,
        &quotes,
        &broken,
    ];
    let mut seed: u64 = 0x5eed;
    let mut next = move |n: usize| {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((seed >> 33) as usize) % n
    };
    for _ in 0..200 {
        let mut old = Vec::new();
        let mut new = Vec::new();
        for _ in 0..next(300) {
            let line = pool[next(pool.len())];
            old.extend_from_slice(line);
            new.extend_from_slice(if next(3) == 0 {
                pool[next(pool.len())]
            } else {
                line
            });
        }
        let lines = hunks::hunks(
            &old,
            &new,
            gix::diff::blob::Algorithm::Myers,
            false,
            u64::MAX,
        );
        // 셈은 hunk 들이다 — 목록의 괄호(`[]`) 2바이트는 파일 몫이다.
        let json = serde_json::to_vec(lines.hunks.as_ref().unwrap())
            .unwrap()
            .len() as u64
            - 2;
        assert!(json <= lines.size, "json {json} > size {}", lines.size);
    }
}

/// 편집 상한(5 MiB)을 넘는 파일의 diff 는 `git.blob` 과 같은 차례를 얻는다: 자리가 다 차 있으면 그 파일만 「바쁨」 오류이고 다른
/// 파일은 나온다. 자리가 나면 hunk 가 나온다.
#[test]
fn a_large_file_diff_waits_for_the_same_seat_as_large_blobs() {
    let r = TestRepo::with_commit();
    r.write("six.txt", &"x\n".repeat(3 * 1024 * 1024));
    r.write("small.txt", "small\n");
    r.commit_all("a");
    r.write(
        "six.txt",
        &("y\n".to_owned() + &"x\n".repeat(3 * 1024 * 1024 - 1)),
    );
    r.write("small.txt", "smaller\n");
    r.commit_all("b");
    let repo = Repo::new(r);

    let seats: Vec<_> = (0..BIG_BLOBS_AT_ONCE)
        .map(|_| BIG_BLOBS.enter(Duration::ZERO).unwrap())
        .collect();
    let got = repo.diff(None, commit("HEAD"));
    let six = got.files.iter().find(|f| f.path == "six.txt").unwrap();
    assert!(
        six.error.as_deref().is_some_and(|e| e.contains("busy")) && six.hunks.is_empty(),
        "{:?}",
        six.error
    );
    let small = got.files.iter().find(|f| f.path == "small.txt").unwrap();
    assert!(small.error.is_none() && !small.hunks.is_empty());

    drop(seats);
    let got = repo.diff(None, commit("HEAD"));
    let six = got.files.iter().find(|f| f.path == "six.txt").unwrap();
    assert!(
        six.error.is_none() && six.hunks.len() == 1,
        "{:?}",
        six.error
    );
}

/// 객체 하나가 손상되면 그 파일에만 오류가 달리고 나머지는 나온다 — 헤더부터 못 읽는 객체(`a.txt`)도, 헤더는 읽히고 내용이
/// 잘린 객체(`b.txt`)도.
#[test]
fn a_corrupt_object_fails_only_its_own_file() {
    let r = TestRepo::with_commit();
    // 잘라도 헤더가 남게 압축이 잘 안 되는 내용(64 KiB).
    let mut seed: u64 = 7;
    let noise: String = (0..64 * 1024)
        .map(|_| {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            char::from(b'a' + ((seed >> 33) % 26) as u8)
        })
        .collect();
    r.write("a.txt", "a\n");
    r.write("b.txt", &noise);
    r.commit_all("a");
    r.write("README.md", "hello\nstaged\n");
    r.write("a.txt", "a\nstaged\n");
    r.write("b.txt", &(noise.clone() + "staged\n"));
    r.git(&["add", "README.md", "a.txt", "b.txt"]);
    let object = |path: &str| {
        let id = r.rev_parse(&format!(":{path}"));
        let object = r.root().join(".git/objects").join(&id[..2]).join(&id[2..]);
        let mut permissions = fs::metadata(&object).unwrap().permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        // 시험이 만든 임시 저장소의 객체 파일이다.
        permissions.set_readonly(false);
        fs::set_permissions(&object, permissions).unwrap();
        object
    };
    fs::write(object("a.txt"), b"not a zlib stream").unwrap();
    let b = object("b.txt");
    let whole = fs::read(&b).unwrap();
    fs::write(&b, &whole[..whole.len() / 2]).unwrap();
    let repo = Repo::new(r);

    let got = repo.diff(None, DiffSide::Index);
    for path in ["a.txt", "b.txt"] {
        let f = got.files.iter().find(|f| f.path == path).unwrap();
        assert!(f.error.is_some() && f.hunks.is_empty(), "{f:?}");
    }
    let readme = got.files.iter().find(|f| f.path == "README.md").unwrap();
    assert!(readme.error.is_none() && !readme.hunks.is_empty());
}
