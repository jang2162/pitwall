//! 묶음 시험(18-m7-plan M7-10). 원격 없이 돈다 — 「다른 머신」 은 자기 `$DATA` 를 가진 두 번째 레지스트리를 [`LocalMachine`] 으로
//! 감싼 가짜 원격 Host 다(그 머신의 레지스트리 · 디스크를 읽는 길이 로컬과 같다). 저장소는 실제 `git` 으로 만든다.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use super::add::{
    AddMemberOutcome, AddMemberRejection, AddTo, BundleMachine, CheckoutFacts, LocalMachine,
    MachineFuture, RemoteMachines, add_member, count_files,
};
use super::*;
use crate::bus::{DEFAULT_CAPACITY, EventBus};
use crate::config::AppPaths;
use crate::registry::{NewProject, Project, path, state, store};

const MINI: &str = "SHA256:mini-fingerprint";
const PC: &str = "SHA256:pc-fingerprint";

/// 가짜 원격 머신: 그 머신의 레지스트리와 디스크. `large` 면 파일 수 확인이 늘 「많다」 다. `gate` 가 있으면 등록 직전에 다른 더하기가
/// 같은 자리에 올 때까지(최대 1초) 기다린다 — 두 더하기의 판정이 겹치게 한다.
struct FakeMachine {
    inner: LocalMachine,
    large: bool,
    gate: Option<Arc<tokio::sync::Barrier>>,
    registers: AtomicUsize,
}

impl BundleMachine for FakeMachine {
    fn inspect<'a>(&'a self, path: &'a str) -> MachineFuture<'a, CheckoutFacts> {
        self.inner.inspect(path)
    }
    fn project_root<'a>(&'a self, project_id: &'a str) -> MachineFuture<'a, Option<String>> {
        self.inner.project_root(project_id)
    }
    fn contained<'a>(
        &'a self,
        path: &'a str,
        commits: &'a [String],
    ) -> MachineFuture<'a, Vec<String>> {
        self.inner.contained(path, commits)
    }
    fn reaches_files<'a>(&'a self, path: &'a str, limit: u64) -> MachineFuture<'a, bool> {
        if self.large {
            Box::pin(async { Ok(true) })
        } else {
            self.inner.reaches_files(path, limit)
        }
    }
    fn register<'a>(&'a self, path: &'a str) -> MachineFuture<'a, String> {
        Box::pin(async move {
            self.registers.fetch_add(1, Ordering::SeqCst);
            if let Some(gate) = &self.gate {
                let _ = tokio::time::timeout(Duration::from_secs(1), gate.wait()).await;
            }
            self.inner.register(path).await
        })
    }
    fn unregister<'a>(&'a self, project_id: &'a str) -> MachineFuture<'a, ()> {
        self.inner.unregister(project_id)
    }
}

struct Remotes(HashMap<String, Arc<dyn BundleMachine>>);

impl RemoteMachines for Remotes {
    fn machine(&self, id: &str) -> Option<Arc<dyn BundleMachine>> {
        self.0.get(id).cloned()
    }
}

struct Fixture {
    _tmp: tempfile::TempDir,
    base: PathBuf,
    /// Desktop(이 머신)의 레지스트리.
    desktop: Registry,
    /// 가짜 원격 머신 `MINI` · `PC` 의 레지스트리.
    mini: Registry,
    pc: Registry,
    fakes: HashMap<String, Arc<FakeMachine>>,
}

fn registry(dir: &Path) -> Registry {
    Registry::new(AppPaths::under(dir), None, EventBus::new(DEFAULT_CAPACITY))
}

impl Fixture {
    fn new() -> Self {
        Self::with(false, None)
    }

    fn with_large(large: bool) -> Self {
        Self::with(large, None)
    }

