//! 스테이징 · 되돌리기 · stash(`docs/design/13-m2-plan.md` M2-07 — F5-6 · F5-8, 결정 M2-D6).
//!
//! 쓰기는 모두 M2-06 의 실행기([`GitWrite::run`])로 git CLI 를 돈다 — 저장소 잠금, `index.lock` 거절, Task 의 stderr 원문, 작업
//! 트리를 바꾸면 미저장 문서 확인. 사람이 누른 것만 쓴다(미신뢰 프로젝트도 쓰기는 된다, D43). 읽기(stash 목록, 줄 단위의 diff 다시
//! 만들기)는 gix 로만 한다 — 미신뢰 프로젝트도 프로세스를 띄우지 않고 필터 드라이버를 모두 뗀다.
//!
//! - 파일 단위: `git.stage` · `git.unstage` · `git.discard`. 경로 목록은 stdin(`--pathspec-from-file`, NUL 구분)으로 넘기고 글자
//!   그대로 본다(`--literal-pathspecs`) — 이름에 `*` · 줄바꿈이 있어도, 목록이 길어도 된다.
//!   - stage = `git add --all`: untracked 는 더하고, 지운 파일은 지움을 담는다.
//!   - unstage = `git reset -q`: index 를 HEAD 의 것으로(HEAD 에 없으면 index 에서 뺀다). 커밋이 없는 저장소에서도 된다(`git restore
//!     --staged` 는 HEAD 가 없으면 실패한다).
//!   - 둘 다 충돌 중인 경로(폴더면 그 아래 충돌 파일)가 하나라도 있으면 아무것도 하지 않는다(invalid params, `data.conflicted`).
//!     `git add` 는 충돌 파일을 조용히 해결로 표시하고 `git reset` 은 HEAD 쪽을 담아 단계를 지운다 — 해결은
//!     `git.conflict.resolve`(M2-11)로만 한다.
//!   - discard = `git restore --worktree`: 작업 트리를 index 의 것으로. index 에 없는 경로와 intent-to-add(`git add -N`) 파일은
//!     지우지도 비우지도 않고 결과의 `untracked` 로 돌려준다 — intent-to-add 항목은 빈 blob 이라 restore 가 내용을 비우고, 그
//!     내용은 객체 DB 에 없어 되살릴 수 없다.
//! - 줄 단위: `git.stageLines` · `git.unstageLines` · `git.discardLines`([`lines`]).
//! - stash: `git.stash.*`([`stash`]).
//! - 변경 버리기 · stash 버리기는 결정 M2-D6 ① 의 확인 대상이다. 확인 다이얼로그는 창의 몫(M2-20)이고, RPC 는 `confirmed: true` 가
//!   없으면 아무것도 하지 않는다(invalid params). 그 RPC 들은 owner 만 부르고 MCP 툴이 아니다.
//!
//! 작업 트리가 바뀐 뒤 에디터는 M1-13 의 외부 변경 경로(`fs.changed`)로 따른다 — 여기서 따로 알리지 않는다.

pub mod lines;
pub mod methods;
pub mod stash;
#[cfg(test)]
mod tests;

pub use methods::register;

use std::sync::Arc;

use gix::bstr::{BStr, ByteSlice};
use gix::index::entry::Flags as EntryFlags;
use pw_rpc::{ConnectionScope, RpcError, code as rpc_code};

use crate::cli::GitCommand;
use crate::write::{Changes, GitWrite, WriteCtx, WriteError, WriteRequest};
use lines::Op;
use methods::{
    GitDiscardLinesParams, GitDiscardParams, GitDiscardResult, GitLinesParams, GitPathsParams,
    GitStashApplyParams, GitStashApplyResult, GitStashDropParams, GitStashList, GitStashListParams,
    GitStashPushParams, GitStashPushResult, GitWorkTree, HunkSelection,
};

impl GitWrite {
    /// `git.stage`.
    pub async fn stage(
        &self,
        caller: ConnectionScope,
        params: GitPathsParams,
    ) -> Result<GitWorkTree, WriteError> {
        self.paths(caller, params, "task-title-git-stage", &["add", "--all"])
            .await
    }

