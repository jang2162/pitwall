//! 경로가 `git worktree add` 로 만든 연결 워크트리 안에 있는지, 그렇다면 메인 체크아웃의 어디에 대응하는지.
//!
//! `git` 프로세스를 띄우지 않고 파일만 읽는다. 연결 워크트리의 꼭대기에는 디렉터리가 아니라 **파일** `.git` 이 있고
//! (`gitdir: <메인>/.git/worktrees/<이름>`), 그 gitdir 안의 `commondir` 이 메인 저장소의 `.git` 을 가리킨다.
//! 서브모듈 · `git init/clone --separate-git-dir` 저장소도 `.git` 이 파일이지만 gitdir 에 `commondir` 이 없으므로 워크트리로 보지
//! 않는다. 그런 gitdir 은 두 경우만 받지 않는다 — 다른 작업 트리의 내장 `.git`(이름이 `.git` 이고 부모가 이 폴더가 아니다)이거나, 그
//! `config` 의 `core.worktree` 가 이 폴더가 아닌 곳을 가리킨다(서브모듈이면 git 이 이 폴더를 되가리키게 적는다). 아니면 남의
//! 저장소의 `.git` 을 직접 가리키는 `.git` 파일 하나로 그 저장소가 아무 폴더의 저장소가 된다. 그 밖의 gitdir(separate-git-dir 의
//! 저장소 폴더)은 git 이 되돌림을 남기지 않아 남의 것인지 가를 수 없다 — 받되 이 폴더 자신을 메인 루트로 하는 저장소다(워크트리가
//! 아니라 다른 프로젝트의 신뢰를 물려받지 않는다).
//! gitdir 의 `gitdir` 파일(git 이 워크트리마다 적는 되돌림 포인터)이 그 `.git` 파일을 가리켜야 한다 — 다른 워크트리의 관리 폴더를
//! 가리키는 `.git` 파일 하나로 아무 폴더나 워크트리(파일 RPC 의 접근 루트)가 되지 않게.
//! gitdir 은 실제 경로로 **`<commondir>/worktrees/<이름>` 바로 아래**여야 한다 — git 은 관리 폴더를 언제나 그 자리에 만든다.
//! 아니면 gitdir · `commondir` · 되돌림 포인터를 모두 제 폴더 안에 둔 위조 `.git` 하나로 아무 폴더나 남의 저장소의 워크트리가
//! 되어 그 프로젝트의 신뢰(런타임 제공자 실행, `registry::trust`)와 파일 루트를 물려받는다. 이 검사가 있으면 위조하려면 그
//! 저장소의 `.git` 안에 써야 하고, 그럴 수 있으면 이미 그 저장소를 가진 것이다.

use std::fs;
use std::path::{Path, PathBuf};

/// 연결 워크트리와 메인 체크아웃의 대응.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeLink {
    /// `.git` 파일이 있는 워크트리 꼭대기.
    pub worktree_top: PathBuf,
    /// 메인 체크아웃 꼭대기(`commondir` 이 가리키는 `.git` 의 부모).
    pub main_top: PathBuf,
}

impl WorktreeLink {
    /// 워크트리 안의 `path` 에 대응하는 메인 체크아웃 쪽 경로. `path` 가 워크트리 밖이면 `None`.
    pub fn map_to_main(&self, path: &Path) -> Option<PathBuf> {
        let rel = path.strip_prefix(&self.worktree_top).ok()?;
        Some(self.main_top.join(rel))
    }
}

/// `start` 와 그 조상에서 가장 가까운 `.git` 을 찾는다. 그것이 연결 워크트리의 `.git` 파일이면 대응을 돌려준다.
/// 일반 체크아웃(`.git` 이 디렉터리), 서브모듈, bare 저장소의 워크트리, 저장소 밖이면 `None`.
pub fn linked_worktree(start: &Path) -> Option<WorktreeLink> {
    for dir in start.ancestors() {
        let Ok(meta) = fs::metadata(dir.join(".git")) else {
            continue;
        };
        if !meta.is_file() {
            return None;
        }
        let GitFile::Worktree { common, .. } = git_file(dir) else {
            return None;
        };
        if common.file_name()? != ".git" {
            // bare 저장소에 붙은 워크트리 — 메인 체크아웃이 없다.
            return None;
        }
        return Some(WorktreeLink {
            worktree_top: dir.to_path_buf(),
            main_top: common.parent()?.to_path_buf(),
        });
    }
    None
}