    fn with(large: bool, gate: Option<Arc<tokio::sync::Barrier>>) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let base = path::simplify(fs::canonicalize(tmp.path()).unwrap());
        let desktop = registry(&base.join("desktop-app"));
        let mini = registry(&base.join("mini-app"));
        let pc = registry(&base.join("pc-app"));
        let fake = |r: &Registry| {
            Arc::new(FakeMachine {
                inner: LocalMachine::new(r.clone()),
                large,
                gate: gate.clone(),
                registers: AtomicUsize::new(0),
            })
        };
        let fakes = HashMap::from([(MINI.to_owned(), fake(&mini)), (PC.to_owned(), fake(&pc))]);
        let f = Self {
            _tmp: tmp,
            base,
            desktop,
            mini,
            pc,
            fakes,
        };
        f.desktop.set_remote_machines(f.remotes());
        f
    }

    /// 다른 Desktop 레지스트리(같은 파일을 여는 다른 프로세스)에도 같은 원격 머신을 꽂는다.
    fn remotes(&self) -> Arc<Remotes> {
        Arc::new(Remotes(
            self.fakes
                .iter()
                .map(|(id, m)| (id.clone(), m.clone() as Arc<dyn BundleMachine>))
                .collect(),
        ))
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.base.join(rel)
    }

    /// `rel` 에 커밋 둘(`seed` 가 다르면 다른 역사)을 가진 저장소를 만든다.
    fn repo(&self, rel: &str, seed: &str) -> PathBuf {
        let dir = self.path(rel);
        fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "-q"]);
        fs::write(dir.join("a.txt"), seed).unwrap();
        git(&dir, &["add", "."]);
        git(&dir, &["commit", "-q", "-m", &format!("root {seed}")]);
        fs::write(dir.join("b.txt"), "second").unwrap();
        git(&dir, &["add", "."]);
        git(&dir, &["commit", "-q", "-m", "second"]);
        dir
    }

    fn clone(&self, from: &Path, rel: &str, depth: Option<&str>) -> PathBuf {
        let dir = self.path(rel);
        let url = format!("file://{}", from.display());
        let mut args = vec!["clone", "-q"];
        if let Some(d) = depth {
            args.extend(["--depth", d]);
        }
        args.extend([url.as_str(), dir.to_str().unwrap()]);
        git(&self.base, &args);
        dir
    }

    fn project(&self, registry: &Registry, root: &Path) -> Project {
        registry
            .create(NewProject {
                root: root.to_path_buf(),
                ..NewProject::default()
            })
            .unwrap()
    }

    async fn add(&self, to: AddTo<'_>, machine: &str, path: &Path) -> AddMemberOutcome {
        add_member(&self.desktop, to, machine, path.to_str().unwrap(), false)
            .await
            .unwrap()
    }
}

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .args([
            "-c",
            "user.name=pitwall-test",
            "-c",
            "user.email=test@pitwall.invalid",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "init.defaultBranch=main",
            "-c",
            "protocol.file.allow=always",
        ])
        .args(args)
        .current_dir(dir)
        .output()
        .expect("git 실행 파일이 있어야 묶음 시험을 돌린다");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn added(outcome: AddMemberOutcome) -> ProjectBundle {
    match outcome {
        AddMemberOutcome::Added { bundle } => bundle,
        other => panic!("더해져야 한다: {other:?}"),
    }
}

fn rejection(outcome: AddMemberOutcome) -> AddMemberRejection {
    match outcome {
        AddMemberOutcome::Rejected { reason } => reason,
        other => panic!("거절돼야 한다: {other:?}"),
    }
}

// ① 공통 커밋이 없는 체크아웃은 더하지 않는다. 그 머신의 레지스트리에도 흔적이 없다.
#[tokio::test]
async fn a_checkout_without_a_common_commit_is_not_added() {
    let f = Fixture::new();
    let p = f.project(&f.desktop, &f.repo("local/app", "one"));
    let other = f.repo("mini/other", "two");

    let reason = rejection(f.add(AddTo::Bundle(&p.id), MINI, &other).await);
    assert_eq!(
        reason,
        AddMemberRejection::NoCommonCommit {
            machine: LOCAL_MACHINE.into()
        }
    );
    assert!(
        f.mini.list().unwrap().is_empty(),
        "거절된 더하기가 원격 레지스트리에 쓰면 안 된다"
    );
    let bundle = f.desktop.bundle(&p.id).unwrap();
    assert!(bundle.implicit);
    assert_eq!(bundle.members.len(), 1);
}