    /// `git.unstage`.
    pub async fn unstage(
        &self,
        caller: ConnectionScope,
        params: GitPathsParams,
    ) -> Result<GitWorkTree, WriteError> {
        self.paths(caller, params, "task-title-git-unstage", &["reset", "-q"])
            .await
    }

    /// 경로 목록에 index 만 바꾸는 git 명령 하나(stage · unstage). 충돌 중인 경로가 있으면 아무것도 하지 않는다.
    async fn paths(
        &self,
        caller: ConnectionScope,
        params: GitPathsParams,
        title: &str,
        args: &'static [&'static str],
    ) -> Result<GitWorkTree, WriteError> {
        let paths = checked_paths(&params.paths)?;
        let request = request(
            caller,
            params.project_id,
            params.worktree,
            title,
            Changes::None,
        );
        self.run(request, move |ctx| async move {
            let checked = paths.clone();
            let conflicted = ctx
                .read(move |o| conflicted_under(&o.repo, &checked))
                .await?
                .map_err(WriteError::Rpc)?;
            if !conflicted.is_empty() {
                // 이유는 메시지에, 경로는 `data` 에(`invalid_params` 는 `data` 가 이유 문자열이라 쓰지 않는다).
                return Err(WriteError::Rpc(
                    RpcError::new(
                        rpc_code::INVALID_PARAMS,
                        format!(
                            "{} in conflict; resolve conflicts with git.conflict.resolve",
                            conflicted.join(", ")
                        ),
                    )
                    .with_data(serde_json::json!({ "conflicted": conflicted })),
                ));
            }
            ctx.run(pathspecs(&ctx, args, &paths)).await?;
            Ok(work_tree(&ctx))
        })
        .await
    }

    /// `git.discard`.
    pub async fn discard(
        &self,
        caller: ConnectionScope,
        params: GitDiscardParams,
    ) -> Result<GitDiscardResult, WriteError> {
        confirmed(params.confirmed)?;
        let paths = checked_paths(&params.paths)?;
        let request = request(
            caller,
            params.project_id,
            params.worktree,
            "task-title-git-discard",
            Changes::Paths(paths.clone()),
        );
        self.run(request, move |ctx| async move {
            let (tracked, untracked) = ctx
                .read(move |o| split_tracked(&o.repo, paths))
                .await?
                .map_err(WriteError::Rpc)?;
            if !tracked.is_empty() {
                ctx.run(pathspecs(&ctx, &["restore", "--worktree"], &tracked))
                    .await?;
            }
            Ok(GitDiscardResult {
                work_tree: work_tree(&ctx).work_tree,
                untracked,
            })
        })
        .await
    }

    /// `git.stageLines`.
    pub async fn stage_lines(
        &self,
        caller: ConnectionScope,
        params: GitLinesParams,
    ) -> Result<GitWorkTree, WriteError> {
        if params.old_path.is_some() {
            return Err(invalid(
                "`oldPath` is for git.unstageLines only — the work tree diff has no renames",
            ));
        }
        self.lines(LinesWrite {
            caller,
            project_id: params.project_id,
            worktree: params.worktree,
            path: params.path,
            old_path: None,
            hunks: params.hunks,
            ignore_whitespace: params.ignore_whitespace,
            op: Op::Stage,
        })
        .await
    }

    /// `git.unstageLines`.
    pub async fn unstage_lines(
        &self,
        caller: ConnectionScope,
        params: GitLinesParams,
    ) -> Result<GitWorkTree, WriteError> {
        self.lines(LinesWrite {
            caller,
            project_id: params.project_id,
            worktree: params.worktree,
            path: params.path,
            old_path: params.old_path,
            hunks: params.hunks,
            ignore_whitespace: params.ignore_whitespace,
            op: Op::Unstage,
        })
        .await
    }

    /// `git.discardLines`.
    pub async fn discard_lines(
        &self,
        caller: ConnectionScope,
        params: GitDiscardLinesParams,
    ) -> Result<GitWorkTree, WriteError> {
        confirmed(params.confirmed)?;
        self.lines(LinesWrite {
            caller,
            project_id: params.project_id,
            worktree: params.worktree,
            path: params.path,
            old_path: None,
            hunks: params.hunks,
            ignore_whitespace: params.ignore_whitespace,
            op: Op::Discard,
        })
        .await
    }

    async fn lines(&self, w: LinesWrite) -> Result<GitWorkTree, WriteError> {
        if w.ignore_whitespace {
            return Err(invalid(
                "hunks read with ignoreWhitespace do not match the file's lines; select lines in a diff that shows whitespace",
            ));
        }
        let path = checked_path(&w.path)?;
        let old_path = w.old_path.as_deref().map(checked_path).transpose()?;
        if w.hunks.is_empty() {
            return Err(invalid("no hunk is selected"));
        }
        let (title, changes) = match w.op {
            Op::Stage => ("task-title-git-stage", Changes::None),
            Op::Unstage => ("task-title-git-unstage", Changes::None),
            Op::Discard => ("task-title-git-discard", Changes::Paths(vec![path.clone()])),
        };
        let request = request(w.caller, w.project_id, w.worktree, title, changes);
        let (op, hunks) = (w.op, w.hunks);
        self.run(request, move |ctx| async move {
            let patch = ctx
                .read(move |o| lines::prepare(o, op, &path, old_path.as_deref(), &hunks))
                .await?
                .map_err(WriteError::Rpc)?;
            if let Some(patch) = patch {
                ctx.run(ctx.git().args(op.apply_args()).stdin(patch))
                    .await?;
            }
            Ok(work_tree(&ctx))
        })
        .await
    }

    /// `git.stash.list`. 저장소가 없으면 invalid params.
    pub async fn stash_list(&self, params: GitStashListParams) -> Result<GitStashList, WriteError> {
        let git = Arc::clone(self.git());
        tokio::task::spawn_blocking(move || {
            let opened = git
                .open(&params.project_id, params.worktree.as_deref())?
                .ok_or_else(|| no_repository(&params.project_id))?;
            Ok(GitStashList {
                work_tree: display(&opened.target.work_tree),
                stashes: stash::list(&opened.repo)?,
            })
        })
        .await
        .map_err(|e| WriteError::Internal(e.to_string()))?
        .map_err(WriteError::Rpc)
    }

    /// `git.stash.push`.
    pub async fn stash_push(
        &self,
        caller: ConnectionScope,
        params: GitStashPushParams,
    ) -> Result<GitStashPushResult, WriteError> {
        let request = request(
            caller,
            params.project_id,
            params.worktree,
            "task-title-git-stash-push",
            Changes::WorkTree,
        );
        let (message, untracked) = (params.message, params.include_untracked);
        self.run(request, move |ctx| async move {
            let before = ctx
                .read(|o| stash::list(&o.repo))
                .await?
                .map_err(WriteError::Rpc)?
                .len();
            let mut command = ctx.git().args(["stash", "push"]);
            if untracked {
                command = command.arg("--include-untracked");
            }
            if let Some(message) = message.filter(|m| !m.is_empty()) {
                command = command.arg(format!("--message={message}"));
            }
            ctx.run(command).await?;
            let after = ctx
                .read(|o| stash::list(&o.repo))
                .await?
                .map_err(WriteError::Rpc)?;
            let stash = (after.len() > before)
                .then(|| after.into_iter().next())
                .flatten();
            Ok(GitStashPushResult {
                work_tree: work_tree(&ctx).work_tree,
                stash,
            })
        })
        .await
    }

    /// `git.stash.apply`.
    pub async fn stash_apply(
        &self,
        caller: ConnectionScope,
        params: GitStashApplyParams,
    ) -> Result<GitStashApplyResult, WriteError> {
        self.apply_stash(caller, params, false).await
    }

    /// `git.stash.pop`.
    pub async fn stash_pop(
        &self,
        caller: ConnectionScope,
        params: GitStashApplyParams,
    ) -> Result<GitStashApplyResult, WriteError> {
        self.apply_stash(caller, params, true).await
    }

    /// 적용(pop 이면 그 뒤 버리기). 적용은 언제나 커밋 해시로 한다 — `git stash pop stash@{n}` 은 확인과 실행 사이에 다른
    /// 프로세스가 stash 를 만들면 남의 항목을 적용하고 지운다. 버리기는 [`drop_stash`] 의 확인을 거친다.
    async fn apply_stash(
        &self,
        caller: ConnectionScope,
        params: GitStashApplyParams,
        pop: bool,
    ) -> Result<GitStashApplyResult, WriteError> {
        let changes = self.stash_changes(&params).await?;
        let title = if pop {
            "task-title-git-stash-pop"
        } else {
            "task-title-git-stash-apply"
        };
        let request = request(caller, params.project_id, params.worktree, title, changes);
        let (index, commit, reinstate) = (params.index, params.commit, params.reinstate_index);
        self.run(request, move |ctx| async move {
            let entries = ctx
                .read(|o| stash::list(&o.repo))
                .await?
                .map_err(WriteError::Rpc)?;
            stash::check(&entries, index, &commit).map_err(WriteError::Rpc)?;
            // 이미 충돌 중이면(진행 중 병합 등) git 은 index 를 쓰지 못해 아무것도 하지 않고 실패한다 — 그 실패를 「적용했고 충돌을
            // 풀 차례」 로 읽지 않는다.
            let already = ctx
                .read(|o| stash::conflicted(&o.repo))
                .await?
                .map_err(WriteError::Rpc)?;
            let mut command = ctx.git().args(["stash", "apply"]);
            if reinstate {
                command = command.arg("--index");
            }
            let output = ctx.output(command.arg(&commit)).await?;
            let conflicted = if output.status.success() {
                false
            } else if !already
                && ctx
                    .read(|o| stash::conflicted(&o.repo))
                    .await?
                    .map_err(WriteError::Rpc)?
            {
                true
            } else {
                return Err(WriteError::failed("stash", &output));
            };
            // git 의 pop 처럼 충돌로 끝났으면 항목을 남긴다.
            if pop && !conflicted {
                let entries = ctx
                    .read(|o| stash::list(&o.repo))
                    .await?
                    .map_err(WriteError::Rpc)?;
                // 그 사이 누가 이미 지웠으면 지울 것이 없다.
                if let Some(entry) = entries.iter().find(|e| e.commit == commit) {
                    drop_stash(&ctx, entry.index, &commit, true).await?;
                }
            }
            Ok(GitStashApplyResult {
                work_tree: work_tree(&ctx).work_tree,
                conflicted,
            })
        })
        .await
    }

    /// 적용할 stash 가 바꿀 작업 트리 경로(미저장 확인의 범위). 저장소를 읽지 못하면 작업 트리 전체로 본다.
    async fn stash_changes(&self, params: &GitStashApplyParams) -> Result<Changes, WriteError> {
        let git = Arc::clone(self.git());
        let (project, worktree, commit) = (
            params.project_id.clone(),
            params.worktree.clone(),
            params.commit.clone(),
        );
        tokio::task::spawn_blocking(move || {
            let Some(opened) = git.open(&project, worktree.as_deref())? else {
                return Ok(Changes::WorkTree);
            };
            stash::paths(&opened.repo, &commit).map(Changes::Paths)
        })
        .await
        .map_err(|e| WriteError::Internal(e.to_string()))?
        .map_err(WriteError::Rpc)
    }

    /// `git.stash.drop`.
    pub async fn stash_drop(
        &self,
        caller: ConnectionScope,
        params: GitStashDropParams,
    ) -> Result<GitWorkTree, WriteError> {
        confirmed(params.confirmed)?;
        let request = request(
            caller,
            params.project_id,
            params.worktree,
            "task-title-git-stash-drop",
            Changes::None,
        );
        let (index, commit) = (params.index, params.commit);
        self.run(request, move |ctx| async move {
            let entries = ctx
                .read(|o| stash::list(&o.repo))
                .await?
                .map_err(WriteError::Rpc)?;
            stash::check(&entries, index, &commit).map_err(WriteError::Rpc)?;
            drop_stash(&ctx, index, &commit, false).await?;
            Ok(work_tree(&ctx))
        })
        .await
    }
}

