//! 멤버 더하기 — 「체크아웃 추가」(18-m7-plan M7-10 ② ③ ⑤, 08 「묶음」).
//!
//! 머신은 [`BundleMachine`] 으로 다룬다. 이 머신은 [`LocalMachine`](레지스트리와 gix 를 직접), 다른 머신은 그 머신 Host 로 가는
//! 구현이 [`Registry::set_remote_machines`] 로 들어온다(링크가 생기는 행의 몫 — 그 전에는 다른 머신이 [`AddMemberError::MachineUnavailable`]).
//! 판정은 머신을 가리지 않고 같다. 순서:
//!
//! 1. 그 경로가 git 저장소가 아니면 거절.
//! 2. 다른 머신의 경로면 파일이 [`LARGE_DIRECTORY_FILES`] 개 이상인지 보고, 사람이 아직 넘기지 않았으면 경고로 멈춘다(F9-10 —
//!    하위 폴더를 고르게 한다).
//! 3. **한 머신의 멤버는 하나**(메인 판정 M7-P20): 그 머신에 멤버가 이미 있으면, 같은 클론의 체크아웃(공용 git 폴더가 같다 —
//!    워크트리)은 「이미 그 멤버의 워크트리로 나온다」, 따로 된 클론은 거절.
//! 4. **같은 저장소**: 묶음의 다른 멤버 모두와 공통 커밋이 있어야 한다 — 양쪽의 바닥 커밋([`super::history`])을 서로 가졌는지
//!    본다. 어느 쪽도 못 찾았는데 한쪽이 얕은 클론이면 「깊이 안에 공통 조상이 없다 — 모른다」 로 거절한다.
//! 5. 다른 묶음: 그 머신 프로젝트가 멤버 둘 이상인 다른 묶음에 있으면 거절, 하나뿐인 묶음이면 옮겨 온다.
//! 6. 그 머신의 레지스트리에 그 경로의 프로젝트가 있으면 그것을, 없으면 만들어 멤버로 넣는다.
//!
//! 판정이 모두 끝난 뒤에야 머신의 레지스트리에 쓴다 — 거절된 더하기는 어느 머신에도 흔적을 남기지 않는다.
//!
//! **동시 더하기**: 한 `Registry` 안에서는 묶음마다 더하기가 차례로 돈다(묶음 id 의 잠금) — 둘째는 첫째가 붙인 멤버를 보고
//! 판정하고, 암묵 묶음이 그사이 행 있는 묶음이 됐으면 판정 전에 `bundleChanged` 로 거절된다. 잠금을 나누지 않는 다른 프로세스
//! (같은 `state.db` 를 연 다른 Desktop)와 겹치면 붙이기 트랜잭션이 멤버 목록이 판정 때와 같은지 다시 보고, 다르면 붙이지 않고
//! 6 에서 새로 만든 등록을 지운다([`BundleMachine::unregister`]).

use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use super::history::{self, HistoryBottoms};
use super::{Attached, LOCAL_MACHINE, ProjectBundle};
use crate::registry::{NewProject, Registry, RegistryError};

/// 이 수 이상의 파일이 있는 원격 경로는 경고한다(F9-10).
pub const LARGE_DIRECTORY_FILES: u64 = 100_000;

pub type MachineFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, AddMemberError>> + Send + 'a>>;

/// 한 경로에 대해 그 머신이 아는 것.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckoutFacts {
    /// 정규화한 경로.
    pub path: String,
    /// 공용 git 폴더(같은 클론의 워크트리끼리 같다). git 저장소가 아니면 `None`.
    pub repository: Option<String>,
    /// 역사 바닥. git 저장소가 아니면 빈 값.
    pub history: HistoryBottoms,
    /// 그 머신 레지스트리에서 이 경로를 루트로(또는 그 프로젝트의 미등록 워크트리의 루트 자리로) 가진 프로젝트.
    pub project: Option<String>,
}

