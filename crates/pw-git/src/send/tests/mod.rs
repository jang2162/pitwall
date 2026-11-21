//! 커밋 보내기 시험(18-m7-plan.md M7-16 완료 기준). 로컬 저장소 둘 — 원본과 그것을 받은 대상 — 을 각각 쓰기 실행기 픽스처
//! (`crate::testing::write`, 진짜 git)로 두고 원격 없이 돈다. 두 픽스처의 PATH 맨 앞의 `git` 은 받은 argv 를 적고 진짜 git 을
//! 부르는 감싸개다 — 엔진이 실제로 부른 git 명령을 시험이 모은다.

mod bundle;
mod flow;
mod guard;
mod hardening;
mod receive;
mod stop;
mod wire;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use pw_rpc::Role;
use tokio_util::sync::CancellationToken;

use super::CHUNK;
use super::apply::{Apply, GitSendOutcome};
use super::methods::{GitSendTheirs, GitSendTheirsParams, theirs};
use super::prepare::{Prepared, prepare as make_bundle};
use super::receive::{Begin, Received, UploadOwner, Uploads};
use crate::testing::TestRepo;
use crate::testing::write::{Window, WriteFixture, which_git};
use crate::write::{Askpass, GitPromptAnswer, WriteError};

/// 시험의 원본 머신 id.
const MACHINE: &str = "laptop";

/// 원본 · 대상 한 쌍.
struct Pair {
    src: WriteFixture,
    dst: WriteFixture,
    /// 대상의 미저장 확인에 「진행」 으로 답하는 창.
    _window: Window,
}

impl Pair {
    /// 원본(첫 커밋 `README.md`)과 그것을 받은 대상. 둘 다 `trusted` 다.
    fn new(trusted: bool) -> Self {
        let src = TestRepo::with_commit();
        let dst = clone_of(&src);
        Self::of(src, dst, trusted)
    }

    fn of(src: TestRepo, dst: TestRepo, trusted: bool) -> Self {
        let src = WriteFixture::with_repo(src, trusted, Askpass::default());
        let dst = WriteFixture::with_repo(dst, trusted, Askpass::default());
        record_git(&src.bin);
        record_git(&dst.bin);
        let window = Window::open(&dst.write, &dst.project, |_| Some(GitPromptAnswer::Proceed));
        Self {
            src,
            dst,
            _window: window,
        }
    }

    async fn theirs(&self, branch: &str) -> GitSendTheirs {
        theirs(
            &self.dst.write,
            GitSendTheirsParams {
                project_id: self.dst.project.clone(),
                worktree: None,
                branch: branch.to_owned(),
            },
        )
        .await
        .unwrap()
    }

    /// 원본에서 `branch` 의 `commit` 을 묶는다(대상이 가진 커밋을 뺀다).
    async fn prepare(&self, branch: &str, commit: &str) -> Prepared {
        let theirs = self.theirs(branch).await.theirs;
        make_bundle(
            self.src.write.git(),
            &self.src.project,
            None,
            branch,
            commit,
            &theirs,
            &CancellationToken::new(),
        )
        .await
        .unwrap()
    }

    /// 묶음 파일을 대상에 받는다.
    async fn receive_file(
        &self,
        file: &Path,
        branch: &str,
        commit: &str,
    ) -> Result<Received, WriteError> {
        receive_bytes(&self.dst, &std::fs::read(file).unwrap(), branch, commit).await
    }

    /// 보내기 한 번: theirs → prepare → receive → apply(`expected` 는 대상의 지금 끝).
    async fn send(&self, branch: &str, commit: &str) -> GitSendOutcome {
        let expected = self.theirs(branch).await.branch_tip;
        self.send_expecting(branch, commit, expected).await
    }

    async fn send_expecting(
        &self,
        branch: &str,
        commit: &str,
        expected: Option<String>,
    ) -> GitSendOutcome {
        let prepared = self.prepare(branch, commit).await;
        self.receive_file(&prepared.file, branch, commit)
            .await
            .unwrap();
        self.apply(branch, commit, expected).await.unwrap()
    }

    async fn apply(
        &self,
        branch: &str,
        commit: &str,
        expected: Option<String>,
    ) -> Result<GitSendOutcome, WriteError> {
        super::apply::apply(
            &self.dst.write,
            Apply {
                caller: self.dst.caller(),
                project_id: self.dst.project.clone(),
                worktree: None,
                source_machine: MACHINE.to_owned(),
                branch: branch.to_owned(),
                commit: commit.to_owned(),
                expected,
            },
        )
        .await
        .map(|(_, outcome)| outcome)
    }

    /// 대상 픽스처가 부른 git 명령(argv).
    fn dst_calls(&self) -> Vec<Vec<String>> {
        calls(&self.dst.bin)
    }

    fn src_calls(&self) -> Vec<Vec<String>> {
        calls(&self.src.bin)
    }
}