/// 파일 `.git` 이 가리키는 관리 폴더의 판정([`git_file`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitFile {
    /// 연결 워크트리. 관리 폴더가 실제 경로로 `<common>/worktrees/<이름>` 바로 아래이고, 그 되돌림 포인터가 이 `.git` 을
    /// 가리킨다. bare 저장소의 워크트리도 여기다(`common` 이 `.git` 이라는 이름이 아니다). 두 경로는 실제 경로다.
    Worktree { gitdir: PathBuf, common: PathBuf },
    /// 관리 폴더에 `commondir` 이 없는 저장소: 서브모듈(그 `config` 의 `core.worktree` 가 이 폴더를 가리킨다 — git 이 서브모듈
    /// 관리 폴더에 적는 되돌림)이나 `--separate-git-dir` 저장소(`core.worktree` 가 없다). 워크트리가 아니다.
    NotWorktree,
    /// `.git` 을 읽지 못했거나, 위 두 경우의 조건을 어긴다(위조 — 모듈 설명). 다른 작업 트리의 내장 `.git` 을 직접 가리키는 `.git`
    /// 파일, `core.worktree` 가 다른 폴더를 가리키는 관리 폴더를 가리키는 `.git` 파일도 여기다.
    Invalid,
}

/// `dir/.git`(파일)이 가리키는 관리 폴더를 판정한다. [`linked_worktree`] 와 같은 검사지만 메인 체크아웃이 없는 bare 저장소의
/// 워크트리도 [`GitFile::Worktree`] 로 받는다 — 메인 체크아웃이 아니라 저장소를 찾는 쪽(`pw-git` 의 저장소 해석)이 쓴다.
pub fn git_file(dir: &Path) -> GitFile {
    let dot_git = dir.join(".git");
    let Some(pointer) = read_pointer(&dot_git, "gitdir:") else {
        return GitFile::Invalid;
    };
    let gitdir = resolve_from(dir, pointer);
    let Ok(common) = fs::read_to_string(gitdir.join("commondir")) else {
        return own_repository(&gitdir, dir);
    };
    if !points_back(&gitdir, &dot_git) {
        return GitFile::Invalid;
    }
    let (Ok(common), Ok(gitdir)) = (
        fs::canonicalize(resolve_from(&gitdir, common.trim())),
        fs::canonicalize(&gitdir),
    ) else {
        return GitFile::Invalid;
    };
    if gitdir.parent() != Some(common.join("worktrees").as_path()) {
        return GitFile::Invalid;
    }
    GitFile::Worktree { gitdir, common }
}

/// `commondir` 이 없는 관리 폴더 `gitdir` 을 `dir` 의 저장소로 받는가(모듈 설명): 다른 작업 트리의 내장 `.git` 이 아니고, 그 `config`
/// 의 `core.worktree` 가 없거나 실제 경로로 `dir` 이다.
fn own_repository(gitdir: &Path, dir: &Path) -> GitFile {
    let (Ok(gitdir), Ok(dir)) = (fs::canonicalize(gitdir), fs::canonicalize(dir)) else {
        return GitFile::Invalid;
    };
    // 이름은 대소문자를 가리지 않고 본다 — 대소문자를 가리지 않는 파일 시스템의 `.GIT` 도 git 의 내장 `.git` 이다.
    let embedded = gitdir
        .file_name()
        .is_some_and(|name| name.eq_ignore_ascii_case(".git"));
    if embedded && gitdir.parent() != Some(dir.as_path()) {
        return GitFile::Invalid;
    }
    let Ok(config) = fs::read_to_string(gitdir.join("config")) else {
        return GitFile::Invalid;
    };
    match core_worktree(&config) {
        Ok(None) => GitFile::NotWorktree,
        Ok(Some(worktree)) => match fs::canonicalize(resolve_from(&gitdir, worktree)) {
            Ok(worktree) if worktree == dir => GitFile::NotWorktree,
            _ => GitFile::Invalid,
        },
        Err(Unreadable) => GitFile::Invalid,
    }
}

/// [`core_worktree`] 가 읽지 않는 문법을 만났다.
#[derive(Debug, PartialEq, Eq)]
struct Unreadable;

