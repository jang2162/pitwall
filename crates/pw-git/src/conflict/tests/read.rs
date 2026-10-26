//! `git.conflicts` · `git.conflict.get`: 갈래가 `git status --porcelain` 의 두 글자와, 단계가 `git ls-files -s` 와 같다. 병합할 수
//! 있는 충돌마다 구간이 그 세 단계를 `git merge-file -p --diff3` 한 것과 같다. 병합할 수 없는 쪽(삭제 · 바이너리 · 서브모듈 ·
//! 심링크 · 대용량)은 결과가 없고, 각 쪽의 내용은 대용량 기준(5 MiB · 50 MiB)을 따른다.

use pw_fs::limits::{EDITABLE_MAX, READ_ONLY_MAX};
use pw_fs::methods::FsContent;
use pw_rpc::{RpcError, code};

use super::*;
use crate::conflict::methods::{
    ConflictSide, GitConflictDetail, GitConflictParams, GitConflicts, GitConflictsParams,
    MergeRegionKind,
};
use crate::status::GitConflict;
use crate::testing::conflict::{
    BOTH_BASE, BOTH_OURS, BOTH_THEIRS, KINDS, SUB_BASE, SUB_OURS, SUB_THEIRS,
};

impl Host {
    fn conflicts(&self) -> GitConflicts {
        self.git
            .conflicts(&GitConflictsParams {
                project_id: self.project.clone(),
                worktree: None,
            })
            .unwrap()
    }

    fn get(&self, path: &str) -> Result<GitConflictDetail, RpcError> {
        self.git.conflict(&GitConflictParams {
            project_id: self.project.clone(),
            worktree: None,
            path: path.to_owned(),
        })
    }
}

fn kinds() -> Host {
    let r = TestRepo::with_commit();
    r.start_conflict_kinds();
    Host::new(r)
}

fn letters(kind: GitConflict) -> &'static str {
    match kind {
        GitConflict::BothModified => "UU",
        GitConflict::BothAdded => "AA",
        GitConflict::DeletedByUs => "DU",
        GitConflict::DeletedByThem => "UD",
        GitConflict::AddedByUs => "AU",
        GitConflict::AddedByThem => "UA",
        GitConflict::BothDeleted => "DD",
    }
}

fn text(side: Option<&ConflictSide>) -> Option<&str> {
    match side?.content.as_ref()? {
        FsContent::Text { text } => Some(text),
        FsContent::Base64 { .. } => None,
    }
}

#[test]
fn every_kind_is_listed_like_git_status_with_the_index_stages() {
    let h = kinds();
    let list = h.conflicts();
    assert_eq!(list.work_tree, h.repo.root().display().to_string());
    let ours: Vec<(String, String)> = list
        .files
        .iter()
        .map(|f| (f.path.clone(), letters(f.kind).to_owned()))
        .collect();
    let mut git: Vec<(String, String)> = h
        .repo
        .git(&["status", "--porcelain=v1", "-z"])
        .split('\0')
        .filter_map(|e| {
            let (xy, path) = (e.get(..2)?, e.get(3..)?);
            ["DD", "AU", "UD", "UA", "DU", "AA", "UU"]
                .contains(&xy)
                .then(|| (path.to_owned(), xy.to_owned()))
        })
        .collect();
    git.sort();
    assert_eq!(ours, git, "kinds are git status's two letters");
    let expected: Vec<(String, String)> = KINDS
        .iter()
        .map(|(p, k)| ((*p).to_owned(), (*k).to_owned()))
        .collect();
    assert_eq!(ours, expected, "the fixture makes every kind");

    // 단계: `git ls-files -s` 의 (경로, 단계, 모드, 해시).
    let mut stages: Vec<(String, u8, String, String)> = Vec::new();
    for f in &list.files {
        for (n, s) in [(1, &f.base), (2, &f.ours), (3, &f.theirs)] {
            if let Some(s) = s {
                stages.push((f.path.clone(), n, s.mode.clone(), s.id.clone()));
            }
        }
    }
    let mut git: Vec<(String, u8, String, String)> = h
        .repo
        .git(&["ls-files", "-s", "-z"])
        .split('\0')
        .filter(|e| !e.is_empty())
        .filter_map(|e| {
            let (meta, path) = e.split_once('\t')?;
            let mut meta = meta.split(' ');
            let (mode, id, stage) = (meta.next()?, meta.next()?, meta.next()?);
            (stage != "0").then(|| {
                (
                    path.to_owned(),
                    stage.parse().unwrap(),
                    mode.to_owned(),
                    id.to_owned(),
                )
            })
        })
        .collect();
    git.sort();
    stages.sort();
    assert_eq!(stages, git);

    let flagged = |pick: fn(&crate::conflict::methods::ConflictFile) -> bool| -> Vec<&str> {
        list.files
            .iter()
            .filter(|f| pick(f))
            .map(|f| f.path.as_str())
            .collect()
    };
    assert_eq!(flagged(|f| f.binary), ["image.bin"]);
    assert_eq!(flagged(|f| f.submodule), ["sub"]);
    #[cfg(unix)]
    assert_eq!(flagged(|f| f.symlink), ["link"]);
    assert_eq!(flagged(|f| f.too_large), Vec::<&str>::new());
}

