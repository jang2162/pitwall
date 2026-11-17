//! 작업 트리 상태 · ref(`docs/design/13-m2-plan.md` M2-02): `git.status` · `git.refs`. 읽기는 gix 로만 한다 — 신뢰와 상관없이 git
//! 을 띄우지 않는다(결정 M2-D8). 저장소는 부를 때마다 지금 신뢰로 연다([`crate::Git::open`]) — 열어 둔 것을 들고 있지 않는다.
//!
//! - 경로는 저장소 작업 트리 기준 상대 경로, 구분자 `/` 다([`paths`]). 응답의 `workTree` 가 그 기준이다.
//! - 큰 저장소(index 항목 [`TASK_THRESHOLD`] 개 이상)의 전체 status 는 작업(`Task`, 제목 [`TASK_TITLE`])으로 돈다 — 상태바에 보이고
//!   `task.cancel` 로 멈춘다. 경로 한정 status 와 작은 저장소는 작업 없이 돈다. 어느 쪽이든 요청이 취소되면 멈춘다.
//! - 계산은 [`compute`](status), [`refs`](ref 목록 · ahead/behind).

pub mod compute;
pub mod paths;
pub mod refs;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

use pw_core::{Scope, TaskSpawner, TaskStatus};
use pw_rpc::{Router, RpcError, code, rpc_method};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::Git;
use crate::methods::{GitRepoParams, RepoHead};

/// 전체 status 를 작업으로 돌리는 index 항목 수.
pub const TASK_THRESHOLD: usize = 10_000;
/// 그 작업의 제목(Fluent 메시지 id, `extensions/i18n-default/i18n/*/project.ftl`).
pub const TASK_TITLE: &str = "task-title-git-status";

#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitStatusParams {
    pub project_id: String,
    /// 활성 워크트리의 꼭대기(`ActiveWorktree.top`, 절대 경로).
    #[serde(default)]
    pub worktree: Option<String>,
    /// 이 경로(저장소 작업 트리 기준, `/` 구분 — 폴더면 그 아래 전체)의 항목만 다시 본다(`git.statusChanged` 의 `paths`). 없으면
    /// 전체. `/` 로 시작하거나 `..` 이 든 경로는 invalid params.
    #[serde(default)]
    pub paths: Option<Vec<String>>,
}

/// Kind of change for one path (one letter of porcelain v2's `XY`).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum GitChange {
    /// `A`. On the working tree side (`unstaged`), a file added with `git add --intent-to-add`.
    Added,
    /// `M` (for a submodule, the commit it points at changed).
    Modified,
    /// `D`.
    Deleted,
    /// `R` — staged side only, moved from `origPath`.
    Renamed,
    /// `C` — staged side only, copied from `origPath`.
    Copied,
    /// `T` (file ↔ symlink, etc.).
    TypeChanged,
    /// `?` — working tree side only.
    Untracked,
}

/// A merge conflict (`XY` of porcelain v2's `u` line).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum GitConflict {
    /// `DD`.
    BothDeleted,
    /// `AU`.
    AddedByUs,
    /// `UD`.
    DeletedByThem,
    /// `UA`.
    AddedByThem,
    /// `DU`.
    DeletedByUs,
    /// `AA`.
    BothAdded,
    /// `UU`.
    BothModified,
}

/// State of one path. A conflict has only `conflict`.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitStatusEntry {
    /// Path relative to the repository's working tree (`/`-separated). Something shown as a single folder, such as an
    /// untracked nested repository, ends with `/`.
    pub path: String,
    /// Original path of a staged rename · copy.
    pub orig_path: Option<String>,
    /// HEAD ↔ index (staged changes).
    pub staged: Option<GitChange>,
    /// Index ↔ working tree (unstaged changes). `untracked` for an untracked file.
    pub unstaged: Option<GitChange>,
    pub conflict: Option<GitConflict>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitStatus {
    /// 저장소의 작업 트리 꼭대기(절대 경로) — 경로들의 기준. `null` 이면 「저장소 없음」 이고 `entries` 는 빈다.
    pub work_tree: Option<String>,
    /// 바뀐 경로(경로순). `paths` 로 불렀으면 그 경로 안의 것만 — 받는 쪽은 그 경로 안의 옛 항목을 이것으로 바꾼다.
    pub entries: Vec<GitStatusEntry>,
    /// 저장소는 있는데 읽지 못하는 형식이면 그 까닭(`entries` 는 빈다). 보통 `null`.
    pub unsupported: Option<GitUnsupported>,
}