// ① 같은 역사의 클론은 더해지고, 암묵 묶음이 행 있는 묶음이 된다. 원격에 프로젝트가 없으면 그 머신에 만든다.
#[tokio::test]
async fn a_clone_with_common_history_is_added_and_registered_on_that_machine() {
    let f = Fixture::new();
    let local = f.repo("local/app", "one");
    let p = f.project(&f.desktop, &local);
    let remote = f.clone(&local, "mini/app", None);
    // 원격에 커밋이 더 있어도(앞서 나간 쪽) 공통 커밋이 있다.
    fs::write(remote.join("c.txt"), "ahead").unwrap();
    git(&remote, &["add", "."]);
    git(&remote, &["commit", "-q", "-m", "ahead"]);

    let bundle = added(f.add(AddTo::Bundle(&p.id), MINI, &remote).await);
    assert!(!bundle.implicit);
    assert!(bundle.id.starts_with(BUNDLE_ID_PREFIX));
    assert_eq!(bundle.name, p.name);
    let registered = f.mini.list().unwrap();
    assert_eq!(registered.len(), 1);
    assert_eq!(
        bundle
            .members
            .iter()
            .map(|m| (m.machine.as_str(), m.project_id.as_str()))
            .collect::<Vec<_>>(),
        [
            (LOCAL_MACHINE, p.id.as_str()),
            (MINI, registered[0].id.as_str())
        ]
    );
    // 로컬 프로젝트 id 는 더는 묶음 id 가 아니고, 목록에 묶음 한 줄로 나온다.
    assert!(matches!(
        f.desktop.bundle(&p.id),
        Err(RegistryError::NotFound(_))
    ));
    assert_eq!(f.desktop.bundles().unwrap(), std::slice::from_ref(&bundle));
    assert_eq!(
        f.desktop.bundle_of(MINI, &registered[0].id).unwrap(),
        Some(bundle)
    );
}

// ① 얕은 클론: 깊이 안에 공통 조상이 있으면 더하고, 없으면 「모른다」 로 거절한다.
#[tokio::test]
async fn a_shallow_clone_is_judged_within_its_depth() {
    let f = Fixture::new();
    let local = f.repo("local/app", "one");
    let p = f.project(&f.desktop, &local);
    let shallow = f.clone(&local, "mini/app", Some("1"));
    added(f.add(AddTo::Bundle(&p.id), MINI, &shallow).await);

    let q = f.project(&f.desktop, &f.repo("local/other", "two"));
    let source = f.repo("src/third", "three");
    let unrelated = f.clone(&source, "mini/third", Some("1"));
    let reason = rejection(f.add(AddTo::Bundle(&q.id), MINI, &unrelated).await);
    assert_eq!(
        reason,
        AddMemberRejection::ShallowUnknown {
            machine: LOCAL_MACHINE.into()
        }
    );
}

// ①b 이미 멤버가 있는 머신의 두 번째 클론은 더하지 않는다 — 로컬 Host 로.
#[tokio::test]
async fn a_second_clone_on_the_local_machine_is_not_added() {
    let f = Fixture::new();
    let local = f.repo("local/app", "one");
    let p = f.project(&f.desktop, &local);
    let second = f.clone(&local, "local/app-copy", None);

    let reason = rejection(f.add(AddTo::Bundle(&p.id), LOCAL_MACHINE, &second).await);
    assert_eq!(
        reason,
        AddMemberRejection::SecondClone {
            member_project_id: p.id.clone()
        }
    );
    // 워크트리는 같은 클론이라 이미 그 멤버의 체크아웃으로 나온다.
    let wt = f.path("local/app-wt");
    git(&local, &["worktree", "add", "-q", wt.to_str().unwrap()]);
    let reason = rejection(f.add(AddTo::Bundle(&p.id), LOCAL_MACHINE, &wt).await);
    assert_eq!(
        reason,
        AddMemberRejection::AlreadyCheckout {
            member_project_id: p.id.clone()
        }
    );
    assert_eq!(
        f.desktop.list().unwrap().len(),
        1,
        "두 번째 클론을 등록하면 안 된다"
    );
}

