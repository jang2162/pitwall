//! 파일 조작(`docs/design/12-m1-plan.md` M1-21, F3-2): 만들기 `fs.create` · 이름 바꾸기와 이동 `fs.rename` · 복사 `fs.copy` · 휴지통
//! `fs.trash`, 그리고 Finder 에서 드롭한 항목을 들여오는 [`Fs::import`](데스크톱 command 만 부른다, RPC 가 아니다).
//!
//! 접근 범위는 [`crate::scope`] 와 같다(결정 M1-D8). 조작은 항목 **자체**에 한다 — 이름을 바꾸거나 지우는 심링크는 링크가 바뀌고
//! 대상은 그대로다. 그래도 판정은 실제 대상으로 한다(규칙 A): 원본이 루트 밖을 가리키는 링크면 거절한다. 대상(새 이름 · 복사본)은
//! 들어갈 폴더를 실제 경로로 풀어 그 안이 루트 안일 때만 받는다. 프로젝트 루트 자체는 바꾸거나 지울 수 없다.
//!
//! **덮지 않는다.** 대상 이름이 이미 있으면 `CONFLICT`(`data.path` 에 그 경로)이고 아무것도 바꾸지 않는다. 확인과 쓰기 사이에 다른
//! 프로세스가 같은 이름을 만들어도 덮지 않도록 마지막 단계가 「없을 때만」 하는 OS 호출이다:
//! - 만들기: 파일은 `O_CREAT | O_EXCL`(`create_new`), 폴더는 `mkdir`.
//! - 이름 바꾸기 · 이동 · 복사의 마지막 자리 잡기: [`rename_noreplace`] — macOS `renamex_np(RENAME_EXCL)`, Linux
//!   `renameat2(RENAME_NOREPLACE)`, Windows `MoveFileExW`(`MOVEFILE_REPLACE_EXISTING` 없이). 복사는 같은 폴더의 임시 이름에 다
//!   만든 뒤 그것으로 자리를 잡는다 — 반쯤 만든 복사본이 대상 이름으로 보이지 않는다.
//! - **한계**: Linux 의 파일 시스템이 `RENAME_NOREPLACE` 를 받지 않으면(`EINVAL`) 파일은 하드 링크 → 원래 이름 지우기로 같은 보장을
//!   지키지만, 폴더는 확인 뒤 `rename` 이다 — 그 사이에 같은 이름의 **빈** 폴더가 생기면 그것을 바꿔 끼운다(빈 폴더가 아니면
//!   `rename` 이 실패한다).
//!
//! 복사는 심링크를 따르지 않는다 — 링크는 같은 대상 글자의 링크로 만든다(Windows 는 권한이 있어야 해 건너뛴다). FIFO · 소켓 · 장치
//! 같은 특수 파일은 건너뛰고 결과의 `skipped` 에 싣는다. 권한 비트는 옮기고 소유자 · 시각 · 확장 속성은 옮기지 않는다.
//!
//! 휴지통은 `trash` 크레이트다. macOS 는 Finder 를 부르는 기본 방식 대신 `NSFileManager` 로 넣는다 — Finder 방식은 처음 쓸 때
//! 「Finder 를 제어하려 한다」 권한 창을 띄운다(대신 Finder 의 「되돌려 놓기」 가 없다). 넣지 못한 항목은 오류가 아니라 결과의
//! `failed` 에 이유와 함께 싣는다 — UI 가 「휴지통이 없음」 이면 확인을 받아 `permanent` 로 다시 부르고, 「권한 없음」 이면 그대로
//! 알린다.

use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use pw_core::registry::path::normalize;
use pw_rpc::{Router, RpcError, code, rpc_method};
use serde::{Deserialize, Serialize};

use crate::scope::{Denied, Roots};
use crate::{Fs, denied, io_error};