/// 줄 단위 쓰기 하나(세 RPC 의 공통 입력).
struct LinesWrite {
    caller: ConnectionScope,
    project_id: String,
    worktree: Option<String>,
    path: String,
    old_path: Option<String>,
    hunks: Vec<HunkSelection>,
    ignore_whitespace: bool,
    op: Op,
}

/// `stash@{index}` 를 지우되, git 이 지웠다고 알린 커밋(`Dropped … (<해시>)`)이 `commit` 이 아니면 — 확인과 실행 사이에 다른
/// 프로세스가 stash 를 만들거나 지워 번호가 밀렸다 — 지운 항목을 그 커밋의 메시지로 `git stash store` 해 되돌려 넣고 conflict.
/// 되돌려 넣은 항목은 맨 위(`stash@{0}`)로 가고, 메시지는 그 커밋의 메시지다 — `git stash push` 가 만든 항목이면 reflog 의 것과
/// 같고, `git stash store -m` 으로 다른 메시지를 붙인 항목은 커밋 메시지로 돌아간다(지운 reflog 줄은 되읽을 수 없다). `applied`
/// 면 pop 의 버리기다(적용은 끝났다고 알린다).
async fn drop_stash(
    ctx: &WriteCtx,
    index: u32,
    commit: &str,
    applied: bool,
) -> Result<(), WriteError> {
    let output = ctx
        .run(
            ctx.git()
                .args(["stash", "drop", &format!("stash@{{{index}}}")]),
        )
        .await?;
    let Some(dropped) = stash::dropped(&output.stdout) else {
        return Err(WriteError::Internal(format!(
            "git stash drop did not say which stash it dropped: {}",
            String::from_utf8_lossy(&output.stdout).trim_end()
        )));
    };
    if dropped == commit {
        return Ok(());
    }
    let id = dropped.clone();
    let message = ctx
        .read(move |o| stash::message(&o.repo, &id))
        .await?
        .map_err(WriteError::Rpc)?;
    ctx.run(ctx.git().args(["stash", "store", "-m", &message, &dropped]))
        .await?;
    let done = if applied {
        "applied the stash but did not drop it"
    } else {
        "dropped nothing"
    };
    Err(WriteError::Rpc(RpcError::new(
        rpc_code::CONFLICT,
        format!(
            "{done}: the stash list moved while dropping {commit} (another stash was put back at stash@{{0}}); read git.stash.list again"
        ),
    )))
}

