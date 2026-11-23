//! 원격 머신의 열기 요청(18-m7-plan M7-15 ② · 결정 M7-D3 · 메인 판정 2026-11-23)을 이 Desktop 의 묶음 창으로 옮긴다.
//!
//! - **듣기**: 붙은 원격 호스트마다(`pw_remote::RemoteHosts` 의 상태를 읽기만 — M7-08 의 것) 그 머신의 Host 에 owner · Host 범위
//!   연결을 하나 열어 `open.listen`(`pw_core::open::desks`)을 연다. 상태가 바뀌면 [`FOLLOW_EVERY`] 안에 따라간다.
//! - **질문에 답하기**: Relay 는 듣는 Desktop 마다 「마지막 사람 입력 뒤 지난 시간」 만 묻는다. 창 목록 · 프로젝트 이름은 원격에
//!   보내지 않는다. 입력 시각은 [`LastInput`] — 웹뷰의 키 · 포인터 입력(`note_input`, 10초에 한 번)과 창이 앞에 온 순간.
//! - **열기**: 그 머신 프로젝트가 든 묶음([`Registry::bundle_of`])의 창에 **활성 체크아웃을 바꾸지 않고** 그 체크아웃의 탭으로
//!   연다([`plan`] — `ProjectOpen.worktree` 는 비우고 `checkout` 에 싣는다). 창이 없으면 앞으로 가져오지 않고 연다. 묶음에 없는
//!   프로젝트 · 경로가 그 체크아웃 밖이면 거절한다(메인 판정 M7-P19 — Desktop 은 Relay 를 믿지 않는다). 다른 머신 체크아웃의 탭을
//!   그리는 것은 M7-11b 다.

use std::collections::HashMap;
use std::path::{Component, Path};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use pw_core::open::OpenedIn;
use pw_core::open::desks::{OpenAnswer, OpenEvent};
use pw_core::registry::Registry;
use pw_core::registry::bundle::{CheckoutRef, ProjectBundle};
use pw_core::{Connection, Host, HostRouter};
use pw_rpc::{Caller, Role};
use serde_json::{Value, json};

use crate::windows::ProjectOpen;

/// 붙은 원격 호스트 목록을 다시 보는 간격.
pub const FOLLOW_EVERY: Duration = Duration::from_secs(1);
/// 입력이 없었던 Desktop 의 「지난 시간」.
const NEVER: u64 = u64::MAX / 2;

/// 이 Desktop 의 마지막 사람 입력 시각. 앱 상태로 둔다.
#[derive(Default)]
pub struct LastInput(Mutex<Option<Instant>>);

impl LastInput {
    pub fn note(&self) {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = Some(Instant::now());
    }

    /// 마지막 입력 뒤 지난 밀리초(없었으면 아주 큰 값).
    pub fn idle_ms(&self) -> u64 {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .map_or(NEVER, |at| {
                u64::try_from(at.elapsed().as_millis()).unwrap_or(NEVER)
            })
    }
}

/// 웹뷰가 키 · 포인터 입력이 있을 때 부른다(`src/remote-input.ts` — 10초에 한 번).
#[tauri::command]
pub fn note_input(input: tauri::State<'_, Arc<LastInput>>) {
    input.note();
}

/// 원격의 열기 요청 하나(`OpenEvent::Open` 의 몸통).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteOpen {
    pub project_id: String,
    pub worktree: Option<String>,
    pub path: Option<String>,
    pub line: Option<u32>,
    pub column: Option<u32>,
}

/// 열 묶음 창과 그 창의 요청.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Planned {
    pub bundle: ProjectBundle,
    pub open: ProjectOpen,
}

/// 절대 경로이고 `.` · `..` 마디가 없고 `base` 아래인가(글자로만 — 그 머신의 파일 시스템은 Relay 가 본다).
fn under(path: &str, base: &str) -> bool {
    let path = Path::new(path);
    path.is_absolute()
        && !path
            .components()
            .any(|c| matches!(c, Component::CurDir | Component::ParentDir))
        && path.starts_with(base)
}