/// 묶음 판정이 한 머신에 묻는 것. 다른 머신의 구현은 그 머신의 Host 로 묻는다.
pub trait BundleMachine: Send + Sync {
    /// 경로의 사실.
    fn inspect<'a>(&'a self, path: &'a str) -> MachineFuture<'a, CheckoutFacts>;
    /// 그 머신 프로젝트의 주 루트. 없는 프로젝트면 `None`.
    fn project_root<'a>(&'a self, project_id: &'a str) -> MachineFuture<'a, Option<String>>;
    /// `commits` 가운데 `path` 의 저장소가 가진 것.
    fn contained<'a>(
        &'a self,
        path: &'a str,
        commits: &'a [String],
    ) -> MachineFuture<'a, Vec<String>>;
    /// `path` 아래 파일이 `limit` 개 이상인가(`.git` 은 세지 않는다).
    fn reaches_files<'a>(&'a self, path: &'a str, limit: u64) -> MachineFuture<'a, bool>;
    /// 그 머신의 레지스트리에 `path` 를 루트로 한 프로젝트를 만들고 id 를 돌려준다.
    fn register<'a>(&'a self, path: &'a str) -> MachineFuture<'a, String>;
    /// [`Self::register`] 로 만든 프로젝트를 지운다 — 붙이기가 거절돼 그 등록을 되돌릴 때만 부른다(폴더는 건드리지 않는다).
    fn unregister<'a>(&'a self, project_id: &'a str) -> MachineFuture<'a, ()>;
}

/// 다른 머신들. 링크가 생기는 행이 구현해 [`Registry::set_remote_machines`] 로 넣는다.
pub trait RemoteMachines: Send + Sync {
    /// 지금 닿을 수 있는 그 머신. 없거나 닿지 못하면 `None`.
    fn machine(&self, id: &str) -> Option<Arc<dyn BundleMachine>>;
}

/// 더하기를 끝내지 못한 오류(판정의 거절이 아니다 — 거절은 [`AddMemberOutcome::Rejected`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AddMemberError {
    Registry(RegistryError),
    /// 그 머신에 닿을 수 없다.
    MachineUnavailable(String),
    /// 그 머신에서 읽지 못했다.
    Machine(String),
}

impl From<RegistryError> for AddMemberError {
    fn from(e: RegistryError) -> Self {
        AddMemberError::Registry(e)
    }
}

impl std::fmt::Display for AddMemberError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AddMemberError::Registry(e) => e.fmt(f),
            AddMemberError::MachineUnavailable(m) => write!(f, "machine `{m}` is not connected"),
            AddMemberError::Machine(e) => f.write_str(e),
        }
    }
}

/// 더하지 않은 까닭. 화면이 사람의 말로 옮긴다.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum AddMemberRejection {
    /// 그 경로가 git 저장소가 아니다.
    NotGitRepository,
    /// 그 체크아웃에 아직 커밋이 없다 — 공통 커밋을 볼 수 없다.
    NoCommits,
    /// 그 머신에 이 묶음의 멤버가 이미 있고, 그 경로는 따로 된 클론이다(한 머신의 멤버는 하나 — 워크트리로 만들어 쓴다).
    SecondClone { member_project_id: String },
    /// 그 경로는 그 머신 멤버의 체크아웃(워크트리)이다 — 이미 묶음에 나온다.
    AlreadyCheckout { member_project_id: String },
    /// 그 머신의 멤버와 공통 커밋이 없다.
    NoCommonCommit { machine: String },
    /// 얕은 클론이라 깊이 안에서 공통 커밋을 찾지 못했다 — 같은 저장소인지 모른다.
    ShallowUnknown { machine: String },
    /// 그 머신 프로젝트가 멤버 둘 이상인 다른 묶음에 있다.
    InOtherBundle { bundle_id: String },
    /// 판정하는 사이 묶음이 바뀌었다(다른 더하기가 먼저 끝났다). 묶음을 다시 읽고 다시 더한다. `bundleId` 는 지금 그 묶음.
    BundleChanged { bundle_id: String },
}