/// 받기 한 번(begin → chunk … → finish). 연결 1, role owner.
async fn receive_bytes(
    dst: &WriteFixture,
    bytes: &[u8],
    branch: &str,
    commit: &str,
) -> Result<Received, WriteError> {
    use base64::Engine as _;
    let uploads = Arc::new(Uploads::default());
    let owner = UploadOwner::test(1, Role::Owner);
    let id = uploads
        .begin(
            &dst.write,
            owner,
            begin(dst, branch, commit, bytes.len() as u64),
            CancellationToken::new(),
        )
        .await?;
    let mut offset = 0u64;
    for chunk in bytes.chunks(CHUNK) {
        let data = base64::engine::general_purpose::STANDARD.encode(chunk);
        offset = uploads.chunk(owner, id, offset, &data).await?;
    }
    uploads.finish(&dst.write, owner, id).await
}

fn begin(dst: &WriteFixture, branch: &str, commit: &str, size: u64) -> Begin {
    Begin {
        caller: dst.caller(),
        project_id: dst.project.clone(),
        worktree: None,
        source_machine: MACHINE.to_owned(),
        branch: branch.to_owned(),
        commit: commit.to_owned(),
        size,
    }
}

/// `src` 를 받은 새 저장소(`git pull` — 원격 설정 없이).
fn clone_of(src: &TestRepo) -> TestRepo {
    let dst = TestRepo::new();
    dst.git(&["pull", "--quiet", src.root().to_str().unwrap(), "main"]);
    dst
}

/// `bin/git`: 받은 argv 를 `bin/git.calls` 에 한 줄로(인자 사이 `\x1f`) 적고 진짜 git 을 부른다.
fn record_git(bin: &Path) {
    let path = bin.join("git");
    std::fs::write(
        &path,
        format!(
            "#!/bin/sh\n{{ for a in \"$@\"; do printf '%s\\037' \"$a\"; done; printf '\\n'; }} >> '{log}'\nexec '{git}' \"$@\"\n",
            log = bin.join("git.calls").display(),
            git = which_git().display(),
        ),
    )
    .unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn calls(bin: &Path) -> Vec<Vec<String>> {
    std::fs::read_to_string(bin.join("git.calls"))
        .unwrap_or_default()
        .lines()
        .map(|line| {
            line.split('\u{1f}')
                .filter(|a| !a.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .collect()
}

/// git 명령의 하위 명령(`-c` · `-C` 의 값과 `-` 로 시작하는 인자를 건너뛴 첫 인자)과 그 뒤의 인자.
fn subcommand(argv: &[String]) -> (String, Vec<String>) {
    let mut i = 0;
    while i < argv.len() {
        let a = &argv[i];
        if a == "-c" || a == "-C" {
            i += 2;
            continue;
        }
        if !a.starts_with('-') {
            return (a.clone(), argv[i + 1..].to_vec());
        }
        i += 1;
    }
    (String::new(), Vec::new())
}

/// `bin/<name>`: 불리면 `bin/<name>.called` 를 만들고 실패하는 실행 파일(원격 도우미 · 서버에 닿는 것의 덫).
fn trap(bin: &Path, name: &str) -> PathBuf {
    let path = bin.join(name);
    let marker = bin.join(format!("{name}.called"));
    std::fs::write(
        &path,
        format!("#!/bin/sh\necho \"$@\" >> '{}'\nexit 1\n", marker.display()),
    )
    .unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    marker
}

/// 대상의 지금 상태: 모든 ref(전용 ref 제외)와 HEAD, 작업 트리 상태, 작업 트리 파일 내용.
#[derive(Debug, PartialEq, Eq)]
struct Snapshot {
    refs: String,
    head: String,
    status: String,
    files: Vec<(String, String)>,
}

fn snapshot(repo: &TestRepo) -> Snapshot {
    let refs = repo
        .git(&["for-each-ref", "--format=%(refname) %(objectname)"])
        .lines()
        .filter(|l| !l.starts_with(super::INCOMING_PREFIX))
        .collect::<Vec<_>>()
        .join("\n");
    let head = repo.git(&["symbolic-ref", "-q", "HEAD"]).trim().to_owned() + " " + &repo.head();
    let status = repo.git(&["status", "--porcelain=v1", "--untracked-files=all"]);
    let mut files = Vec::new();
    walk(repo.root(), repo.root(), &mut files);
    files.sort();
    Snapshot {
        refs,
        head,
        status,
        files,
    }
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, String)>) {
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if path.file_name().is_some_and(|n| n == ".git") {
            continue;
        }
        if path.is_dir() {
            walk(root, &path, out);
        } else {
            let rel = path.strip_prefix(root).unwrap().display().to_string();
            out.push((
                rel,
                String::from_utf8_lossy(&std::fs::read(&path).unwrap()).into_owned(),
            ));
        }
    }
}

fn incoming(repo: &TestRepo, branch: &str) -> Option<String> {
    let out = repo.try_git_in(
        repo.root(),
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &super::incoming_ref(MACHINE, branch),
        ],
    );
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

fn ref_of(repo: &TestRepo, name: &str) -> Option<String> {
    let out = repo.try_git_in(repo.root(), &["rev-parse", "--verify", "--quiet", name]);
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
}