/// 만들 것.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum FsCreateKind {
    File,
    Dir,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct FsCreateParams {
    pub project_id: String,
    #[serde(default)]
    pub worktree: Option<String>,
    /// 만들 절대 경로. 들어갈 폴더는 있어야 한다.
    pub path: String,
    pub kind: FsCreateKind,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct FsCreated {
    /// 만든 경로(들어간 폴더를 실제 경로로 푼 것).
    pub path: String,
}

/// 원본 · 대상을 받는 요청(`fs.rename` · `fs.copy`).
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct FsFromToParams {
    pub project_id: String,
    #[serde(default)]
    pub worktree: Option<String>,
    /// 옮기거나 복사할 항목의 절대 경로. 심링크면 링크 자체다.
    pub from: String,
    /// 새 절대 경로(폴더 안의 이름까지). 들어갈 폴더는 있어야 하고 이 이름은 없어야 한다.
    pub to: String,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct FsMoved {
    /// 옮긴 항목의 원래 경로(들어 있던 폴더를 실제 경로로 푼 것). 열린 문서 · 탭의 키를 이것 아래에서 `to` 아래로 바꾼다.
    pub from: String,
    pub to: String,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct FsCopied {
    /// 만든 복사본의 경로.
    pub to: String,
    /// 복사한 파일 · 폴더 · 심링크 수(맨 위 항목 포함).
    #[specta(type = pw_rpc::JsNumber)]
    pub files: u64,
    #[specta(type = pw_rpc::JsNumber)]
    pub dirs: u64,
    #[specta(type = pw_rpc::JsNumber)]
    pub links: u64,
    /// 건너뛴 항목(특수 파일, Windows 의 심링크)의 원본 경로.
    pub skipped: Vec<String>,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct FsTrashParams {
    pub project_id: String,
    #[serde(default)]
    pub worktree: Option<String>,
    /// 지울 항목의 절대 경로들. 하나라도 루트 밖이면 아무것도 지우지 않고 오류다. 다른 항목 아래의 항목은 그 위 항목과 함께 간다.
    pub paths: Vec<String>,
    /// 휴지통을 거치지 않고 영구히 지운다. UI 는 휴지통에 넣지 못한 항목(`failed` 의 `unavailable`)을 사람에게 확인한 뒤에만 켠다.
    #[serde(default)]
    pub permanent: bool,
}

/// 지우지 못한 이유.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum FsTrashReason {
    /// 이 항목을 넣을 휴지통이 없거나 쓰지 못한다(휴지통이 없는 볼륨 · 환경). 확인 뒤 `permanent` 로 지울 수 있다.
    Unavailable,
    /// 이 프로세스가 그 항목을 들어 낼 권한이 없다(들어 있는 폴더에 쓰기 권한이 없다). 영구 삭제도 실패한다.
    Permission,
    /// 그사이 없어졌다.
    Missing,
    /// 그 밖의 실패(영구 삭제의 IO 오류 등).
    Failed,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct FsTrashFailure {
    pub path: String,
    pub reason: FsTrashReason,
    pub message: String,
}

#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct FsTrashed {
    /// 휴지통에 넣었거나(`permanent` 면 지웠거나) 한 항목.
    pub removed: Vec<String>,
    pub failed: Vec<FsTrashFailure>,
}

/// Finder 드롭 들여오기([`Fs::import`])의 결과.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Imported {
    /// 만든 항목(대상 폴더 안의 경로).
    pub copied: Vec<String>,
    /// 대상 폴더에 같은 이름이 있어 들여오지 않은 원본.
    pub conflicts: Vec<String>,
    /// 들여온 항목 안에서 건너뛴 원본(특수 파일, Windows 의 심링크).
    pub skipped: Vec<String>,
    /// 링크로 복사한 심링크 수.
    pub links: u64,
    /// 취소돼 끝까지 하지 않았다. 끝낸 항목은 남는다.
    pub cancelled: bool,
}

rpc_method! {
    /// 빈 파일이나 폴더를 만든다. 같은 이름이 있으면 아무것도 바꾸지 않고 `CONFLICT`(`data.path`).
    pub FsCreateMethod: "fs.create" {
        params: FsCreateParams,
        result: FsCreated,
        tier: Write,
    }

    /// 항목의 이름을 바꾸거나 다른 폴더로 옮긴다(심링크면 링크 자체). 대상 이름이 있으면 덮지 않고 `CONFLICT`. 대소문자만 바꾸는
    /// 이름(대소문자를 무시하는 볼륨에서 같은 항목)은 바꾼다(macOS · Windows).
    pub FsRenameMethod: "fs.rename" {
        params: FsFromToParams,
        result: FsMoved,
        tier: Write,
    }

    /// 파일이나 폴더(그 아래 전부)를 복사한다. 심링크는 따르지 않고 링크로 복사한다. 대상 이름이 있으면 덮지 않고 `CONFLICT`.
    pub FsCopyMethod: "fs.copy" {
        params: FsFromToParams,
        result: FsCopied,
        tier: Write,
    }

    /// 항목들을 OS 휴지통에 넣는다(`permanent` 면 영구히 지운다). 넣지 못한 것은 `failed` 에 이유와 함께.
    pub FsTrashMethod: "fs.trash" {
        params: FsTrashParams,
        result: FsTrashed,
        tier: Write,
    }
}

/// 핸들러를 붙인다(`crate::methods::register` 가 부른다). 디스크 IO 는 블로킹 스레드에서 한다.
pub(crate) fn register(router: &mut Router, fs: Arc<Fs>) {
    async fn blocking<T: Send + 'static>(
        f: impl FnOnce() -> Result<T, RpcError> + Send + 'static,
    ) -> Result<T, RpcError> {
        tokio::task::spawn_blocking(f)
            .await
            .map_err(|e| RpcError::internal(e.to_string()))?
    }
    let (create, rename, copy) = (Arc::clone(&fs), Arc::clone(&fs), Arc::clone(&fs));
    router
        .unary(FsCreateMethod, move |_ctx, params: FsCreateParams| {
            let s = Arc::clone(&create);
            blocking(move || s.create(&params))
        })
        .unary(FsRenameMethod, move |_ctx, params: FsFromToParams| {
            let s = Arc::clone(&rename);
            blocking(move || s.rename(&params))
        })
        .unary(FsCopyMethod, move |_ctx, params: FsFromToParams| {
            let s = Arc::clone(&copy);
            blocking(move || s.copy(&params))
        })
        .unary(FsTrashMethod, move |_ctx, params: FsTrashParams| {
            let s = Arc::clone(&fs);
            blocking(move || s.trash(&params))
        });
}

/// 휴지통에 넣는 길. 시험은 가짜를 넘긴다 — 사람의 휴지통에 시험 파일이 쌓이지 않게.
pub(crate) type Trasher<'a> = &'a dyn Fn(&Path) -> Result<(), trash::Error>;

/// OS 휴지통([`crate::ops`] 머리 주석).
pub(crate) fn os_trash(path: &Path) -> Result<(), trash::Error> {
    #[allow(unused_mut)]
    let mut context = trash::TrashContext::default();
    #[cfg(target_os = "macos")]
    {
        use trash::macos::{DeleteMethod, TrashContextExtMacos as _};
        context.set_delete_method(DeleteMethod::NsFileManager);
    }
    context.delete(path)
}

impl Fs {
    pub fn create(&self, params: &FsCreateParams) -> Result<FsCreated, RpcError> {
        let roots = self.roots(&params.project_id, params.worktree.as_deref())?;
        let path = destination(&roots, Path::new(&params.path), None)?;
        let made = match params.kind {
            FsCreateKind::File => fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
                .map(drop),
            FsCreateKind::Dir => fs::create_dir(&path),
        };
        made.map_err(|e| op_error(&path, e))?;
        Ok(FsCreated {
            path: path.display().to_string(),
        })
    }

    pub fn rename(&self, params: &FsFromToParams) -> Result<FsMoved, RpcError> {
        let roots = self.roots(&params.project_id, params.worktree.as_deref())?;
        let from = entry(&roots, Path::new(&params.from))?;
        let to = destination(&roots, Path::new(&params.to), Some(&from))?;
        if to == from {
            return Err(RpcError::invalid_params(format!(
                "`{}` already has that name",
                from.display()
            )));
        }
        if to.starts_with(&from) {
            return Err(RpcError::invalid_params(format!(
                "cannot move `{}` into itself",
                from.display()
            )));
        }
        // 대소문자만 다른 이름(같은 항목)도 「없을 때만」 으로 바꾼다 — macOS `renamex_np(RENAME_EXCL)` 과 Windows `MoveFileExW` 는
        // 같은 항목의 새 대소문자를 받는다. 보통 `rename(2)` 는 같은 항목이면 아무것도 하지 않는다(POSIX).
        rename_noreplace(&from, &to).map_err(|e| op_error(&to, e))?;
        Ok(FsMoved {
            from: from.display().to_string(),
            to: to.display().to_string(),
        })
    }

    pub fn copy(&self, params: &FsFromToParams) -> Result<FsCopied, RpcError> {
        let roots = self.roots(&params.project_id, params.worktree.as_deref())?;
        let from = entry(&roots, Path::new(&params.from))?;
        let to = destination(&roots, Path::new(&params.to), None)?;
        if to.starts_with(&from) {
            return Err(RpcError::invalid_params(format!(
                "cannot copy `{}` into itself",
                from.display()
            )));
        }
        let mut tally = Tally::default();
        place_copy(&from, &to, &mut tally, &|| false).map_err(|e| op_error(&to, e))?;
        Ok(FsCopied {
            to: to.display().to_string(),
            files: tally.files,
            dirs: tally.dirs,
            links: tally.links,
            skipped: tally.skipped,
        })
    }

    pub fn trash(&self, params: &FsTrashParams) -> Result<FsTrashed, RpcError> {
        self.trash_with(params, &os_trash)
    }

    pub(crate) fn trash_with(
        &self,
        params: &FsTrashParams,
        trasher: Trasher<'_>,
    ) -> Result<FsTrashed, RpcError> {
        let roots = self.roots(&params.project_id, params.worktree.as_deref())?;
        // 모두 판정한 뒤에 지운다 — 하나라도 루트 밖이면 아무것도 지우지 않는다. 없는 것은 판정을 막지 않고 `missing` 으로 싣는다.
        let mut targets = Vec::new();
        let mut failed = Vec::new();
        for path in &params.paths {
            match entry(&roots, Path::new(path)) {
                Ok(entry) => targets.push(entry),
                Err(e) if e.code == code::NOT_FOUND => failed.push(FsTrashFailure {
                    path: path.clone(),
                    reason: FsTrashReason::Missing,
                    message: e.message,
                }),
                Err(e) => return Err(e),
            }
        }
        // 다른 항목 아래의 항목은 그 위 항목과 함께 간다.
        targets.sort();
        targets.dedup();
        let covered: Vec<bool> = targets
            .iter()
            .map(|t| {
                targets
                    .iter()
                    .any(|other| other != t && t.starts_with(other))
            })
            .collect();
        let mut removed = Vec::new();
        for (target, covered) in targets.into_iter().zip(covered) {
            let shown = target.display().to_string();
            if covered {
                removed.push(shown);
                continue;
            }
            match remove_one(&target, params.permanent, trasher) {
                Ok(()) => removed.push(shown),
                Err((reason, message)) => failed.push(FsTrashFailure {
                    path: shown,
                    reason,
                    message,
                }),
            }
        }
        Ok(FsTrashed { removed, failed })
    }

    /// Finder 에서 드롭한 항목(`sources`, 루트 밖이어도 된다)을 `dir` 폴더로 복사한다. 원본은 데스크톱이 그 창의 네이티브 드롭
    /// 이벤트에서 받은 경로만이다 — 이 함수는 RPC 로 열지 않는다(원본 검사가 없다). 대상 폴더는 쓰기 규칙으로 판정하고, 같은 이름이
    /// 있는 항목은 덮지 않고 `conflicts` 에 싣는다. `cancelled` 가 참이 되면 지금 항목을 치우고 멈춘다. `progress` 는 항목 하나를
    /// 끝낼 때마다 끝낸 수로 부른다.
    pub fn import(
        &self,
        project_id: &str,
        worktree: Option<&str>,
        dir: &Path,
        sources: &[PathBuf],
        cancelled: &dyn Fn() -> bool,
        progress: &mut dyn FnMut(usize),
    ) -> Result<Imported, RpcError> {
        let roots = self.roots(project_id, worktree)?;
        let dir = roots.writable_dir(dir).map_err(denied)?;
        if !fs::metadata(&dir).is_ok_and(|m| m.is_dir()) {
            return Err(RpcError::invalid_params(format!(
                "`{}` is not a folder",
                dir.display()
            )));
        }
        let mut out = Imported::default();
        for (done, source) in sources.iter().enumerate() {
            if cancelled() {
                out.cancelled = true;
                break;
            }
            let Some(name) = source.is_absolute().then(|| source.file_name()).flatten() else {
                return Err(RpcError::invalid_params(format!(
                    "`{}` is not an absolute path to a file or folder",
                    source.display()
                )));
            };
            let to = dir.join(name);
            if fs::symlink_metadata(&to).is_ok() {
                out.conflicts.push(source.display().to_string());
                progress(done + 1);
                continue;
            }
            let mut tally = Tally::default();
            match place_copy(source, &to, &mut tally, cancelled) {
                Ok(()) => out.copied.push(to.display().to_string()),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                    out.conflicts.push(source.display().to_string());
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted && cancelled() => {
                    out.cancelled = true;
                    break;
                }
                Err(e) => return Err(io_error(source, e)),
            }
            out.skipped.extend(tally.skipped);
            out.links += tally.links;
            progress(done + 1);
        }
        Ok(out)
    }
}

impl Roots {
    /// 무언가를 넣을 폴더: 실제 경로가 루트 안(루트 자체 포함)이어야 한다.
    pub(crate) fn writable_dir(&self, dir: &Path) -> Result<PathBuf, Denied> {
        let real = normalize(dir).map_err(|_| Denied::Relative(dir.to_path_buf()))?;
        if self.contains(&real) {
            Ok(real)
        } else {
            Err(Denied::Outside(dir.to_path_buf()))
        }
    }
}

/// 경로를 들어 있는 폴더와 마지막 이름으로 나눈다. 이름이 없거나(`/`, `..` 로 끝남) 상대 경로면 invalid params.
fn split(path: &Path) -> Result<(&Path, &std::ffi::OsStr), RpcError> {
    if !path.is_absolute() {
        return Err(denied(Denied::Relative(path.to_path_buf())));
    }
    match (path.parent(), path.components().next_back()) {
        (Some(parent), Some(Component::Normal(name))) => Ok((parent, name)),
        _ => Err(RpcError::invalid_params(format!(
            "`{}` does not name a file or folder",
            path.display()
        ))),
    }
}

/// 조작할 항목 자체의 경로: 들어 있는 폴더를 실제 경로로 풀고 마지막 이름은 그대로 둔다(링크를 따르지 않는다). 규칙 A — 링크면
/// 실제 대상도 루트 안이어야 한다. 없으면 `NOT_FOUND`.
fn entry(roots: &Roots, path: &Path) -> Result<PathBuf, RpcError> {
    let (parent, name) = split(path)?;
    let parent = normalize(parent).map_err(|_| denied(Denied::Relative(path.to_path_buf())))?;
    let entry = parent.join(name);
    inside_not_root(roots, path, &entry)?;
    let real = normalize(&entry).map_err(|_| denied(Denied::Relative(path.to_path_buf())))?;
    if !roots.contains(&real) {
        return Err(denied(Denied::LinkOutside(path.to_path_buf())));
    }
    fs::symlink_metadata(&entry).map_err(|e| io_error(&entry, e))?;
    Ok(entry)
}

/// 새로 생길 항목의 경로: 들어갈 폴더(있어야 한다)를 실제 경로로 풀어 그 안. 이미 있으면 `CONFLICT` — `same` 과 같은 항목이면
/// (대소문자만 다른 이름) 받는다. 여기서의 「있음」 확인은 빨리 알리려는 것이고, 덮지 않는 보장은 마지막 OS 호출이 한다.
fn destination(roots: &Roots, path: &Path, same: Option<&Path>) -> Result<PathBuf, RpcError> {
    let (parent, name) = split(path)?;
    let parent = normalize(parent).map_err(|_| denied(Denied::Relative(path.to_path_buf())))?;
    let dest = parent.join(name);
    inside_not_root(roots, path, &dest)?;
    match fs::metadata(&parent) {
        Ok(meta) if meta.is_dir() => {}
        Ok(_) => {
            return Err(RpcError::invalid_params(format!(
                "`{}` is not a folder",
                parent.display()
            )));
        }
        Err(e) => return Err(io_error(&parent, e)),
    }
    if fs::symlink_metadata(&dest).is_ok() && !same.is_some_and(|s| same_entry(s, &dest)) {
        return Err(exists(&dest));
    }
    Ok(dest)
}

/// `entry` 가 루트 안이고 루트 자체가 아니다.
fn inside_not_root(roots: &Roots, requested: &Path, entry: &Path) -> Result<(), RpcError> {
    if roots.as_slice().iter().any(|root| root == entry) {
        return Err(RpcError::invalid_params(format!(
            "`{}` is a project root",
            requested.display()
        )));
    }
    if !roots.contains(entry) {
        return Err(denied(Denied::Outside(requested.to_path_buf())));
    }
    Ok(())
}

fn exists(path: &Path) -> RpcError {
    RpcError::new(
        code::CONFLICT,
        format!("`{}` already exists", path.display()),
    )
    .with_data(serde_json::json!({ "path": path.display().to_string() }))
}

/// 조작의 IO 오류. 이미 있음은 `CONFLICT`(확인 뒤에 생긴 것), 나머지는 읽기와 같다.
fn op_error(path: &Path, e: io::Error) -> RpcError {
    if e.kind() == io::ErrorKind::AlreadyExists || e.raw_os_error() == Some(ENOTEMPTY) {
        return exists(path);
    }
    io_error(path, e)
}

#[cfg(unix)]
const ENOTEMPTY: i32 = libc::ENOTEMPTY;
#[cfg(windows)]
const ENOTEMPTY: i32 = 145; // ERROR_DIR_NOT_EMPTY

/// 두 경로가 같은 디스크 항목이다(대소문자를 무시하는 볼륨의 다른 대소문자 이름). 링크를 따르지 않는다.
fn same_entry(a: &Path, b: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        match (fs::symlink_metadata(a), fs::symlink_metadata(b)) {
            (Ok(x), Ok(y)) => x.dev() == y.dev() && x.ino() == y.ino(),
            _ => false,
        }
    }
    #[cfg(not(unix))]
    {
        // 파일 id 를 얻는 안정 API 가 없다. 이름이 대소문자만 다르면 같은 항목으로 본다(NTFS 는 기본으로 대소문자를 무시한다).
        a != b
            && a.as_os_str().eq_ignore_ascii_case(b.as_os_str())
            && fs::symlink_metadata(b).is_ok()
    }
}