// ①b 이미 멤버가 있는 머신의 두 번째 클론은 더하지 않는다 — 가짜 원격 Host 로.
#[tokio::test]
async fn a_second_clone_on_a_remote_machine_is_not_added() {
    let f = Fixture::new();
    let local = f.repo("local/app", "one");
    let p = f.project(&f.desktop, &local);
    let first = f.clone(&local, "mini/app", None);
    let bundle = added(f.add(AddTo::Bundle(&p.id), MINI, &first).await);
    let second = f.clone(&local, "mini/app-copy", None);

    let reason = rejection(f.add(AddTo::Bundle(&bundle.id), MINI, &second).await);
    let member = bundle.member(MINI).unwrap().project_id.clone();
    assert_eq!(
        reason,
        AddMemberRejection::SecondClone {
            member_project_id: member
        }
    );
    assert_eq!(f.mini.list().unwrap().len(), 1);
    assert_eq!(f.desktop.bundle(&bundle.id).unwrap(), bundle);
}

// ② 멤버 둘 이상인 묶음의 멤버는 다른 묶음에 들지 않는다. 멤버 하나뿐인 묶음의 멤버는 옮겨 온다.
#[tokio::test]
async fn a_member_of_a_bundle_with_two_members_does_not_join_another() {
    let f = Fixture::new();
    let local = f.repo("local/app", "one");
    let p = f.project(&f.desktop, &local);
    let remote = f.clone(&local, "mini/app", None);
    let first = added(f.add(AddTo::Bundle(&p.id), MINI, &remote).await);
    let q = f.project(&f.desktop, &f.clone(&local, "local/app-2", None));

    let reason = rejection(f.add(AddTo::Bundle(&q.id), MINI, &remote).await);
    assert_eq!(
        reason,
        AddMemberRejection::InOtherBundle {
            bundle_id: first.id.clone()
        }
    );
    assert_eq!(f.desktop.bundle(&first.id).unwrap(), first);
    assert!(f.desktop.bundle(&q.id).unwrap().implicit);

    // 원격만인 묶음(멤버 하나)의 멤버는 옮겨 오고 그 묶음은 사라진다.
    let lone = f.clone(&local, "mini/lone", None);
    let s = f.project(&f.desktop, &f.clone(&local, "local/app-3", None));
    f.desktop.delete(&s.id).unwrap();
    let remote_only = added(f.add(AddTo::NewRemote, MINI, &lone).await);
    assert_eq!(remote_only.members.len(), 1);
    assert!(remote_only.local_project().is_none());
    let moved = added(f.add(AddTo::Bundle(&q.id), MINI, &lone).await);
    assert_eq!(moved.members.len(), 2);
    assert!(matches!(
        f.desktop.bundle(&remote_only.id),
        Err(RegistryError::NotFound(_))
    ));
}

// ③ 빼기는 묶음에서만 — 그 머신의 레지스트리와 파일은 그대로다. 로컬 하나로 줄면 암묵 묶음으로 돌아간다.
#[tokio::test]
async fn removing_a_member_leaves_that_machine_registry_alone() {
    let f = Fixture::new();
    let local = f.repo("local/app", "one");
    let p = f.project(&f.desktop, &local);
    let remote = f.clone(&local, "mini/app", None);
    let bundle = added(f.add(AddTo::Bundle(&p.id), MINI, &remote).await);
    let before = f.mini.list().unwrap();

    f.desktop.remove_bundle_member(&bundle.id, MINI).unwrap();
    assert_eq!(f.mini.list().unwrap(), before);
    assert!(remote.join("a.txt").exists());
    assert_eq!(f.desktop.list().unwrap().len(), 1);
    assert!(matches!(
        f.desktop.bundle(&bundle.id),
        Err(RegistryError::NotFound(_))
    ));
    let back = f.desktop.bundle(&p.id).unwrap();
    assert!(back.implicit);
    assert_eq!(f.desktop.bundle_of(MINI, &before[0].id).unwrap(), None);
}