fn request(
    caller: ConnectionScope,
    project_id: String,
    worktree: Option<String>,
    title: &str,
    changes: Changes,
) -> WriteRequest {
    WriteRequest {
        caller,
        project_id,
        worktree,
        title: title.to_owned(),
        changes,
        askpass: false,
    }
}

/// `git --literal-pathspecs <args> --pathspec-from-file=- --pathspec-file-nul`, 경로는 stdin 에 NUL 구분으로.
fn pathspecs(ctx: &WriteCtx, args: &[&str], paths: &[String]) -> GitCommand {
    ctx.git()
        .arg("--literal-pathspecs")
        .args(args)
        .args(["--pathspec-from-file=-", "--pathspec-file-nul"])
        .stdin(paths.join("\0"))
}

/// 버릴 경로(index 에 있는 파일이거나 그 아래 파일이 있는 폴더)와 버리지 않을 경로로 나눈다. 순서는 그대로다. index 에 없는
/// 경로와 intent-to-add 파일은 버리지 않는다(모듈 머리 주석). 폴더 아래에 intent-to-add 파일이 있으면 폴더 대신 그 아래의 다른
/// 항목을 버리고, intent-to-add 파일은 버리지 않을 쪽에 담는다.
fn split_tracked(
    repo: &gix::Repository,
    paths: Vec<String>,
) -> Result<(Vec<String>, Vec<String>), RpcError> {
    let index = repo
        .index_or_empty()
        .map_err(|e| RpcError::internal(e.to_string()))?;
    let (mut tracked, mut untracked) = (Vec::new(), Vec::new());
    for path in paths {
        let rel = path.as_bytes().as_bstr();
        if let Some(range) = index.entry_range(rel) {
            if index.entries()[range].iter().all(intent_to_add) {
                untracked.push(path);
            } else {
                tracked.push(path);
            }
            continue;
        }
        let Some(entries) = under(&index, &path) else {
            untracked.push(path);
            continue;
        };
        if !entries.iter().any(intent_to_add) {
            tracked.push(path);
            continue;
        }
        for entry in entries {
            let name = entry.path(&index).to_str_lossy().into_owned();
            let side = if intent_to_add(entry) {
                &mut untracked
            } else {
                &mut tracked
            };
            if side.last() != Some(&name) {
                side.push(name);
            }
        }
    }
    Ok((tracked, untracked))
}