/// 병합할 수 있는 충돌마다 구간이 그 세 단계를 `git merge-file -p --diff3` 한 것과 같다.
#[test]
fn merge_results_have_the_regions_of_git_merge_file_on_the_stages() {
    let h = kinds();
    let tmp = tempfile::tempdir().unwrap();
    let mut merged = Vec::new();
    for f in h.conflicts().files {
        let c = h.get(&f.path).unwrap();
        assert_eq!(c.file, f, "get carries the list entry");
        let Some(result) = &c.merged else {
            assert!(c.regions.is_empty());
            continue;
        };
        merged.push(f.path.clone());
        let stage = |n: u8, present: bool| {
            if present {
                h.repo.git(&["show", &format!(":{n}:{}", f.path)])
            } else {
                String::new()
            }
        };
        let (b, o, t) = (stage(1, f.base.is_some()), stage(2, true), stage(3, true));
        let regions: Vec<_> = c
            .regions
            .iter()
            .map(|r| {
                let kind = match r.kind {
                    MergeRegionKind::Ours => Kind::Ours,
                    MergeRegionKind::Theirs => Kind::Theirs,
                    MergeRegionKind::Conflict => Kind::Conflict,
                };
                let range = |l: &crate::conflict::methods::MergeLines| l.start..l.start + l.count;
                (
                    kind,
                    [
                        range(&r.base),
                        range(&r.ours),
                        range(&r.theirs),
                        range(&r.merged),
                    ],
                )
            })
            .collect();
        assert_eq!(
            sections(
                result.as_bytes(),
                &regions,
                b.as_bytes(),
                o.as_bytes(),
                t.as_bytes()
            ),
            merge_file(tmp.path(), b.as_bytes(), o.as_bytes(), t.as_bytes()),
            "{}",
            f.path
        );
    }
    assert_eq!(merged, ["added.txt", "both.txt"]);

    let both = h.get("both.txt").unwrap();
    assert_eq!(text(both.base.as_ref()), Some(BOTH_BASE));
    assert_eq!(text(both.ours.as_ref()), Some(BOTH_OURS));
    assert_eq!(text(both.theirs.as_ref()), Some(BOTH_THEIRS));
    assert_eq!(
        both.merged.as_deref(),
        Some("1\n3\n4\n5\n6 theirs\n7\n8\n9 ours\n10\n")
    );
    let kinds: Vec<(MergeRegionKind, u32, u32)> = both
        .regions
        .iter()
        .map(|r| (r.kind, r.merged.start, r.merged.count))
        .collect();
    assert_eq!(
        kinds,
        [
            (MergeRegionKind::Conflict, 1, 0),
            (MergeRegionKind::Theirs, 4, 1),
            (MergeRegionKind::Ours, 7, 1)
        ]
    );
    // 양쪽 추가: 빈 조상 — 파일 전체가 충돌이다.
    let added = h.get("added.txt").unwrap();
    assert_eq!(added.base, None);
    assert_eq!(added.merged.as_deref(), Some(""));
    assert_eq!(added.regions.len(), 1);
    assert_eq!(added.regions[0].kind, MergeRegionKind::Conflict);
}

