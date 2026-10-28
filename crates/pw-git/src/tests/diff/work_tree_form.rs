//! `git.blob { workTreeForm: true }`(에디터 거터의 기준, 13-m2-plan M2-17): 내용이 `git checkout` 이 작업 트리에 쓰는 바이트와
//! 같고(`core.autocrlf` · `eol` · `text=auto` · `ident` · `working-tree-encoding`), 외부 필터 드라이버가 걸린 경로는 드라이버를
//! 돌리지 않고 `filter` 로 알린다 — 신뢰한 프로젝트에서도.

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use pw_fs::methods::FsContent;

use super::*;

/// 표준 base64 를 푼다(시험이 바이트로 견주려고).
fn unbase64(data: &str) -> Vec<u8> {
    let value = |c: u8| -> u32 {
        match c {
            b'A'..=b'Z' => u32::from(c - b'A'),
            b'a'..=b'z' => u32::from(c - b'a') + 26,
            b'0'..=b'9' => u32::from(c - b'0') + 52,
            b'+' => 62,
            b'/' => 63,
            _ => panic!("not base64: {c}"),
        }
    };
    let mut out = Vec::new();
    for chunk in data.as_bytes().chunks(4) {
        let digits: Vec<u8> = chunk.iter().copied().filter(|&c| c != b'=').collect();
        let mut n = 0u32;
        for (i, &c) in digits.iter().enumerate() {
            n |= value(c) << (18 - 6 * i);
        }
        let bytes = n.to_be_bytes();
        out.extend_from_slice(&bytes[1..digits.len()]);
    }
    out
}

fn work_tree_params(repo: &Repo, path: &str) -> GitBlobParams {
    GitBlobParams {
        work_tree_form: true,
        bytes: true,
        ..repo.blob_params(Some("HEAD"), path)
    }
}

/// HEAD 의 `path` 를 작업 트리 꼴로 받은 바이트.
fn ours(repo: &Repo, path: &str) -> Vec<u8> {
    let blob = repo.f.git.blob(&work_tree_params(repo, path)).unwrap();
    assert_eq!(blob.filter, None, "{blob:?}");
    match blob.content.unwrap() {
        FsContent::Base64 { data } => unbase64(&data),
        FsContent::Text { .. } => panic!("asked for bytes"),
    }
}

/// git 이 HEAD 의 `path` 를 작업 트리에 다시 쓴 바이트(지우고 `git checkout HEAD -- path`). 시험 도우미의 git 은
/// `-c core.autocrlf=false` 로 돌므로(`testing::repo::git_command`) 저장소 설정의 값을 다시 `-c` 로 준다.
fn checked_out(r: &TestRepo, path: &str) -> Vec<u8> {
    fs::remove_file(r.root().join(path)).unwrap();
    let autocrlf = r.git(&["config", "--local", "core.autocrlf"]);
    let autocrlf = format!("core.autocrlf={}", autocrlf.trim());
    r.git(&["-c", &autocrlf, "checkout", "HEAD", "--", path]);
    fs::read(r.root().join(path)).unwrap()
}