/// git config 텍스트에서 `[core]` 의 `worktree` 마지막 값(없으면 `None`). git 이 서브모듈 관리 폴더에 쓰는 모양
/// (`\tworktree = ../../../m`)과 따옴표 · 줄 끝 주석까지만 읽는다. 그 밖의 문법(이스케이프, 줄 이음, 같은 줄의 섹션과 값)이 보이면
/// [`Unreadable`] — 값이 있는지 모르므로 받지 않는 쪽으로 틀린다.
fn core_worktree(config: &str) -> Result<Option<String>, Unreadable> {
    let mut in_core = false;
    let mut found = None;
    for line in config.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix('[') {
            let (name, after) = rest.split_once(']').ok_or(Unreadable)?;
            if !after.trim().is_empty() && !after.trim().starts_with(['#', ';']) {
                return Err(Unreadable);
            }
            in_core = name.trim().eq_ignore_ascii_case("core");
            continue;
        }
        if !in_core {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            // 값 없는 키(`worktree` 만)는 git 이 오류로 본다.
            if line.eq_ignore_ascii_case("worktree") {
                return Err(Unreadable);
            }
            continue;
        };
        if key.trim().eq_ignore_ascii_case("worktree") {
            found = Some(config_value(value.trim()).ok_or(Unreadable)?);
        }
    }
    Ok(found)
}

/// 값 하나: `"..."` 전체를 따옴표로 감싼 것이거나, 따옴표 없이 줄 끝 주석(`#` · `;`) 앞까지.
fn config_value(raw: &str) -> Option<String> {
    if raw.contains('\\') {
        return None;
    }
    if let Some(rest) = raw.strip_prefix('"') {
        let (value, after) = rest.split_once('"')?;
        let after = after.trim();
        return (after.is_empty() || after.starts_with(['#', ';'])).then(|| value.to_owned());
    }
    if raw.contains('"') {
        return None;
    }
    let value = raw.split(['#', ';']).next().unwrap_or("").trim();
    (!value.is_empty()).then(|| value.to_owned())
}