/// 머신 `machine` 의 열기 요청을 이 Desktop 의 묶음 창 요청으로. `root` 는 그 멤버 프로젝트의 루트(그 머신이 말한 것 — 워크트리
/// 요청이면 그 워크트리 꼭대기가 기준이다). 활성 체크아웃을 바꾸는 `worktree` 는 늘 비운다.
pub fn plan(
    registry: &Registry,
    machine: &str,
    request: &RemoteOpen,
    root: Option<&str>,
) -> Result<Planned, String> {
    let bundle = registry
        .bundle_of(machine, &request.project_id)
        .map_err(|e| format!("cannot read the projects on the desktop: {e}"))?;
    plan_in(bundle, machine, request, root)
}

/// [`plan`] 의 몸통 — `bundle` 은 그 머신 프로젝트가 든 묶음.
fn plan_in(
    bundle: Option<ProjectBundle>,
    machine: &str,
    request: &RemoteOpen,
    root: Option<&str>,
) -> Result<Planned, String> {
    let bundle = bundle
        .filter(|b| {
            b.member(machine)
                .is_some_and(|m| m.project_id == request.project_id)
        })
        .ok_or_else(|| "this project is not in a Pitwall project on the desktop".to_owned())?;
    let base = request.worktree.as_deref().or(root);
    if let Some(path) = &request.path
        && !base.is_some_and(|base| under(path, base))
    {
        return Err("the path is outside that project's checkout".to_owned());
    }
    if request.worktree.as_deref().is_some_and(|w| !under(w, "/")) {
        return Err("the worktree must be an absolute path".to_owned());
    }
    Ok(Planned {
        bundle,
        open: ProjectOpen {
            worktree: None,
            file: request.path.clone(),
            line: request.path.as_ref().and(request.line),
            col: request.path.as_ref().and(request.column),
            checkout: Some(CheckoutRef {
                machine: machine.to_owned(),
                worktree: request.worktree.clone(),
            }),
        },
    })
}

/// 묶음 창을 여는 쪽(앱 — 시험은 가짜).
pub trait Windows: Send + Sync + 'static {
    /// 그 묶음 창에 요청을 넣는다(없으면 앞으로 가져오지 않고 만든다).
    fn open(&self, planned: &Planned) -> Result<OpenedIn, String>;
}

/// 붙은 원격 머신 하나의 듣기. 연결이 끊기면 끝난다.
pub async fn listen(
    machine: String,
    host: Arc<dyn Host>,
    registry: Registry,
    input: Arc<LastInput>,
    windows: Arc<dyn Windows>,
) {
    let Ok(connection) = host.connect(Caller::new(Role::Owner)).await else {
        return;
    };
    let mut rpc = Rpc {
        connection,
        next: 0,
    };
    if !rpc.listen().await {
        tracing::info!(%machine, "the remote host does not take open requests");
        return;
    }
    let mut token = String::new();
    while let Some(event) = rpc.chunk().await {
        let Ok(event) = serde_json::from_value::<OpenEvent>(event) else {
            continue;
        };
        match event {
            OpenEvent::Listening { listener } => token = listener,
            OpenEvent::Query { request_id } => {
                rpc.answer(
                    &token,
                    request_id,
                    OpenAnswer::Idle {
                        idle_ms: input.idle_ms(),
                    },
                );
            }
            OpenEvent::Open {
                request_id,
                project_id,
                worktree,
                path,
                line,
                column,
            } => {
                let request = RemoteOpen {
                    project_id,
                    worktree,
                    path,
                    line,
                    column,
                };
                let root = rpc.project_root(&request.project_id).await;
                let (registry, windows, from) =
                    (registry.clone(), Arc::clone(&windows), machine.clone());
                let answer = tokio::task::spawn_blocking(move || {
                    plan(&registry, &from, &request, root.as_deref())
                        .and_then(|planned| windows.open(&planned))
                })
                .await
                .unwrap_or_else(|e| Err(e.to_string()));
                let answer = match answer {
                    Ok(opened_in) => OpenAnswer::Opened { opened_in },
                    Err(message) => {
                        tracing::info!(%machine, %message, "refused a remote open request");
                        OpenAnswer::Failed { message }
                    }
                };
                rpc.answer(&token, request_id, answer);
            }
        }
    }
}