/// 내장 변환마다: 저장소 꼴(LF)로 담긴 파일을 git 이 작업 트리에 쓴 바이트와 우리의 `workTreeForm` 바이트가 같다. 견주는 바이트가
/// 저장소 꼴과 다른지도 본다 — 변환이 실제로 일어나는 픽스처다.
#[test]
fn work_tree_form_matches_git_checkout_byte_for_byte() {
    let r = TestRepo::with_commit();
    r.write(
        ".gitattributes",
        "eol-crlf.txt text eol=crlf\n\
         auto.txt text=auto\n\
         ident.txt ident\n\
         euckr.txt text working-tree-encoding=EUC-KR eol=crlf\n\
         raw.bin -text\n",
    );
    r.write("eol-crlf.txt", "one\ntwo\nno newline at end");
    r.write("auto.txt", "a\nb\n");
    r.write("ident.txt", "id: $Id$\nnext\n");
    r.write("raw.bin", "keep\nlf\n");
    r.write("plain.txt", "plain\nlines\n");
    // working-tree-encoding: git 이 add 할 때 EUC-KR 을 UTF-8 로 담고 checkout 이 EUC-KR 로 쓴다.
    fs::write(
        r.root().join("euckr.txt"),
        [0xC7, 0xD1, 0xB1, 0xDB, b'\n', b'o', b'k', b'\n'],
    )
    .unwrap();
    r.commit_all("attributes");
    // autocrlf 는 담은 뒤에 켠다 — 담긴 것은 LF 이고 checkout 이 CRLF 로 쓴다(Windows 의 흔한 설정).
    r.git(&["config", "core.autocrlf", "true"]);
    let repo = Repo::new(r);

    for path in [
        "eol-crlf.txt",
        "auto.txt",
        "ident.txt",
        "euckr.txt",
        "raw.bin",
        "plain.txt",
        "README.md",
    ] {
        let mine = ours(&repo, path);
        let git = checked_out(&repo.r, path);
        assert_eq!(
            String::from_utf8_lossy(&mine),
            String::from_utf8_lossy(&git),
            "{path}"
        );
        assert_eq!(mine, git, "{path}");
    }
    // 변환이 일어난 픽스처다: 담긴 내용(LF · UTF-8 · 펼치지 않은 `$Id$`)과 다르다. `-text` 는 그대로다.
    for (path, changed) in [
        ("eol-crlf.txt", true),
        ("ident.txt", true),
        ("euckr.txt", true),
        ("plain.txt", true),
        ("raw.bin", false),
    ] {
        let stored = repo.r.git(&["cat-file", "blob", &format!("HEAD:{path}")]);
        assert_eq!(ours(&repo, path) != stored.as_bytes(), changed, "{path}");
    }
    // 끝 개행 없는 줄은 끝 개행 없이 남는다.
    assert!(ours(&repo, "eol-crlf.txt").ends_with(b"two\r\nno newline at end"));
}

/// encoding_rs 가 git 과 같게 쓰지 못하는 인코딩(UTF-16 계열을 UTF-8 로 쓴다 · 모르는 이름)은 틀린 바이트 대신 invalid params 다.
#[test]
fn an_encoding_gix_cannot_write_like_git_is_refused() {
    let r = TestRepo::with_commit();
    r.write(
        ".gitattributes",
        "*.u16 working-tree-encoding=UTF-16LE\n*.bom working-tree-encoding=UTF-16LE-BOM\n",
    );
    for (path, bom) in [("a.u16", false), ("b.bom", true)] {
        let mut bytes = if bom { vec![0xFF, 0xFE] } else { vec![] };
        bytes.extend("x\n".encode_utf16().flat_map(u16::to_le_bytes));
        fs::write(r.root().join(path), bytes).unwrap();
    }
    r.commit_all("utf-16");
    let repo = Repo::new(r);
    for path in ["a.u16", "b.bom"] {
        let e = repo.f.git.blob(&work_tree_params(&repo, path)).unwrap_err();
        assert_eq!(e.code, pw_rpc::code::INVALID_PARAMS, "{path}: {e:?}");
    }
}

/// 바꾸지 않으면(기본) 객체 바이트 그대로다 — 기존 호출(diff 뷰)은 그대로다.
#[test]
fn without_work_tree_form_the_blob_is_the_stored_bytes() {
    let r = TestRepo::with_commit();
    r.commit_file("a.txt", "x\ny\n", "a");
    r.git(&["config", "core.autocrlf", "true"]);
    let repo = Repo::new(r);
    let blob = repo.blob(Some("HEAD"), "a.txt");
    assert_eq!(
        blob.content,
        Some(FsContent::Text {
            text: "x\ny\n".into()
        })
    );
    assert_eq!(blob.filter, None);
}