// ④ 암묵 묶음의 창 label 과 `project.state` 는 M7 전과 같다 — 옛(v4) 레지스트리의 프로젝트를 이전 없이 연다.
#[tokio::test]
async fn an_implicit_bundle_keeps_the_window_label_and_state_of_its_project() {
    let f = Fixture::new();
    let app = f.base.join("old-app");
    let paths = AppPaths::under(&app);
    fs::create_dir_all(&paths.data).unwrap();
    // v5 전의 파일: 묶음 표가 없는 스키마로 프로젝트와 창 상태를 둔다.
    let file = paths.data.join(store::FILE_NAME);
    let old = store::open_with(&file, store::BUSY_TIMEOUT, &store::MIGRATIONS[..4]);
    assert_eq!(
        old.status,
        store::StorageStatus::Ok {
            path: file.display().to_string()
        }
    );
    let root = f.repo("local/app", "one");
    old.conn
        .execute(
            "INSERT INTO projects (id, name, host, config_mode, slug, created_at)
             VALUES ('0123456789abcdef', 'app', 'local', 'repository', 'app', 1)",
            [],
        )
        .unwrap();
    old.conn
        .execute(
            "INSERT INTO project_roots (project_id, position, path) VALUES ('0123456789abcdef', 0, ?1)",
            [root.to_str().unwrap()],
        )
        .unwrap();
    drop(old);
    let saved = serde_json::json!({ "layout": "kept" });
    let state_file = state::state_file(&paths.data, "0123456789abcdef");
    fs::create_dir_all(state_file.parent().unwrap()).unwrap();
    fs::write(&state_file, saved.to_string()).unwrap();

    let registry = registry(&app);
    let bundles = registry.bundles().unwrap();
    assert_eq!(bundles.len(), 1);
    let bundle = &bundles[0];
    assert!(bundle.implicit);
    assert_eq!(bundle.id, "0123456789abcdef");
    assert_eq!(window_label(&bundle.id), "project-0123456789abcdef");
    assert_eq!(
        registry.project_state(&bundle.id).unwrap().state,
        Some(saved)
    );
}

// ⑤ 묶음 id 와 로컬 프로젝트 id 는 겹치지 않는다(프로젝트 id 로 묶음 행을 찾지 않고, 묶음 id 로 프로젝트를 찾지 않는다).
#[tokio::test]
async fn bundle_ids_never_overlap_project_ids() {
    let f = Fixture::new();
    let local = f.repo("local/app", "one");
    let p = f.project(&f.desktop, &local);
    let bundle = added(
        f.add(
            AddTo::Bundle(&p.id),
            MINI,
            &f.clone(&local, "mini/app", None),
        )
        .await,
    );
    let projects = f.desktop.list().unwrap();
    assert!(
        projects.iter().all(|p| !is_bundle_row_id(&p.id)),
        "{projects:?}"
    );
    assert!(is_bundle_row_id(&bundle.id));
    assert!(projects.iter().all(|p| p.id != bundle.id));
    assert!(
        state::valid_id(&bundle.id),
        "창 상태 폴더 이름으로 쓸 수 있어야 한다"
    );
    assert!(matches!(
        f.desktop.get(&bundle.id),
        Err(RegistryError::NotFound(_))
    ));
}

#[tokio::test]
async fn the_last_send_target_is_kept_per_source_member() {
    let f = Fixture::new();
    let local = f.repo("local/app", "one");
    let p = f.project(&f.desktop, &local);
    let bundle = added(
        f.add(
            AddTo::Bundle(&p.id),
            MINI,
            &f.clone(&local, "mini/app", None),
        )
        .await,
    );
    let target = CheckoutRef {
        machine: MINI.into(),
        worktree: Some("/w/ci".into()),
    };

    let after = f
        .desktop
        .set_last_send_target(&bundle.id, LOCAL_MACHINE, Some(&target))
        .unwrap();
    assert_eq!(
        after.member(LOCAL_MACHINE).unwrap().last_target,
        Some(target)
    );
    assert_eq!(after.member(MINI).unwrap().last_target, None);
    let elsewhere = CheckoutRef {
        machine: "SHA256:other".into(),
        worktree: None,
    };
    assert!(
        f.desktop
            .set_last_send_target(&bundle.id, LOCAL_MACHINE, Some(&elsewhere))
            .is_err()
    );
    // 대상 머신을 빼면 그 기억도 지운다.
    f.desktop.remove_bundle_member(&bundle.id, MINI).unwrap();
    assert!(
        f.desktop.bundle(&p.id).unwrap().members[0]
            .last_target
            .is_none()
    );
}