#[test]
fn sides_that_cannot_merge_give_their_stages_and_no_result() {
    let h = kinds();
    let du = h.get("deleted-by-us.txt").unwrap();
    assert_eq!(du.file.kind, GitConflict::DeletedByUs);
    assert_eq!(
        (
            text(du.base.as_ref()),
            du.ours.is_none(),
            text(du.theirs.as_ref())
        ),
        (Some("keep\n"), true, Some("keep theirs\n"))
    );
    assert_eq!((du.merged, du.regions.len()), (None, 0));

    let ud = h.get("deleted-by-them.txt").unwrap();
    assert_eq!(
        (text(ud.ours.as_ref()), ud.theirs.is_none(), ud.merged),
        (Some("keep ours\n"), true, None)
    );

    let bin = h.get("image.bin").unwrap();
    assert!(bin.file.binary);
    for side in [&bin.base, &bin.ours, &bin.theirs] {
        let side = side.as_ref().unwrap();
        assert_eq!(side.content, None, "binary content is not sent");
        assert!(side.judgment.as_ref().unwrap().binary);
    }
    assert_eq!(bin.merged, None);

    let sub = h.get("sub").unwrap();
    assert!(sub.file.submodule);
    let ids: Vec<(&str, &str, u64, bool)> = [&sub.base, &sub.ours, &sub.theirs]
        .into_iter()
        .map(|s| {
            let s = s.as_ref().unwrap();
            (
                s.id.as_str(),
                s.mode.as_str(),
                s.size,
                s.content.is_none() && s.judgment.is_none(),
            )
        })
        .collect();
    assert_eq!(
        ids,
        [
            (SUB_BASE, "160000", 0, true),
            (SUB_OURS, "160000", 0, true),
            (SUB_THEIRS, "160000", 0, true)
        ]
    );
    assert_eq!(sub.merged, None);

    #[cfg(unix)]
    {
        let link = h.get("link").unwrap();
        assert!(link.file.symlink);
        assert_eq!(link.ours.as_ref().unwrap().mode, "120000");
        assert_eq!(text(link.ours.as_ref()), Some("target-ours"));
        assert_eq!(link.merged, None, "a symlink is picked, not merged");
    }

    let dd = h.get("old.txt").unwrap();
    assert_eq!(dd.file.kind, GitConflict::BothDeleted);
    assert_eq!((dd.ours.is_none(), dd.theirs.is_none()), (true, true));
    let au = h.get("renamed-ours.txt").unwrap();
    assert_eq!(
        (
            au.file.kind,
            au.base.is_none(),
            au.ours.is_some(),
            au.theirs.is_none()
        ),
        (GitConflict::AddedByUs, true, true, true)
    );
}