/// 경로(파일 · 폴더)에 걸린 충돌 파일. 폴더면 그 아래의 충돌 파일이다.
fn conflicted_under(repo: &gix::Repository, paths: &[String]) -> Result<Vec<String>, RpcError> {
    let index = repo
        .index_or_empty()
        .map_err(|e| RpcError::internal(e.to_string()))?;
    let mut out: Vec<String> = Vec::new();
    for path in paths {
        let entries = match index.entry_range(path.as_bytes().as_bstr()) {
            Some(range) => &index.entries()[range],
            None => under(&index, path).unwrap_or_default(),
        };
        for entry in entries.iter().filter(|e| e.stage_raw() != 0) {
            let name = entry.path(&index).to_str_lossy().into_owned();
            if !out.contains(&name) {
                out.push(name);
            }
        }
    }
    Ok(out)
}

/// 폴더 `path` 아래의 index 항목(없으면 `None`).
fn under<'a>(index: &'a gix::index::State, path: &str) -> Option<&'a [gix::index::Entry]> {
    let prefix = format!("{path}/");
    index.prefixed_entries(BStr::new(prefix.as_bytes()))
}

fn intent_to_add(entry: &gix::index::Entry) -> bool {
    entry.flags.contains(EntryFlags::INTENT_TO_ADD)
}