/// 「체크아웃 추가」 의 결과.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum AddMemberOutcome {
    /// 더했다. 묶음은 더한 뒤의 것(암묵 묶음이었으면 새 id).
    Added { bundle: ProjectBundle },
    /// 더하지 않았다.
    Rejected { reason: AddMemberRejection },
    /// 원격 경로의 파일이 많다(F9-10). 하위 폴더를 고르게 하고, 사람이 그대로 가겠다면 `allowLarge` 로 다시 부른다.
    LargeDirectory {
        #[specta(type = pw_rpc::JsNumber)]
        at_least: u64,
    },
}

fn rejected(reason: AddMemberRejection) -> AddMemberOutcome {
    AddMemberOutcome::Rejected { reason }
}

/// 머신 id → 머신. 이 머신은 레지스트리로, 다른 머신은 들어온 [`RemoteMachines`] 로.
pub(crate) fn machine_for(
    registry: &Registry,
    id: &str,
) -> Result<Arc<dyn BundleMachine>, AddMemberError> {
    if id == LOCAL_MACHINE {
        return Ok(Arc::new(LocalMachine::new(registry.clone())));
    }
    registry
        .remote_machines()
        .and_then(|m| m.machine(id))
        .ok_or_else(|| AddMemberError::MachineUnavailable(id.to_owned()))
}

/// 무엇에 더하는가.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddTo<'a> {
    /// 있는 묶음(암묵 묶음이면 로컬 프로젝트 id).
    Bundle(&'a str),
    /// 원격만인 새 묶음(⑤ — 원격 호스트 시트에서 만든다). 이 머신의 경로면 프로젝트를 만들면 된다(거절).
    NewRemote,
}