/// 원격 Host 연결 위의 작은 JSON-RPC 손님.
struct Rpc {
    connection: Connection,
    next: i64,
}

impl Rpc {
    fn send(&mut self, method: &str, params: Value) -> i64 {
        self.next += 1;
        self.connection
            .sink
            .send(json!({"jsonrpc": "2.0", "id": self.next, "method": method, "params": params}));
        self.next
    }

    /// `open.listen` 을 연다. 거절되면 `false`.
    async fn listen(&mut self) -> bool {
        let id = self.send("open.listen", json!({}));
        loop {
            let Some(frame) = self.connection.source.recv().await else {
                return false;
            };
            if frame["id"] == json!(id) {
                let Some(stream) = frame.get("result").and_then(|r| r.get("streamId")).cloned()
                else {
                    return false;
                };
                self.connection.sink.send(json!({"jsonrpc": "2.0", "method": "stream.credit", "params": {"streamId": stream, "credit": 1_000_000}}));
                return true;
            }
        }
    }

    async fn chunk(&mut self) -> Option<Value> {
        loop {
            let frame = self.connection.source.recv().await?;
            match frame["method"].as_str() {
                Some("stream.chunk") => return Some(frame["params"]["data"].clone()),
                Some("stream.end") => return None,
                _ => {}
            }
        }
    }

    fn answer(&mut self, token: &str, request_id: u64, answer: OpenAnswer) {
        self.send(
            "open.answer",
            json!({"listener": token, "requestId": request_id, "answer": answer}),
        );
    }

    /// 그 머신 프로젝트의 루트(`machine.projectRoot`). 그사이 온 열기 청크는 버리지 않도록 다른 연결로 묻지 않고, 답만 기다린다 —
    /// Relay 는 한 요청씩 묻고 답을 기다리므로 그사이 다른 청크는 오지 않는다.
    async fn project_root(&mut self, project_id: &str) -> Option<String> {
        let id = self.send("machine.projectRoot", json!({ "projectId": project_id }));
        loop {
            let frame = tokio::time::timeout(Duration::from_secs(5), self.connection.source.recv())
                .await
                .ok()??;
            if frame["id"] == json!(id) {
                return frame["result"]["root"].as_str().map(str::to_owned);
            }
        }
    }
}

/// 붙은 원격 호스트를 따라 듣기를 열고 닫는다. 앱이 사는 동안 돈다.
pub async fn follow(
    remote: pw_remote::RemoteHosts,
    router: Arc<HostRouter>,
    registry: Registry,
    input: Arc<LastInput>,
    windows: Arc<dyn Windows>,
) {
    let mut listening: HashMap<String, tokio::task::JoinHandle<()>> = HashMap::new();
    loop {
        let connected: Vec<String> = remote
            .list()
            .hosts
            .into_iter()
            .filter_map(|h| match h.status {
                pw_remote::RemoteStatus::Connected { machine } => Some(machine),
                _ => None,
            })
            .collect();
        listening.retain(|machine, task| {
            let keep = connected.contains(machine) && !task.is_finished();
            if !keep {
                task.abort();
            }
            keep
        });
        for machine in connected {
            if listening.contains_key(&machine) {
                continue;
            }
            let Ok(host) = router.host(&machine) else {
                continue;
            };
            let task = tokio::spawn(listen(
                machine.clone(),
                host,
                registry.clone(),
                Arc::clone(&input),
                Arc::clone(&windows),
            ));
            listening.insert(machine, task);
        }
        tokio::time::sleep(FOLLOW_EVERY).await;
    }
}

/// 앱의 묶음 창.
pub struct AppWindows<R: tauri::Runtime>(pub tauri::AppHandle<R>);