#[test]
fn work_tree_form_refuses_a_range_read() {
    let r = TestRepo::with_commit();
    let repo = Repo::new(r);
    let mut params = work_tree_params(&repo, "README.md");
    params.offset = Some(1);
    let e = repo.f.git.blob(&params).unwrap_err();
    assert_eq!(e.code, pw_rpc::code::INVALID_PARAMS, "{e:?}");
    params.offset = None;
    params.length = Some(2);
    assert!(repo.f.git.blob(&params).is_err());
}

/// 사람의 전역 설정에 smudge · process 드라이버(부르면 `marks/<이름>` 을 남긴다)를, 저장소 설정(`.git/config`)에 smudge 드라이버
/// 하나를 두고, 커밋된 `.gitattributes` 가 `*.lfs` · `*.proc` 에 전역의 것을, `*.loc` 에 저장소 설정의 것을, `*.none` 에 어디에도 없는
/// 드라이버를 건다. 미신뢰 프로젝트는 저장소 설정을 읽지 않는다(`repo::open_options`).
fn drivers() -> (Repo, PathBuf) {
    let r = TestRepo::with_commit();
    let config = isolate_global_config(r.base());
    let marks = r.base().join("marks");
    let bin = r.base().join("global");
    fs::create_dir_all(&marks).unwrap();
    fs::create_dir_all(&bin).unwrap();
    let script = |name: &str| {
        let path = bin.join(name);
        fs::write(
            &path,
            format!(
                "#!/bin/sh\necho \"$*\" >> '{}'\ncat\n",
                marks.join(name).display()
            ),
        )
        .unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path.display().to_string()
    };
    r.write(
        ".gitattributes",
        "*.lfs filter=lfs\n*.proc filter=proc\n*.loc filter=loc\n*.none filter=none\n",
    );
    r.write("big.lfs", "pointer\n");
    r.write("p.proc", "process\n");
    r.write("l.loc", "local\n");
    r.write("x.none", "no driver\n");
    r.write("plain.txt", "plain\n");
    // 담을 때는 드라이버가 없다(빈 전역 설정) — 담긴 것은 글 그대로다.
    r.commit_all("filtered");
    fs::write(
        &config,
        format!(
            "[filter \"lfs\"]\n\tclean = {}\n\tsmudge = {}\n\trequired = true\n[filter \"proc\"]\n\tprocess = {}\n",
            script("clean"),
            script("smudge"),
            script("process"),
        ),
    )
    .unwrap();
    let local = script("local");
    r.git(&["config", "filter.loc.smudge", &local]);
    (Repo::new(r), marks)
}

fn marks(dir: &Path) -> Vec<String> {
    fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect()
}

/// `filter` 속성이 가리키면 드라이버가 전역 설정에 있든 저장소 설정에만 있든(미신뢰는 그것을 읽지 않는다) 어디에도 없든 내용 없이
/// `filter` 로 알리고 드라이버를 띄우지 않는다 — 신뢰 · 미신뢰 같다. `filter` 속성이 없는 파일은 바꿔 준다.
#[test]
fn a_filter_attribute_is_reported_and_never_started_trusted_or_not() {
    let (repo, dir) = drivers();
    for trusted in [false, true] {
        repo.f.trust(&repo.id, trusted);
        for (path, driver) in [
            ("big.lfs", "lfs"),
            ("p.proc", "proc"),
            ("l.loc", "loc"),
            ("x.none", "none"),
        ] {
            let blob = repo.f.git.blob(&work_tree_params(&repo, path)).unwrap();
            assert_eq!(
                blob.filter.as_deref(),
                Some(driver),
                "{path} trusted={trusted}"
            );
            assert_eq!((blob.content, blob.judgment), (None, None), "{path}");
        }
        assert_eq!(ours(&repo, "plain.txt"), b"plain\n");
        assert_eq!(marks(&dir), Vec::<String>::new(), "trusted={trusted}");
    }
}