#[tokio::test]
async fn deleting_the_local_project_takes_it_out_of_its_bundle() {
    let f = Fixture::new();
    let local = f.repo("local/app", "one");
    let p = f.project(&f.desktop, &local);
    let bundle = added(
        f.add(
            AddTo::Bundle(&p.id),
            MINI,
            &f.clone(&local, "mini/app", None),
        )
        .await,
    );
    f.desktop.delete(&p.id).unwrap();
    let left = f.desktop.bundle(&bundle.id).unwrap();
    assert_eq!(left.members.len(), 1);
    assert!(
        left.local_project().is_none(),
        "원격 멤버만인 묶음으로 남는다"
    );
}

// ③(F9-10) 파일이 많은 원격 경로는 경고로 멈추고, 사람이 넘기면 더한다.
#[tokio::test]
async fn a_large_remote_directory_asks_before_adding() {
    let f = Fixture::with_large(true);
    let local = f.repo("local/app", "one");
    let p = f.project(&f.desktop, &local);
    let remote = f.clone(&local, "mini/app", None);
    let outcome = f.add(AddTo::Bundle(&p.id), MINI, &remote).await;
    assert_eq!(
        outcome,
        AddMemberOutcome::LargeDirectory {
            at_least: add::LARGE_DIRECTORY_FILES
        }
    );
    assert!(f.mini.list().unwrap().is_empty());
    let outcome = add_member(
        &f.desktop,
        AddTo::Bundle(&p.id),
        MINI,
        remote.to_str().unwrap(),
        true,
    )
    .await
    .unwrap();
    added(outcome);
}

#[test]
fn counting_files_stops_at_the_limit_and_skips_git() {
    let tmp = tempfile::tempdir().unwrap();
    for i in 0..5 {
        fs::write(tmp.path().join(format!("f{i}")), "").unwrap();
    }
    fs::create_dir_all(tmp.path().join(".git/objects")).unwrap();
    for i in 0..10 {
        fs::write(tmp.path().join(format!(".git/objects/o{i}")), "").unwrap();
    }
    assert_eq!(count_files(tmp.path(), 100), 5);
    assert_eq!(count_files(tmp.path(), 3), 3);
}

#[tokio::test]
async fn a_machine_that_is_not_connected_is_an_error() {
    let f = Fixture::new();
    let p = f.project(&f.desktop, &f.repo("local/app", "one"));
    let e = add_member(&f.desktop, AddTo::Bundle(&p.id), "SHA256:away", "/x", false)
        .await
        .unwrap_err();
    assert_eq!(
        e,
        add::AddMemberError::MachineUnavailable("SHA256:away".into())
    );
}

// 같은 저장소 판정의 역방향: 기존 멤버가 얕은 클론이면 그 멤버는 새 체크아웃의 루트를 갖지 않는다 — 새 체크아웃이 멤버의 얕은
// 경계 커밋을 가졌는지로 더한다(리뷰 M2).
#[tokio::test]
async fn a_full_clone_joins_a_bundle_whose_member_is_a_shallow_clone() {
    let f = Fixture::new();
    let full = f.repo("local/app", "one");
    let shallow = f.clone(&full, "mini/app", Some("1"));
    let remote_only = added(f.add(AddTo::NewRemote, MINI, &shallow).await);

    let bundle = added(
        f.add(AddTo::Bundle(&remote_only.id), LOCAL_MACHINE, &full)
            .await,
    );
    assert_eq!(bundle.id, remote_only.id);
    assert_eq!(bundle.members.len(), 2);
    assert!(bundle.local_project().is_some());
}