/// 「체크아웃 추가」. 규칙은 모듈 머리 주석.
pub async fn add_member(
    registry: &Registry,
    to: AddTo<'_>,
    machine: &str,
    path: &str,
    allow_large: bool,
) -> Result<AddMemberOutcome, AddMemberError> {
    // 묶음마다 더하기를 하나씩 — 같은 묶음에 동시에 더한 둘째는 첫째가 붙인 멤버를 보고 판정한다.
    let _turn = match to {
        AddTo::Bundle(id) => Some(registry.bundle_add_lock(id).lock_owned().await),
        AddTo::NewRemote => None,
    };
    let bundle = match to {
        AddTo::Bundle(id) => match blocking(registry, {
            let id = id.to_owned();
            move |r| r.bundle_for_add(&id)
        })
        .await?
        {
            Ok(bundle) => Some(bundle),
            Err(bundle_id) => {
                return Ok(rejected(AddMemberRejection::BundleChanged { bundle_id }));
            }
        },
        AddTo::NewRemote if machine == LOCAL_MACHINE => {
            return Err(AddMemberError::Registry(RegistryError::InvalidPath {
                path: PathBuf::from(path),
                reason: "a bundle of this machine alone is a project — create the project instead"
                    .into(),
            }));
        }
        AddTo::NewRemote => None,
    };
    let target = machine_for(registry, machine)?;
    let facts = target.inspect(path).await?;
    let Some(repository) = facts.repository.clone() else {
        return Ok(rejected(AddMemberRejection::NotGitRepository));
    };
    if facts.history.commits.is_empty() {
        return Ok(rejected(AddMemberRejection::NoCommits));
    }
    if machine != LOCAL_MACHINE
        && !allow_large
        && target
            .reaches_files(&facts.path, LARGE_DIRECTORY_FILES)
            .await?
    {
        return Ok(AddMemberOutcome::LargeDirectory {
            at_least: LARGE_DIRECTORY_FILES,
        });
    }
    let members = bundle.as_ref().map_or(&[][..], |b| &b.members[..]);
    // 한 머신의 멤버는 하나(M7-P20).
    if let Some(same) = members.iter().find(|m| m.machine == machine) {
        let root = target
            .project_root(&same.project_id)
            .await?
            .ok_or_else(|| {
                AddMemberError::Machine(format!("member project `{}` is gone", same.project_id))
            })?;
        let theirs = target.inspect(&root).await?;
        let member_project_id = same.project_id.clone();
        return Ok(rejected(
            if theirs.repository.as_deref() == Some(&repository) {
                AddMemberRejection::AlreadyCheckout { member_project_id }
            } else {
                AddMemberRejection::SecondClone { member_project_id }
            },
        ));
    }
    // 같은 저장소 — 다른 멤버 모두와.
    for member in members {
        let other = machine_for(registry, &member.machine)?;
        let root = other
            .project_root(&member.project_id)
            .await?
            .ok_or_else(|| {
                AddMemberError::Machine(format!("member project `{}` is gone", member.project_id))
            })?;
        let theirs = other.inspect(&root).await?;
        let shared = !other
            .contained(&root, &facts.history.commits)
            .await?
            .is_empty()
            || !target
                .contained(&facts.path, &theirs.history.commits)
                .await?
                .is_empty();
        if !shared {
            let machine = member.machine.clone();
            return Ok(rejected(
                if facts.history.shallow || theirs.history.shallow {
                    AddMemberRejection::ShallowUnknown { machine }
                } else {
                    AddMemberRejection::NoCommonCommit { machine }
                },
            ));
        }
    }
    // 다른 묶음: 이미 등록된 프로젝트면 레지스트리에 쓰기 전에 본다(멤버 둘 이상인 묶음이면 거절).
    if let Some(project_id) = &facts.project {
        let found = blocking(registry, {
            let (machine, project_id) = (machine.to_owned(), project_id.clone());
            move |r| r.bundle_of(&machine, &project_id)
        })
        .await?;
        if let Some(other) = found
            && Some(other.id.as_str()) != bundle.as_ref().map(|b| b.id.as_str())
            && other.members.len() >= 2
        {
            return Ok(rejected(AddMemberRejection::InOtherBundle {
                bundle_id: other.id,
            }));
        }
    }
    let registered = facts.project.is_none();
    let project_id = match facts.project.clone() {
        Some(id) => id,
        None => target.register(&facts.path).await?,
    };
    let name = Path::new(&facts.path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| facts.path.clone());
    let attached = blocking(registry, {
        let bundle_id = bundle.as_ref().map(|b| b.id.clone());
        let judged = members.to_vec();
        let (machine, project_id) = (machine.to_owned(), project_id.clone());
        move |r| r.attach_bundle_member(bundle_id.as_deref(), &judged, &machine, &project_id, &name)
    })
    .await;
    let outcome = match attached {
        Ok(Attached::Bundle(bundle)) => return Ok(AddMemberOutcome::Added { bundle }),
        Ok(Attached::InOtherBundle { bundle_id }) => {
            Ok(rejected(AddMemberRejection::InOtherBundle { bundle_id }))
        }
        Ok(Attached::BundleChanged { bundle_id }) => {
            Ok(rejected(AddMemberRejection::BundleChanged { bundle_id }))
        }
        Err(e) => Err(e),
    };
    // 거절된 더하기는 흔적을 남기지 않는다 — 판정 뒤 이 더하기가 만든 등록을 되돌린다. 되돌리기가 실패하면 그 오류를 낸다(등록이
    // 남았다는 것을 숨기지 않는다).
    if registered {
        target.unregister(&project_id).await?;
    }
    outcome
}

async fn blocking<T: Send + 'static>(
    registry: &Registry,
    f: impl FnOnce(&Registry) -> Result<T, RegistryError> + Send + 'static,
) -> Result<T, AddMemberError> {
    let registry = registry.clone();
    tokio::task::spawn_blocking(move || f(&registry))
        .await
        .map_err(|e| AddMemberError::Machine(e.to_string()))?
        .map_err(AddMemberError::Registry)
}