fn confirmed(confirmed: bool) -> Result<(), WriteError> {
    if confirmed {
        Ok(())
    } else {
        Err(invalid(
            "this write discards work that git cannot bring back; ask the person first and send `confirmed: true`",
        ))
    }
}

fn checked_paths(paths: &[String]) -> Result<Vec<String>, WriteError> {
    if paths.is_empty() {
        return Err(invalid("no path is given"));
    }
    paths.iter().map(|p| checked_path(p)).collect()
}

/// 작업 트리 기준 상대 경로(`/` 구분, `.` · `..` · 빈 칸 없음). 아니면 invalid params.
fn checked_path(path: &str) -> Result<String, WriteError> {
    let ok = !path.is_empty()
        && !path.contains(['\\', '\0'])
        && path
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..");
    if ok {
        Ok(path.to_owned())
    } else {
        Err(invalid(format!(
            "`{path}` must be a path relative to the work tree top, separated by `/`"
        )))
    }
}

fn invalid(message: impl Into<String>) -> WriteError {
    WriteError::Rpc(RpcError::invalid_params(message))
}

fn no_repository(project_id: &str) -> RpcError {
    RpcError::invalid_params(format!("project `{project_id}` has no git repository"))
}

fn work_tree(ctx: &WriteCtx) -> GitWorkTree {
    GitWorkTree {
        work_tree: display(ctx.work_tree()),
    }
}

/// `git.repo` · `git.diff` 와 같은 정규화를 거친 경로 문자열.
fn display(path: &std::path::Path) -> String {
    pw_core::registry::path::normalize(path)
        .unwrap_or_else(|_| path.to_path_buf())
        .display()
        .to_string()
}