/// 두 더하기를 한꺼번에 돌린다.
async fn both(
    a: impl std::future::Future<Output = AddMemberOutcome>,
    b: impl std::future::Future<Output = AddMemberOutcome>,
) -> [AddMemberOutcome; 2] {
    let (a, b) = tokio::join!(a, b);
    [a, b]
}

/// 하나는 더해지고 하나는 「묶음이 바뀌었다」 로 거절됐다. (더해진 묶음, 거절된 쪽의 머신 번호)
fn one_added_one_changed(outcomes: [AddMemberOutcome; 2]) -> (ProjectBundle, usize) {
    match outcomes {
        [
            AddMemberOutcome::Added { bundle },
            AddMemberOutcome::Rejected { reason },
        ] => {
            assert_eq!(
                reason,
                AddMemberRejection::BundleChanged {
                    bundle_id: bundle.id.clone()
                }
            );
            (bundle, 1)
        }
        [
            AddMemberOutcome::Rejected { reason },
            AddMemberOutcome::Added { bundle },
        ] => {
            assert_eq!(
                reason,
                AddMemberRejection::BundleChanged {
                    bundle_id: bundle.id.clone()
                }
            );
            (bundle, 0)
        }
        other => panic!("하나만 더해져야 한다: {other:?}"),
    }
}

// 권고 1: 같은 묶음(암묵)에 두 머신을 동시에 더하면 차례로 돈다 — 둘째는 첫째가 끝난 뒤 시작해 판정 전에 「묶음이 바뀌었다」
// 로 거절되고(NotFound 오류가 아니다), 그 머신에 등록하러 가지도 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_adds_to_one_bundle_take_turns() {
    let f = Fixture::with(false, Some(Arc::new(tokio::sync::Barrier::new(2))));
    let local = f.repo("local/app", "one");
    let p = f.project(&f.desktop, &local);
    let on_mini = f.clone(&local, "mini/app", None);
    let on_pc = f.clone(&local, "pc/app", None);

    let outcomes = both(
        f.add(AddTo::Bundle(&p.id), MINI, &on_mini),
        f.add(AddTo::Bundle(&p.id), PC, &on_pc),
    )
    .await;
    let (bundle, loser) = one_added_one_changed(outcomes);
    let (lost, lost_registry) = [(MINI, &f.mini), (PC, &f.pc)][loser];
    assert_eq!(
        f.fakes[lost].registers.load(Ordering::SeqCst),
        0,
        "둘째는 판정 전에 멈춰야 한다"
    );
    assert!(lost_registry.list().unwrap().is_empty());
    assert_eq!(bundle.members.len(), 2);
}

// 권고 1: 다른 프로세스(같은 state.db 를 연 다른 Desktop — 잠금을 나누지 않는다)가 같은 묶음에 먼저 붙이면, 붙이기 트랜잭션이 멤버가
// 판정 때와 다른 것을 보고 거절하고, 판정 뒤 만든 등록을 되돌린다(거절된 더하기는 흔적을 남기지 않는다).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_add_that_loses_a_race_leaves_no_registration() {
    let f = Fixture::with(false, Some(Arc::new(tokio::sync::Barrier::new(2))));
    let local = f.repo("local/app", "one");
    let p = f.project(&f.desktop, &local);
    let other_process = registry(&f.base.join("desktop-app"));
    other_process.set_remote_machines(f.remotes());
    let on_mini = f.clone(&local, "mini/app", None);
    let on_pc = f.clone(&local, "pc/app", None);

    let outcomes = both(
        async {
            add_member(
                &f.desktop,
                AddTo::Bundle(&p.id),
                MINI,
                on_mini.to_str().unwrap(),
                false,
            )
            .await
            .unwrap()
        },
        async {
            add_member(
                &other_process,
                AddTo::Bundle(&p.id),
                PC,
                on_pc.to_str().unwrap(),
                false,
            )
            .await
            .unwrap()
        },
    )
    .await;
    let (bundle, loser) = one_added_one_changed(outcomes);
    let (lost, lost_registry) = [(MINI, &f.mini), (PC, &f.pc)][loser];
    assert_eq!(
        f.fakes[lost].registers.load(Ordering::SeqCst),
        1,
        "둘 다 판정을 지나 등록했다"
    );
    assert!(
        lost_registry.list().unwrap().is_empty(),
        "거절된 쪽의 등록이 남았다"
    );
    assert_eq!(f.desktop.bundle(&bundle.id).unwrap().members.len(), 2);
}