/// 5 MiB 까지는 병합 · 편집, 그 위 50 MiB 까지는 내용만(읽기 전용 — 병합 결과 없음), 그 위는 내용도 없다(결정 M1-D9).
#[test]
fn side_contents_follow_the_large_file_limits() {
    let r = TestRepo::with_commit();
    let big = |size: u64| -> Vec<u8> {
        let mut data = b"line\n".repeat(size as usize / 5 + 1);
        data.truncate(size as usize);
        *data.last_mut().unwrap() = b'\n';
        data
    };
    r.stage_conflict(
        "edit.txt",
        Some(b"x\n"),
        Some(&big(EDITABLE_MAX)),
        Some(b"y\n"),
    );
    r.stage_conflict(
        "read-only.txt",
        Some(b"x\n"),
        Some(&big(EDITABLE_MAX + 1)),
        Some(b"y\n"),
    );
    r.stage_conflict(
        "huge.txt",
        Some(b"x\n"),
        Some(b"z\n"),
        Some(&big(READ_ONLY_MAX + 1)),
    );
    let h = Host::new(r);

    let edit = h.get("edit.txt").unwrap();
    assert!(
        edit.ours
            .as_ref()
            .unwrap()
            .judgment
            .as_ref()
            .unwrap()
            .editable
    );
    assert!(edit.merged.is_some(), "an editable side merges");

    let read_only = h.get("read-only.txt").unwrap();
    let ours = read_only.ours.as_ref().unwrap();
    assert_eq!(ours.size, EDITABLE_MAX + 1);
    assert!(!ours.judgment.as_ref().unwrap().editable);
    assert_eq!(
        text(Some(ours)).map(str::len),
        Some((EDITABLE_MAX + 1) as usize)
    );
    assert_eq!((read_only.merged, read_only.regions.len()), (None, 0));

    let huge = h.get("huge.txt").unwrap();
    let theirs = huge.theirs.as_ref().unwrap();
    assert_eq!(
        (
            theirs.size,
            theirs.too_large,
            theirs.content.is_none(),
            theirs.judgment.is_none()
        ),
        (READ_ONLY_MAX + 1, true, true, true)
    );
    assert!(huge.file.too_large);
    assert_eq!(huge.merged, None);
    let flags: Vec<(String, bool)> = h
        .conflicts()
        .files
        .into_iter()
        .map(|f| (f.path, f.too_large))
        .collect();
    assert_eq!(
        flags,
        [
            ("edit.txt".to_owned(), false),
            ("huge.txt".to_owned(), true),
            ("read-only.txt".to_owned(), false)
        ]
    );
}

#[test]
fn byte_order_marks_are_set_aside_before_merging() {
    let r = TestRepo::with_commit();
    r.stage_conflict(
        "bom.txt",
        Some("\u{feff}a\nb\n".as_bytes()),
        Some("\u{feff}a\nb ours\n".as_bytes()),
        Some("\u{feff}a\nb theirs\n".as_bytes()),
    );
    let h = Host::new(r);
    let c = h.get("bom.txt").unwrap();
    assert_eq!(c.merged.as_deref(), Some("a\n"));
    assert_eq!(c.regions.len(), 1);
    assert_eq!(text(c.ours.as_ref()), Some("a\nb ours\n"));
    for side in [&c.base, &c.ours, &c.theirs] {
        assert!(
            side.as_ref()
                .unwrap()
                .judgment
                .as_ref()
                .unwrap()
                .bom
                .is_some()
        );
    }
}

#[test]
fn paths_are_literal_relative_and_must_be_in_conflict() {
    let r = TestRepo::with_commit();
    r.stage_conflict("dir/한글 [x].txt", Some(b"a\n"), Some(b"o\n"), Some(b"t\n"));
    r.stage_conflict("dir/한글 x.txt", Some(b"a\n"), Some(b"o\n"), Some(b"t\n"));
    let h = Host::new(r);
    let paths: Vec<String> = h.conflicts().files.into_iter().map(|f| f.path).collect();
    assert_eq!(paths, ["dir/한글 [x].txt", "dir/한글 x.txt"]);
    assert_eq!(
        h.get("dir/한글 [x].txt").unwrap().file.path,
        "dir/한글 [x].txt"
    );

    let err = h.get("README.md").unwrap_err();
    assert_eq!(err.code, code::NOT_FOUND, "{err:?}");
    for bad in [
        "",
        "/abs",
        "../x",
        "dir/../README.md",
        "a//b",
        "./a",
        "dir\\x",
    ] {
        let err = h.get(bad).unwrap_err();
        assert_eq!(err.code, code::INVALID_PARAMS, "{bad}: {err:?}");
    }
}

#[test]
fn a_project_without_a_repository_is_invalid_params() {
    let h = Host::new(TestRepo::with_commit());
    std::fs::remove_dir_all(h.repo.root().join(".git")).unwrap();
    let err = h
        .git
        .conflicts(&GitConflictsParams {
            project_id: h.project.clone(),
            worktree: None,
        })
        .unwrap_err();
    assert_eq!(err.code, code::INVALID_PARAMS, "{err:?}");
}