/// `<gitdir>/gitdir` 이 `dot_git` 을 가리키는가. 내용은 그 워크트리의 `.git` 파일 경로다(`worktree.useRelativePaths` 면 gitdir 기준
/// 상대 경로). 실제 경로로 비교한다.
fn points_back(gitdir: &Path, dot_git: &Path) -> bool {
    let Ok(text) = fs::read_to_string(gitdir.join("gitdir")) else {
        return false;
    };
    let back = resolve_from(gitdir, text.trim());
    match (fs::canonicalize(back), fs::canonicalize(dot_git)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// `gitdir: <경로>` 형식 파일에서 경로를 읽는다.
fn read_pointer(file: &Path, prefix: &str) -> Option<String> {
    let text = fs::read_to_string(file).ok()?;
    let line = text.lines().next()?;
    Some(line.strip_prefix(prefix)?.trim().to_owned())
}

fn resolve_from(base: &Path, path: impl AsRef<Path>) -> PathBuf {
    let path = path.as_ref();
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// `git worktree add` 가 만드는 것과 같은 모양을 손으로 만든다(테스트가 git 실행 파일에 의존하지 않게).
    pub(crate) fn fake_repo(root: &Path) -> (PathBuf, PathBuf) {
        let main = root.join("main");
        let wt = root.join("wt");
        let admin = main.join(".git/worktrees/wt");
        fs::create_dir_all(&admin).unwrap();
        fs::create_dir_all(wt.join("sub/dir")).unwrap();
        fs::write(admin.join("commondir"), "../..\n").unwrap();
        fs::write(wt.join(".git"), format!("gitdir: {}\n", admin.display())).unwrap();
        fs::write(
            admin.join("gitdir"),
            format!("{}\n", wt.join(".git").display()),
        )
        .unwrap();
        (fs::canonicalize(main).unwrap(), wt)
    }

    #[test]
    fn linked_worktree_maps_to_main_checkout() {
        let tmp = tempfile::tempdir().unwrap();
        let (main, wt) = fake_repo(tmp.path());
        let link = linked_worktree(&wt.join("sub/dir")).expect("worktree");
        assert_eq!(link.worktree_top, wt);
        assert_eq!(link.main_top, main);
        assert_eq!(link.map_to_main(&wt.join("sub")).unwrap(), main.join("sub"));
        // 메인 체크아웃 자신은 워크트리가 아니다.
        assert_eq!(linked_worktree(&main), None);
    }

    #[test]
    fn relative_gitdir_is_resolved_from_the_worktree() {
        let tmp = tempfile::tempdir().unwrap();
        let (main, wt) = fake_repo(tmp.path());
        fs::write(wt.join(".git"), "gitdir: ../main/.git/worktrees/wt\n").unwrap();
        assert_eq!(linked_worktree(&wt).unwrap().main_top, main);
    }

    /// bare 저장소(`repo.git`)에 붙은 워크트리에는 메인 체크아웃이 없다.
    #[test]
    fn worktree_of_a_bare_repository_has_no_main_checkout() {
        let tmp = tempfile::tempdir().unwrap();
        let admin = tmp.path().join("repo.git/worktrees/wt");
        fs::create_dir_all(&admin).unwrap();
        fs::write(admin.join("commondir"), "../..\n").unwrap();
        let wt = tmp.path().join("wt");
        fs::create_dir_all(&wt).unwrap();
        fs::write(wt.join(".git"), format!("gitdir: {}\n", admin.display())).unwrap();
        fs::write(
            admin.join("gitdir"),
            format!("{}\n", wt.join(".git").display()),
        )
        .unwrap();
        assert_eq!(linked_worktree(&wt), None);
        // 저장소를 찾는 쪽에는 워크트리다.
        assert!(
            matches!(git_file(&wt), GitFile::Worktree { common, .. } if common.ends_with("repo.git")),
            "{:?}",
            git_file(&wt)
        );
    }

    /// [`git_file`] 은 서브모듈을 워크트리가 아닌 것으로, 위조 · 읽을 수 없는 `.git` 을 잘못된 것으로 가른다.
    #[test]
    fn git_file_tells_worktrees_submodules_and_forgeries_apart() {
        let tmp = tempfile::tempdir().unwrap();
        let (main, wt) = fake_repo(tmp.path());
        assert_eq!(
            git_file(&wt),
            GitFile::Worktree {
                gitdir: main.join(".git/worktrees/wt"),
                common: main.join(".git"),
            }
        );
        let sub_admin = main.join(".git/modules/m");
        fs::create_dir_all(&sub_admin).unwrap();
        let module = main.join("m");
        fs::create_dir_all(&module).unwrap();
        fs::write(
            module.join(".git"),
            format!("gitdir: {}\n", sub_admin.display()),
        )
        .unwrap();
        // 관리 폴더에 `config` 가 없으면(저장소가 아니다) 받지 않는다.
        assert_eq!(git_file(&module), GitFile::Invalid);
        fs::write(
            sub_admin.join("config"),
            "[core]\n\trepositoryformatversion = 0\n\tbare = false\n\tworktree = ../../../m\n",
        )
        .unwrap();
        assert_eq!(git_file(&module), GitFile::NotWorktree);
        let evil = tmp.path().join("evil");
        fs::create_dir_all(evil.join(".admin")).unwrap();
        fs::write(evil.join(".git"), "gitdir: .admin\n").unwrap();
        fs::write(evil.join(".admin/gitdir"), "../.git\n").unwrap();
        fs::write(
            evil.join(".admin/commondir"),
            format!("{}\n", main.join(".git").display()),
        )
        .unwrap();
        assert_eq!(git_file(&evil), GitFile::Invalid);
        fs::write(evil.join(".git"), "not a pointer\n").unwrap();
        assert_eq!(git_file(&evil), GitFile::Invalid);
    }

    /// 다른 작업 트리의 내장 `.git`(관리 폴더가 아니라 본체)을 직접 가리키는 `.git` 파일은 받지 않는다 — 그 `core.worktree` 가 이
    /// 폴더를 가리켜도 같다(git 은 서브모듈 관리 폴더를 `.git/modules/<이름>` 에 만든다).
    #[test]
    fn a_dot_git_file_pointing_straight_at_another_repository_is_invalid() {
        let tmp = tempfile::tempdir().unwrap();
        let (main, _wt) = fake_repo(tmp.path());
        fs::write(main.join(".git/config"), "[core]\n\tbare = false\n").unwrap();
        let evil = tmp.path().join("downloads/evil");
        fs::create_dir_all(&evil).unwrap();
        fs::write(
            evil.join(".git"),
            format!("gitdir: {}\n", main.join(".git").display()),
        )
        .unwrap();
        assert_eq!(git_file(&evil), GitFile::Invalid);
        // 그 저장소의 `core.worktree` 가 제 작업 트리를 가리켜도 이 폴더가 아니다.
        fs::write(
            main.join(".git/config"),
            format!("[core]\n\tworktree = {}\n", main.display()),
        )
        .unwrap();
        assert_eq!(git_file(&evil), GitFile::Invalid);
        fs::write(
            main.join(".git/config"),
            format!("[core]\n\tworktree = \"{}\" # moved\n", evil.display()),
        )
        .unwrap();
        assert_eq!(git_file(&evil), GitFile::Invalid);
        // 이름만 다른 대소문자(대소문자를 가리지 않는 파일 시스템)도 내장 `.git` 이다.
        let upper = tmp.path().join("upper");
        fs::create_dir_all(upper.join(".GIT")).unwrap();
        fs::write(upper.join(".GIT/config"), "[core]\n\tbare = false\n").unwrap();
        fs::write(
            evil.join(".git"),
            format!("gitdir: {}\n", upper.join(".GIT").display()),
        )
        .unwrap();
        assert_eq!(git_file(&evil), GitFile::Invalid);
    }

    /// `git init/clone --separate-git-dir` 저장소: 저장소 폴더에 `commondir` 도 `core.worktree` 도 없다. 이 폴더의 저장소로 받는다
    /// (워크트리가 아니다). 그 `core.worktree` 가 다른 폴더를 가리키거나 읽지 않는 문법이면 받지 않는다.
    #[test]
    fn a_separate_git_dir_is_the_repository_of_its_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let store = tmp.path().join("store/proj.git");
        let proj = tmp.path().join("proj");
        fs::create_dir_all(&store).unwrap();
        fs::create_dir_all(proj.join("sub")).unwrap();
        fs::write(store.join("config"), "[core]\n\tbare = false\n").unwrap();
        fs::write(proj.join(".git"), format!("gitdir: {}\n", store.display())).unwrap();
        assert_eq!(git_file(&proj), GitFile::NotWorktree);
        assert_eq!(linked_worktree(&proj.join("sub")), None);
        // git 은 Windows 에서도 `/` 로 적는다(이 판정은 `\` 가 든 값을 읽지 않는다).
        let slashed = |p: &Path| p.display().to_string().replace('\\', "/");
        for (config, want) in [
            (
                format!("[core]\n\tworktree = {}\n", slashed(&proj)),
                GitFile::NotWorktree,
            ),
            (
                "[core]\n\tworktree = ../../proj\n".to_owned(),
                GitFile::NotWorktree,
            ),
            (
                format!(
                    "[core]\n\tworktree = {}\n",
                    slashed(&tmp.path().join("store"))
                ),
                GitFile::Invalid,
            ),
            ("[core]\n\tworktree = a\\\nb\n".to_owned(), GitFile::Invalid),
            ("[core]\n\tworktree\n".to_owned(), GitFile::Invalid),
        ] {
            fs::write(store.join("config"), &config).unwrap();
            assert_eq!(git_file(&proj), want, "{config:?}");
        }
        // 관리 폴더에 `config` 가 없으면 저장소가 아니다.
        fs::remove_file(store.join("config")).unwrap();
        assert_eq!(git_file(&proj), GitFile::Invalid);
    }

    #[test]
    fn core_worktree_is_read_from_the_core_section_only() {
        let w = |s: &str| core_worktree(s).unwrap();
        assert_eq!(
            w("[core]\n\tworktree = ../../../m\n").as_deref(),
            Some("../../../m")
        );
        assert_eq!(w("[CORE]\nWorkTree=a ; c\n").as_deref(), Some("a"));
        assert_eq!(w("[core]\nworktree = \"a b\"\n").as_deref(), Some("a b"));
        // 마지막 값이 이긴다.
        assert_eq!(
            w("[core]\nworktree = a\n[core]\nworktree = b\n").as_deref(),
            Some("b")
        );
        // 다른 섹션 · 하위 섹션의 같은 키는 아니다.
        assert_eq!(w("[remote \"core\"]\nworktree = a\n"), None);
        assert_eq!(w("[other]\nworktree = a\n"), None);
        assert_eq!(w(""), None);
        // 읽지 않는 문법은 값이 있는지 모르는 것이다.
        for bad in [
            "[core] worktree = a\n",
            "[core]\nworktree = a\\\nb\n",
            "[core]\nworktree = a\"b\n",
            "[core]\nworktree = \"a\" b\n",
            "[core]\nworktree =\n",
            "[core]\nworktree\n",
        ] {
            assert_eq!(core_worktree(bad), Err(Unreadable), "{bad:?}");
        }
    }

    /// 다른 워크트리의 관리 폴더를 가리키는 `.git` 파일은 워크트리가 아니다(되돌림 포인터가 그 파일을 가리키지 않는다).
    #[test]
    fn a_dot_git_file_must_be_pointed_back_to() {
        let tmp = tempfile::tempdir().unwrap();
        let (main, wt) = fake_repo(tmp.path());
        let impostor = tmp.path().join("impostor");
        fs::create_dir_all(&impostor).unwrap();
        fs::write(
            impostor.join(".git"),
            format!("gitdir: {}\n", main.join(".git/worktrees/wt").display()),
        )
        .unwrap();
        assert_eq!(linked_worktree(&impostor), None);
        assert!(linked_worktree(&wt).is_some());
        // 상대 경로 되돌림 포인터(`worktree.useRelativePaths`)도 받는다.
        fs::write(
            main.join(".git/worktrees/wt/gitdir"),
            "../../../../wt/.git\n",
        )
        .unwrap();
        assert!(linked_worktree(&wt).is_some());
        // 되돌림 포인터가 없으면 워크트리로 보지 않는다.
        fs::remove_file(main.join(".git/worktrees/wt/gitdir")).unwrap();
        assert_eq!(linked_worktree(&wt), None);
    }

    /// gitdir · `commondir` · 되돌림 포인터를 모두 제 폴더 안에 둔 위조 `.git` 은 워크트리가 아니다 — 관리 폴더가 메인 저장소의
    /// `.git/worktrees/` 바로 아래가 아니다. `commondir` 이 절대 경로든 상대 경로든 같다.
    #[test]
    fn a_gitdir_outside_the_common_worktrees_folder_is_not_a_worktree() {
        let tmp = tempfile::tempdir().unwrap();
        let (main, wt) = fake_repo(tmp.path());
        let evil = tmp.path().join("downloads/evil");
        fs::create_dir_all(evil.join(".admin")).unwrap();
        fs::write(evil.join(".git"), "gitdir: .admin\n").unwrap();
        fs::write(evil.join(".admin/gitdir"), "../.git\n").unwrap();
        for common in [
            main.join(".git").display().to_string(),
            "../../../main/.git".to_owned(),
        ] {
            fs::write(evil.join(".admin/commondir"), format!("{common}\n")).unwrap();
            assert_eq!(linked_worktree(&evil), None, "commondir {common}");
        }
        // `worktrees/` 의 더 깊은 곳 · 다른 이름의 폴더도 git 이 만드는 자리가 아니다.
        for admin in ["worktrees/a/b", "worktrees-x/wt", "modules/wt"] {
            let dir = main.join(".git").join(admin);
            fs::create_dir_all(&dir).unwrap();
            let back = "../".repeat(admin.matches('/').count() + 3);
            fs::write(
                dir.join("commondir"),
                format!("{}..\n", "../".repeat(admin.matches('/').count())),
            )
            .unwrap();
            fs::write(dir.join("gitdir"), format!("{back}downloads/evil/.git\n")).unwrap();
            fs::write(evil.join(".git"), format!("gitdir: {}\n", dir.display())).unwrap();
            assert_eq!(linked_worktree(&evil), None, "{admin}");
        }
        assert!(linked_worktree(&wt).is_some());
    }

    #[test]
    fn submodule_and_plain_dirs_are_not_worktrees() {
        let tmp = tempfile::tempdir().unwrap();
        let sub_admin = tmp.path().join("super/.git/modules/m");
        fs::create_dir_all(&sub_admin).unwrap();
        let module = tmp.path().join("super/m");
        fs::create_dir_all(&module).unwrap();
        fs::write(
            module.join(".git"),
            format!("gitdir: {}\n", sub_admin.display()),
        )
        .unwrap();
        assert_eq!(linked_worktree(&module), None);
        assert_eq!(linked_worktree(tmp.path()), None);
    }
}