impl<R: tauri::Runtime> Windows for AppWindows<R> {
    fn open(&self, planned: &Planned) -> Result<OpenedIn, String> {
        let created = match planned.bundle.local_project() {
            // 로컬 멤버가 있는 묶음은 그 프로젝트의 길(신뢰 관문 · 창 연결 묶기 — launch.rs)로.
            Some(project) if planned.bundle.implicit => crate::launch::open_project_as(
                &self.0,
                project,
                &planned.open,
                crate::windows::Activation::Background,
            )?,
            _ => {
                crate::windows::open_project_as(
                    &self.0,
                    &planned.bundle.id,
                    &planned.bundle.name,
                    &planned.open,
                    crate::windows::Activation::Background,
                )
                .map_err(|e| e.to_string())?
                .1
            }
        };
        Ok(if created {
            OpenedIn::NewWindow
        } else {
            OpenedIn::ExistingWindow
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(path: Option<&str>, worktree: Option<&str>) -> RemoteOpen {
        RemoteOpen {
            project_id: "rp1".into(),
            worktree: worktree.map(str::to_owned),
            path: path.map(str::to_owned),
            line: Some(7),
            column: Some(2),
        }
    }

    fn bundle() -> ProjectBundle {
        ProjectBundle {
            id: "b-1".into(),
            name: "pitwall".into(),
            favorite: false,
            tags: Vec::new(),
            opened_at: None,
            implicit: false,
            members: vec![pw_core::registry::bundle::ProjectBundleMember {
                machine: "mini-id".into(),
                project_id: "rp1".into(),
                last_target: None,
            }],
        }
    }

    /// 원격의 열기 요청은 그 묶음 창에 **활성 체크아웃을 바꾸지 않는** 요청이 된다 — `worktree`(활성 워크트리를 바꾸는 칸)는 늘
    /// 비고, 파일이 든 체크아웃은 `checkout` 으로 간다(18-m7-plan M7-15 시험 ③).
    #[test]
    fn a_remote_open_does_not_change_the_active_checkout() {
        let planned = plan_in(
            Some(bundle()),
            "mini-id",
            &request(Some("/home/me/wt/README.md"), Some("/home/me/wt")),
            Some("/home/me/pitwall"),
        )
        .unwrap();
        assert_eq!(planned.bundle.id, "b-1");
        assert_eq!(
            planned.open,
            ProjectOpen {
                worktree: None,
                file: Some("/home/me/wt/README.md".into()),
                line: Some(7),
                col: Some(2),
                checkout: Some(CheckoutRef {
                    machine: "mini-id".into(),
                    worktree: Some("/home/me/wt".into()),
                }),
            }
        );
        let main = plan_in(
            Some(bundle()),
            "mini-id",
            &request(Some("/home/me/pitwall/a.rs"), None),
            Some("/home/me/pitwall"),
        )
        .unwrap();
        assert_eq!(main.open.worktree, None);
        assert_eq!(main.open.checkout.unwrap().worktree, None);
    }

    /// 묶음에 없는 프로젝트 · 다른 머신 · 그 체크아웃 밖의 경로(`..` 포함)는 거절한다(메인 판정 M7-P19).
    #[test]
    fn opens_outside_a_bundle_member_are_refused() {
        let root = Some("/home/me/pitwall");
        for (machine, req) in [
            ("other-id", request(Some("/home/me/pitwall/a"), None)),
            (
                "mini-id",
                RemoteOpen {
                    project_id: "nope".into(),
                    ..request(None, None)
                },
            ),
            ("mini-id", request(Some("/etc/passwd"), None)),
            (
                "mini-id",
                request(Some("/home/me/pitwall/../../etc/passwd"), None),
            ),
            ("mini-id", request(Some("relative"), None)),
        ] {
            assert!(
                plan_in(Some(bundle()), machine, &req, root).is_err(),
                "{machine} {req:?}"
            );
        }
        assert!(plan_in(None, "mini-id", &request(None, None), root).is_err());
        assert!(
            plan_in(
                Some(bundle()),
                "mini-id",
                &request(Some("/home/me/pitwall/a"), None),
                None
            )
            .is_err()
        );
    }

    #[test]
    fn idle_time_counts_from_the_last_input() {
        let input = LastInput::default();
        assert_eq!(input.idle_ms(), NEVER);
        input.note();
        assert!(input.idle_ms() < 1_000);
    }
}