/// A repository format gix cannot read. The UI shows git features as an "unsupported repository" (not an error).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum GitUnsupported {
    /// A repository that keeps refs in reftable (`extensions.refStorage = reftable`, `git init --ref-format=reftable`).
    Reftable,
}

/// `repo` 가 gix 로 읽지 못하는 형식이면 그 까닭.
fn unsupported(repo: &gix::Repository) -> Option<GitUnsupported> {
    use gix::bstr::ByteSlice;
    repo.config_snapshot()
        .string("extensions.refStorage")
        .filter(|v| v.as_bytes().eq_ignore_ascii_case(b"reftable"))
        .map(|_| GitUnsupported::Reftable)
}

/// ahead/behind.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitTrack {
    /// Commits only on the branch (`↑`).
    pub ahead: u32,
    /// Commits only on the upstream (`↓`).
    pub behind: u32,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitUpstream {
    /// Short name (`origin/main`; the local branch name when the remote is `.`).
    pub name: String,
    /// Full name (`refs/remotes/origin/main`).
    pub ref_name: String,
    /// `null` means it is configured but the ref does not exist (git's `[gone]`).
    pub track: Option<GitTrack>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitBranch {
    /// Short name (`main`, `feat/x`).
    pub name: String,
    /// `refs/heads/…`.
    pub ref_name: String,
    pub commit: String,
    /// This working tree's HEAD is on this branch.
    pub head: bool,
    pub upstream: Option<GitUpstream>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitRemoteBranch {
    /// Remote name (`origin`).
    pub remote: String,
    /// Short name (`origin/main`).
    pub name: String,
    /// `refs/remotes/…`.
    pub ref_name: String,
    pub commit: String,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitTag {
    pub name: String,
    /// `refs/tags/…`.
    pub ref_name: String,
    /// The object the ref points at (the tag object for an annotated tag).
    pub target: String,
    /// For an annotated tag, the object with the tag peeled off (usually a commit). `null` for a lightweight tag.
    pub peeled: Option<String>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitRefs {
    /// 저장소의 작업 트리 꼭대기. `null` 이면 「저장소 없음」 이고 나머지는 빈다.
    pub work_tree: Option<String>,
    pub head: Option<RepoHead>,
    /// 로컬 브랜치(ref 이름순). 심볼릭 ref 는 넣지 않는다.
    pub branches: Vec<GitBranch>,
    /// 원격 추적 브랜치(ref 이름순). `origin/HEAD` 같은 심볼릭 ref 는 넣지 않는다.
    pub remote_branches: Vec<GitRemoteBranch>,
    pub tags: Vec<GitTag>,
    /// 저장소는 있는데 읽지 못하는 형식이면 그 까닭(나머지는 빈다). 보통 `null`.
    pub unsupported: Option<GitUnsupported>,
    /// 읽지 못해 건너뛴 ref(git 의 `warning: ignoring broken ref`). 보통 빈다.
    pub broken: Vec<GitBrokenRef>,
}

impl GitRefs {
    fn empty(work_tree: Option<String>, unsupported: Option<GitUnsupported>) -> Self {
        Self {
            work_tree,
            head: None,
            branches: Vec::new(),
            remote_branches: Vec::new(),
            tags: Vec::new(),
            unsupported,
            broken: Vec::new(),
        }
    }
}

/// 읽지 못한 ref 하나.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct GitBrokenRef {
    /// 전체 이름. 이름조차 읽지 못했으면(ref 파일을 열지 못함) `null`.
    pub ref_name: Option<String>,
    /// 사람이 읽을 이유(gix 의 오류 문장, 영어).
    pub error: String,
}

rpc_method! {
    /// 작업 트리 상태(gix): HEAD↔index · index↔작업 트리 · untracked(파일마다) · 충돌 · 스테이징한 이름 바뀜. 경로는 저장소 작업
    /// 트리 기준(`/`). `paths` 를 주면 그 경로만 다시 본다. 저장소가 없으면 `workTree: null`. 큰 저장소의 전체 status 는 작업으로
    /// 돌고, 작업을 취소하면 cancelled 오류다.
    pub GitStatusMethod: "git.status" {
        params: GitStatusParams,
        result: GitStatus,
        tier: Read,
        roles: [Owner],
    }
}

rpc_method! {
    /// 로컬 · 원격 브랜치 · 태그 · HEAD · upstream · ahead/behind(gix). 저장소가 없으면 `workTree: null`.
    pub GitRefsMethod: "git.refs" {
        params: GitRepoParams,
        result: GitRefs,
        tier: Read,
        roles: [Owner],
    }
}

/// `git.status` · `git.refs` 서비스. Host 를 만든 뒤 작업 spawner 를 붙인다([`Status::attach`]) — 붙기 전에는 큰 저장소도 작업
/// 없이 돈다.
pub struct Status {
    git: Arc<Git>,
    tasks: OnceLock<TaskSpawner>,
    staged: Arc<compute::StagedCache>,
    generations: refs::Generations,
}

impl Status {
    pub fn new(git: Arc<Git>) -> Self {
        Self {
            git,
            tasks: OnceLock::new(),
            staged: Arc::default(),
            generations: refs::Generations::default(),
        }
    }

    /// 이 서비스가 쓰는 `Git`.
    pub fn git(&self) -> &Arc<Git> {
        &self.git
    }

    /// 두 번째부터는 무시한다.
    pub fn attach(&self, tasks: TaskSpawner) {
        let _ = self.tasks.set(tasks);
    }

    /// `git.status`.
    pub async fn status(self: &Arc<Self>, params: GitStatusParams) -> Result<GitStatus, RpcError> {
        if let Some(paths) = &params.paths
            && let Some(bad) = paths.iter().find(|p| !paths::is_valid(p))
        {
            return Err(RpcError::invalid_params(format!(
                "`{bad}` must be relative to the work tree, with `/` and without `..`"
            )));
        }
        let this = Arc::clone(self);
        let project_id = params.project_id.clone();
        let opened = blocking(move || {
            let opened = this
                .git
                .open(&params.project_id, params.worktree.as_deref())?;
            let Some(mut opened) = opened else {
                return Ok(Err(None));
            };
            if !opened.trusted {
                crate::diff::changes::without_filter_drivers(&mut opened.repo)?;
            }
            crate::diff::changes::without_safecrlf(&mut opened.repo)?;
            if let Some(why) = unsupported(&opened.repo) {
                return Ok(Err(Some((display(&opened.target.work_tree), why))));
            }
            let entries = compute::index_entries(&opened.repo).map_err(RpcError::internal)?;
            Ok(Ok((opened, entries, params.paths)))
        })
        .await?;
        let (opened, entries, limit) = match opened {
            Ok(opened) => opened,
            Err(unreadable) => {
                let (work_tree, unsupported) = unreadable.unzip();
                return Ok(GitStatus {
                    work_tree,
                    entries: Vec::new(),
                    unsupported,
                });
            }
        };
        let work_tree = display(&opened.target.work_tree);
        let interrupt = Arc::new(AtomicBool::new(false));
        // 요청이 취소되면(이 future 가 버려지면) 계산도 멈춘다.
        let _stop = StopOnDrop(Arc::clone(&interrupt));
        let task = match (&limit, self.tasks.get()) {
            (None, Some(tasks)) if entries >= TASK_THRESHOLD => Some(tasks),
            _ => None,
        };
        let compute = {
            let interrupt = Arc::clone(&interrupt);
            let cache = Arc::clone(&self.staged);
            move || {
                compute::status(
                    &opened.repo,
                    limit.as_deref(),
                    &interrupt,
                    Some((&cache, opened.trusted)),
                )
            }
        };
        let result = match task {
            None => blocking(move || Ok(compute())).await?,
            Some(tasks) => {
                let tasks = tasks.scoped(Scope::Project(project_id), CancellationToken::new());
                let (tx, rx) = tokio::sync::oneshot::channel();
                let flag = Arc::clone(&interrupt);
                let mut handle = tasks.spawn(TASK_TITLE, move |ctx| async move {
                    // 작업이 취소되면 계산을 멈춘다(취소된 작업의 future 는 곧 버려지지만 블로킹 스레드는 깃발로만 멈춘다).
                    let cancelled = ctx.cancellation().clone();
                    let watcher = tokio::spawn(async move {
                        cancelled.cancelled().await;
                        flag.store(true, Ordering::Relaxed);
                    });
                    let result = tokio::task::spawn_blocking(compute)
                        .await
                        .unwrap_or_else(|e| Err(compute::Stopped::Failed(e.to_string())));
                    watcher.abort();
                    let failed = match &result {
                        Err(compute::Stopped::Failed(e)) => Some(e.clone()),
                        _ => None,
                    };
                    let _ = tx.send(result);
                    failed.map_or(Ok(()), Err)
                });
                let _cancel = handle.cancel_on_drop();
                match handle.wait().await {
                    TaskStatus::Cancelled => Err(compute::Stopped::Interrupted),
                    _ => rx.await.unwrap_or(Err(compute::Stopped::Interrupted)),
                }
            }
        };
        match result {
            Ok(entries) => Ok(GitStatus {
                work_tree: Some(work_tree),
                entries,
                unsupported: None,
            }),
            Err(compute::Stopped::Interrupted) => Err(RpcError::new(
                code::CANCELLED,
                "the git status was cancelled",
            )),
            Err(compute::Stopped::Failed(e)) => Err(RpcError::internal(format!(
                "cannot read the status of {work_tree}: {e}"
            ))),
        }
    }

    /// `git.refs`. 블로킹(디스크 IO) — 블로킹 스레드에서 부른다.
    pub fn refs(&self, params: &GitRepoParams) -> Result<GitRefs, RpcError> {
        let Some(opened) = self
            .git
            .open(&params.project_id, params.worktree.as_deref())?
        else {
            return Ok(GitRefs::empty(None, None));
        };
        let work_tree = display(&opened.target.work_tree);
        if let Some(why) = unsupported(&opened.repo) {
            return Ok(GitRefs::empty(Some(work_tree), Some(why)));
        }
        let mut refs = refs::refs(&opened.repo, Some(&self.generations))
            .map_err(|e| RpcError::internal(format!("cannot read the refs of {work_tree}: {e}")))?;
        refs.work_tree = Some(work_tree);
        Ok(refs)
    }
}

/// 버려질 때 멈춤 깃발을 세운다.
struct StopOnDrop(Arc<AtomicBool>);

impl Drop for StopOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, RpcError> + Send + 'static,
) -> Result<T, RpcError> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| RpcError::internal(e.to_string()))?
}

/// 레지스트리 · `git.repo` 와 같은 정규화를 거친 경로.
pub(crate) fn display(path: &std::path::Path) -> String {
    pw_core::registry::path::normalize(path)
        .unwrap_or_else(|_| path.to_path_buf())
        .display()
        .to_string()
}

/// 핸들러를 붙인다. git 조회 툴 메서드(`crate::tools`)도 여기서 붙는다 — 툴은 `git.status` 의 서비스와 그것이 쓰는 `Git` 을 함께
/// 부르므로, 둘을 다 가진 이 자리가 앱의 Host 에 `git.status` 와 함께 툴을 붙인다(헤드리스 Host 는 git 을 붙이지 않는다).
pub fn register(router: &mut Router, status: Arc<Status>) {
    crate::tools::register(router, Arc::clone(&status));
    let for_status = Arc::clone(&status);
    router.unary(GitStatusMethod, move |_ctx, params: GitStatusParams| {
        let status = Arc::clone(&for_status);
        async move { status.status(params).await }
    });
    router.unary(GitRefsMethod, move |_ctx, params: GitRepoParams| {
        let status = Arc::clone(&status);
        async move { blocking(move || status.refs(&params)).await }
    });
}