// 권고 4: 머신 id 를 옮긴다(M7-P6 「이 머신을 새로 믿기」) — 멤버와 마지막 보내기 대상 둘 다. 한 묶음에 두 id 가 함께 있으면 거절하고
// 아무것도 바꾸지 않는다. `local` 은 옮기지 않는다.
#[tokio::test]
async fn renaming_a_machine_moves_its_members_and_send_targets() {
    let f = Fixture::new();
    let local = f.repo("local/app", "one");
    let p = f.project(&f.desktop, &local);
    let bundle = added(
        f.add(
            AddTo::Bundle(&p.id),
            MINI,
            &f.clone(&local, "mini/app", None),
        )
        .await,
    );
    let target = CheckoutRef {
        machine: MINI.into(),
        worktree: None,
    };
    f.desktop
        .set_last_send_target(&bundle.id, LOCAL_MACHINE, Some(&target))
        .unwrap();
    let remote = bundle.member(MINI).unwrap().project_id.clone();

    // 같은 묶음에 두 id 가 있으면 거절, 그대로다.
    let with_pc = added(
        f.add(
            AddTo::Bundle(&bundle.id),
            PC,
            &f.clone(&local, "pc/app", None),
        )
        .await,
    );
    assert!(f.desktop.rename_bundle_machine(MINI, PC).is_err());
    assert_eq!(f.desktop.bundle(&bundle.id).unwrap(), with_pc);
    assert!(
        f.desktop
            .rename_bundle_machine(LOCAL_MACHINE, "SHA256:x")
            .is_err()
    );
    assert!(
        f.desktop
            .rename_bundle_machine(MINI, LOCAL_MACHINE)
            .is_err()
    );

    const NEW: &str = "SHA256:mini-new";
    assert_eq!(
        f.desktop.rename_bundle_machine(MINI, NEW).unwrap(),
        [bundle.id.clone()]
    );
    let moved = f.desktop.bundle(&bundle.id).unwrap();
    assert_eq!(moved.member(NEW).unwrap().project_id, remote);
    assert!(moved.member(MINI).is_none());
    assert!(moved.member(PC).is_some());
    assert_eq!(
        moved.member(LOCAL_MACHINE).unwrap().last_target,
        Some(CheckoutRef {
            machine: NEW.into(),
            worktree: None
        })
    );
    assert_eq!(
        f.desktop.bundle_of(NEW, &remote).unwrap().map(|b| b.id),
        Some(bundle.id.clone())
    );
}

// 권고 1: 붙이기 트랜잭션은 판정 때 본 멤버 목록과 지금 목록을 견준다 — 다르면 붙이지 않는다(새 멤버는 지금 멤버들과 판정하지 않았다).
#[tokio::test]
async fn attaching_refuses_when_the_members_changed_since_judging() {
    let f = Fixture::new();
    let local = f.repo("local/app", "one");
    let p = f.project(&f.desktop, &local);
    let bundle = added(
        f.add(
            AddTo::Bundle(&p.id),
            MINI,
            &f.clone(&local, "mini/app", None),
        )
        .await,
    );
    let judged = &bundle.members[..1];

    let attached = f
        .desktop
        .attach_bundle_member(Some(&bundle.id), judged, PC, "f00d", "app")
        .unwrap();
    assert_eq!(
        attached,
        Attached::BundleChanged {
            bundle_id: bundle.id.clone()
        }
    );
    assert_eq!(f.desktop.bundle(&bundle.id).unwrap(), bundle);
}