/// 이 머신: 레지스트리와 gix 를 직접 읽는다. 디스크를 읽으므로 블로킹 스레드에서 돈다.
#[derive(Clone)]
pub struct LocalMachine {
    registry: Registry,
}

impl LocalMachine {
    pub fn new(registry: Registry) -> Self {
        Self { registry }
    }

    fn run<'a, T: Send + 'static>(
        &'a self,
        f: impl FnOnce(&Registry) -> Result<T, AddMemberError> + Send + 'static,
    ) -> MachineFuture<'a, T> {
        let registry = self.registry.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || f(&registry))
                .await
                .map_err(|e| AddMemberError::Machine(e.to_string()))?
        })
    }
}

fn git(e: history::GitReadError) -> AddMemberError {
    AddMemberError::Machine(e.0)
}

/// 레지스트리의 경로 해석으로 이 경로를 가진 프로젝트(루트, 또는 미등록 워크트리의 루트 자리).
fn project_at(registry: &Registry, path: &Path) -> Result<Option<String>, RegistryError> {
    Ok(registry.resolve(path)?.and_then(|r| {
        let at_root = match &r.worktree {
            Some(w) => r.path == w.root || r.path == w.top,
            None => r.path == r.root,
        };
        at_root.then_some(r.project.id)
    }))
}

impl BundleMachine for LocalMachine {
    fn inspect<'a>(&'a self, path: &'a str) -> MachineFuture<'a, CheckoutFacts> {
        let path = PathBuf::from(path);
        self.run(move |registry| {
            let path = crate::registry::existing_dir(&path)?;
            let repository = history::repository(&path).map_err(git)?;
            let history = match &repository {
                Some(_) => history::bottoms(&path).map_err(git)?.unwrap_or_default(),
                None => HistoryBottoms::default(),
            };
            Ok(CheckoutFacts {
                project: project_at(registry, &path)?,
                path: path.display().to_string(),
                repository,
                history,
            })
        })
    }

    fn project_root<'a>(&'a self, project_id: &'a str) -> MachineFuture<'a, Option<String>> {
        let id = project_id.to_owned();
        self.run(move |registry| match registry.get(&id) {
            Ok(p) => Ok(Some(p.roots[0].clone())),
            Err(RegistryError::NotFound(_)) => Ok(None),
            Err(e) => Err(e.into()),
        })
    }

    fn contained<'a>(
        &'a self,
        path: &'a str,
        commits: &'a [String],
    ) -> MachineFuture<'a, Vec<String>> {
        let (path, commits) = (PathBuf::from(path), commits.to_vec());
        self.run(move |_| history::contained(&path, &commits).map_err(git))
    }

    fn reaches_files<'a>(&'a self, path: &'a str, limit: u64) -> MachineFuture<'a, bool> {
        let path = PathBuf::from(path);
        self.run(move |_| Ok(count_files(&path, limit) >= limit))
    }

    fn unregister<'a>(&'a self, project_id: &'a str) -> MachineFuture<'a, ()> {
        let id = project_id.to_owned();
        self.run(move |registry| Ok(registry.delete(&id)?))
    }

    fn register<'a>(&'a self, path: &'a str) -> MachineFuture<'a, String> {
        let root = PathBuf::from(path);
        self.run(move |registry| {
            Ok(registry
                .create(NewProject {
                    root,
                    ..NewProject::default()
                })?
                .id)
        })
    }
}

/// `root` 아래 파일 수, `limit` 에서 멈춘다. `.git` 폴더와 심볼릭 링크는 따라가지 않는다. 읽지 못하는 폴더는 건너뛴다.
pub fn count_files(root: &Path, limit: u64) -> u64 {
    let mut count = 0;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                if entry.file_name() != ".git" {
                    stack.push(entry.path());
                }
            } else {
                count += 1;
                if count >= limit {
                    return count;
                }
            }
        }
    }
    count
}