/// `to` 가 없을 때만 `from` 을 `to` 로 옮긴다(머리 주석). 있으면 `AlreadyExists`(폴더는 `ENOTEMPTY` 일 수 있다).
pub(crate) fn rename_noreplace(from: &Path, to: &Path) -> io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        let (f, t) = (c_path(from)?, c_path(to)?);
        // SAFETY: 두 인자는 NUL 로 끝나는 살아 있는 경로 문자열이다.
        let rc = unsafe { libc::renamex_np(f.as_ptr(), t.as_ptr(), libc::RENAME_EXCL) };
        if rc == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }
    #[cfg(target_os = "linux")]
    {
        let (f, t) = (c_path(from)?, c_path(to)?);
        // SAFETY: 두 경로는 NUL 로 끝나는 살아 있는 문자열이고 AT_FDCWD 는 늘 유효하다.
        let rc = unsafe {
            libc::renameat2(
                libc::AT_FDCWD,
                f.as_ptr(),
                libc::AT_FDCWD,
                t.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        };
        if rc == 0 {
            return Ok(());
        }
        let e = io::Error::last_os_error();
        match e.raw_os_error() {
            Some(libc::EINVAL | libc::ENOSYS) => rename_noreplace_fallback(from, to),
            _ => Err(e),
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt as _;
        use windows_sys::Win32::Storage::FileSystem::MoveFileExW;
        let wide = |p: &Path| -> Vec<u16> { p.as_os_str().encode_wide().chain([0]).collect() };
        let (f, t) = (wide(from), wide(to));
        // SAFETY: 두 인자는 NUL 로 끝나는 살아 있는 UTF-16 경로다. 플래그 0 은 대상이 있으면 실패한다.
        let ok = unsafe { MoveFileExW(f.as_ptr(), t.as_ptr(), 0) };
        if ok != 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
    {
        rename_noreplace_fallback(from, to)
    }
}

/// `RENAME_NOREPLACE` 가 없는 파일 시스템: 파일 · 링크는 하드 링크(대상이 있으면 실패) 뒤 원래 이름을 지운다. 폴더는 확인 뒤
/// rename(머리 주석의 한계).
#[cfg(unix)]
#[cfg_attr(target_os = "macos", allow(dead_code))]
fn rename_noreplace_fallback(from: &Path, to: &Path) -> io::Result<()> {
    if fs::symlink_metadata(from)?.is_dir() {
        if fs::symlink_metadata(to).is_ok() {
            return Err(io::Error::from(io::ErrorKind::AlreadyExists));
        }
        return fs::rename(from, to);
    }
    fs::hard_link(from, to)?;
    fs::remove_file(from)
}

#[cfg(unix)]
fn c_path(path: &Path) -> io::Result<std::ffi::CString> {
    use std::os::unix::ffi::OsStrExt as _;
    std::ffi::CString::new(path.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "a path contains a NUL byte"))
}

#[derive(Default)]
struct Tally {
    files: u64,
    dirs: u64,
    links: u64,
    skipped: Vec<String>,
}

/// `from` 을 `to` 옆의 임시 이름에 다 복사한 뒤 「없을 때만」 으로 `to` 에 자리 잡는다. 실패하거나 취소되면(`Interrupted`) 임시
/// 복사본을 치운다.
fn place_copy(
    from: &Path,
    to: &Path,
    tally: &mut Tally,
    cancelled: &dyn Fn() -> bool,
) -> io::Result<()> {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let dir = to.parent().unwrap_or(Path::new("."));
    let name = to
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = dir.join(format!(
        ".{name}.{}.{}.pitwall-tmp",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let placed = copy_entry(from, &tmp, tally, cancelled).and_then(|()| rename_noreplace(&tmp, to));
    if placed.is_err() {
        let _ = remove_entry(&tmp);
    }
    placed
}

/// 항목 하나(폴더면 그 아래 전부)를 `to`(없는 경로)로 복사한다. 링크는 따르지 않는다.
fn copy_entry(
    from: &Path,
    to: &Path,
    tally: &mut Tally,
    cancelled: &dyn Fn() -> bool,
) -> io::Result<()> {
    if cancelled() {
        return Err(io::Error::from(io::ErrorKind::Interrupted));
    }
    let meta = fs::symlink_metadata(from)?;
    let kind = meta.file_type();
    if kind.is_symlink() {
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(fs::read_link(from)?, to)?;
            tally.links += 1;
        }
        #[cfg(not(unix))]
        tally.skipped.push(from.display().to_string());
        return Ok(());
    }
    if kind.is_dir() {
        fs::create_dir(to)?;
        tally.dirs += 1;
        let mut children: Vec<_> = fs::read_dir(from)?.collect::<io::Result<_>>()?;
        children.sort_by_key(fs::DirEntry::file_name);
        for child in children {
            copy_entry(&child.path(), &to.join(child.file_name()), tally, cancelled)?;
        }
        // 쓰기 권한이 없는 폴더도 다 채운 뒤에 권한을 옮긴다.
        return fs::set_permissions(to, meta.permissions());
    }
    if kind.is_file() {
        let mut source = fs::File::open(from)?;
        let mut target = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(to)?;
        io::copy(&mut source, &mut target)?;
        target.set_permissions(meta.permissions())?;
        tally.files += 1;
        return Ok(());
    }
    // FIFO · 소켓 · 장치: 열면 막히거나 장치를 읽는다.
    tally.skipped.push(from.display().to_string());
    Ok(())
}

/// 항목을 영구히 지운다(링크는 링크만, 폴더는 그 아래까지 — 아래의 링크는 따르지 않는다).
fn remove_entry(path: &Path) -> io::Result<()> {
    let meta = fs::symlink_metadata(path)?;
    if meta.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    }
}

/// 하나를 휴지통에 넣거나(`permanent` 면 영구히) 지운다. 못 하면 이유와 문장.
fn remove_one(
    target: &Path,
    permanent: bool,
    trasher: Trasher<'_>,
) -> Result<(), (FsTrashReason, String)> {
    if fs::symlink_metadata(target).is_err() {
        return Err((
            FsTrashReason::Missing,
            format!("`{}` is gone", target.display()),
        ));
    }
    if !can_unlink_from_parent(target) {
        return Err((
            FsTrashReason::Permission,
            format!(
                "no permission to remove `{}` from its folder",
                target.display()
            ),
        ));
    }
    if permanent {
        return remove_entry(target).map_err(|e| {
            let reason = match e.kind() {
                io::ErrorKind::PermissionDenied => FsTrashReason::Permission,
                io::ErrorKind::NotFound => FsTrashReason::Missing,
                _ => FsTrashReason::Failed,
            };
            (reason, e.to_string())
        });
    }
    trasher(target).map_err(|e| (trash_reason(&e), e.to_string()))
}

/// 휴지통 오류의 이유. 권한 오류만 가려내고 나머지는 「이 항목을 넣을 휴지통이 없다」로 본다 — 권한은 위에서 먼저 봤으므로 남은
/// 실패는 대개 휴지통 쪽이다(휴지통이 없는 볼륨, 휴지통 폴더를 만들 수 없음).
fn trash_reason(e: &trash::Error) -> FsTrashReason {
    match e {
        #[cfg(all(unix, not(target_os = "macos")))]
        trash::Error::FileSystem { source, .. }
            if source.kind() == io::ErrorKind::PermissionDenied =>
        {
            FsTrashReason::Permission
        }
        // macOS `NSFileWriteNoPermissionError`(513), Windows `E_ACCESSDENIED`.
        #[cfg(target_os = "macos")]
        trash::Error::Os { code: 513, .. } => FsTrashReason::Permission,
        #[cfg(windows)]
        trash::Error::Os { code, .. } if *code as u32 == 0x8007_0005 => FsTrashReason::Permission,
        _ => FsTrashReason::Unavailable,
    }
}

/// 들어 있는 폴더에 쓸 수 있다(이름을 빼려면 필요하다). unix 는 `access(W_OK)`, 그 밖은 늘 참(휴지통 · 삭제가 실패를 알린다).
fn can_unlink_from_parent(target: &Path) -> bool {
    #[cfg(unix)]
    {
        let Some(parent) = target.parent() else {
            return false;
        };
        let Ok(c) = c_path(parent) else {
            return false;
        };
        // SAFETY: `c` 는 NUL 로 끝나는 살아 있는 문자열이고 access 는 그것을 읽기만 한다.
        unsafe { libc::access(c.as_ptr(), libc::W_OK) == 0 }
    }
    #[cfg(not(unix))]
    {
        let _ = target;
        true
    }
}

#[cfg(test)]
mod tests;
