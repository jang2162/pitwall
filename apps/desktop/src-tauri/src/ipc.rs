//! Tauri IPC transport: 웹뷰의 JSON-RPC 메시지를 Host 연결로 실어 나른다.
//!
//! - `rpc_page()`: 이 웹뷰의 현재 페이지 세대. 프런트엔드는 문서가 뜰 때 한 번 받아 둔다.
//! - `rpc_connect({ target, channel, page })`: [`HostRouter`] 로 Host 를 골라 연결을 열고 연결 id 를 돌려준다.
//!   Host 가 보내는 메시지(응답 · 스트림 청크 · 이벤트)는 `channel` 로 온다. 연결이 닫히면 마지막에
//!   [`CLOSED_FRAME`] 하나가 온다.
//! - `rpc_send({ conn, frame })`: JSON-RPC 메시지 하나를 그 연결로 보낸다.
//! - `rpc_disconnect({ conn })`: 연결을 닫는다.
//!
//! 연결은 그것을 연 웹뷰에 묶인다. 다른 웹뷰는 그 id 로 보내거나 닫을 수 없고, 웹뷰가 닫히거나 새로 로드되면
//! 그 웹뷰의 연결을 모두 닫는다 — 연결을 버리면 Host 가 진행 중 요청 · 스트림을 취소하고 구독을 걷는다
//! ([`pw_core::FrameSink`] 계약).
//!
//! 로컬 창은 데스크톱 사용자 본인이므로 호출자 role 은 owner 다.
//!
//! 리로드와의 경합은 페이지 세대로 막는다. 웹뷰마다 세대를 두고 `PageLoadEvent::Started` 마다 새 값으로 바꾼다.
//! Started 는 새 문서가 **커밋된** 시점이다(wry 가 세 백엔드 모두 커밋에 건다 — WKWebView `didCommitNavigation`,
//! WebView2 `ContentLoading`, WebKitGTK `Committed`). 그래서 새 문서의 JS 가 `rpc_page` 로 받는 세대는 언제나 새 값이고,
//! 옛 문서가 들고 있던 세대는 낡았다. invoke 는 어느 문서에서 왔는지 알려 주지 않으므로, 문서가 받은 세대를
//! `rpc_connect` 에 실어 보내게 하고, 등록할 때 그 세대가 아직 현재이고 창이 살아 있을 때만 받는다.
//! 커밋 직전에 옛 문서가 보낸 `rpc_connect` 가 Started 뒤에 처리돼도 낡은 세대라 거절된다 — 옛 문서는 응답을 받지 못해
//! 스스로 닫지 못하므로 등록되면 다음 리로드까지 유휴 연결로 남는다.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use pw_core::{FrameSink, HostError, HostRouter, Target};
use pw_rpc::{Caller, Role};
use serde::Serialize;
use serde_json::Value;
use tauri::ipc::Channel;
use tauri::webview::PageLoadEvent;
use tauri::{Builder, Manager, Runtime, State, Webview, Window, WindowEvent};

pub type ConnId = u64;

/// 연결이 닫혔음을 알리는 채널의 마지막 메시지. JSON-RPC 메시지가 아니다 — 프런트엔드 transport 가 가로채
/// `onClose` 로 바꾼다(`src/tauri-transport.ts`). Host 쪽이 먼저 닫는 경우(원격 Host 끊김 등)에도 대기 중 호출이
/// 다음 `send` 실패까지 걸려 있지 않게 한다.
pub const CLOSED_FRAME: &str = r#"{"pitwallTransport":"closed"}"#;

/// 웹뷰 label → 그 웹뷰의 현재 페이지와 그 페이지가 연 연결들.
type ConnTable = HashMap<String, PageConns>;

struct PageConns {
    /// 페이지 세대. 다시 로드될 때마다 앱 전체에서 유일한 새 값을 받는다.
    page: u64,
    conns: HashMap<ConnId, Box<dyn FrameSink>>,
}

pub struct IpcState {
    router: Arc<HostRouter>,
    next_conn: AtomicU64,
    next_page: AtomicU64,
    conns: Mutex<ConnTable>,
}

impl IpcState {
    pub fn new(router: Arc<HostRouter>) -> Self {
        Self {
            router,
            next_conn: AtomicU64::new(1),
            next_page: AtomicU64::new(1),
            conns: Mutex::default(),
        }
    }

    fn conns(&self) -> MutexGuard<'_, ConnTable> {
        self.conns.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn new_page(&self) -> PageConns {
        PageConns {
            page: self.next_page.fetch_add(1, Ordering::Relaxed),
            conns: HashMap::new(),
        }
    }

    /// 이 웹뷰의 현재 페이지 세대. 처음 보는 웹뷰면 세대를 새로 매긴다.
    fn current_page(&self, label: &str) -> u64 {
        let mut table = self.conns();
        table
            .entry(label.to_owned())
            .or_insert_with(|| self.new_page())
            .page
    }

    /// 웹뷰에 새 페이지가 로드되기 시작했다. 옛 페이지의 연결을 모두 닫고 세대를 바꾼다. 닫은 수를 돌려준다.
    pub fn page_reloaded(&self, label: &str) -> usize {
        // 잠금 밖에서 버린다. 연결을 버리면 Host 쪽 정리(세션 close)가 돈다.
        let removed = {
            let mut table = self.conns();
            std::mem::replace(
                table
                    .entry(label.to_owned())
                    .or_insert_with(|| self.new_page()),
                self.new_page(),
            )
        };
        removed.conns.len()
    }

    /// 웹뷰가 사라졌다. 연결을 모두 닫는다. 닫은 수를 돌려준다.
    pub fn close_webview(&self, label: &str) -> usize {
        let removed = self.conns().remove(label);
        removed.map_or(0, |page| page.conns.len())
    }

    /// 연결을 등록한다. 연결을 여는 사이 페이지가 바뀌었거나(`page` 가 낡음) 웹뷰가 사라졌으면(`alive` 가 거짓)
    /// 등록하지 않는다 — 그때 `sink` 는 버려져 연결이 닫힌다.
    fn register(
        &self,
        label: &str,
        page: u64,
        alive: bool,
        id: ConnId,
        sink: Box<dyn FrameSink>,
    ) -> bool {
        let mut table = self.conns();
        if !alive {
            table.remove(label);
            return false;
        }
        match table.get_mut(label) {
            Some(current) if current.page == page => {
                current.conns.insert(id, sink);
                true
            }
            _ => false,
        }
    }

    #[cfg(test)]
    pub fn connection_count(&self, label: &str) -> usize {
        self.conns().get(label).map_or(0, |page| page.conns.len())
    }
}

#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum IpcError {
    /// 라우팅 · 연결 실패.
    Host { error: HostError },
    /// 이 웹뷰가 연 연결이 아니거나 이미 닫혔다.
    UnknownConnection { conn: ConnId },
    /// 연결을 여는 사이 웹뷰가 다시 로드되거나 닫혔다. 연결은 닫았다.
    PageGone,
}

/// transport 를 앱에 붙인다: 상태, command, 웹뷰가 닫히거나 다시 로드될 때의 정리.
pub fn install<R: Runtime>(builder: Builder<R>, router: Arc<HostRouter>) -> Builder<R> {
    builder
        .manage(IpcState::new(router))
        // Tauri 는 invoke handler 를 하나만 받으므로 앱의 다른 command 도 여기에 둔다.
        .invoke_handler(tauri::generate_handler![
            rpc_page,
            rpc_connect,
            rpc_send,
            rpc_disconnect,
            crate::menu::keymap_sync_menu,
            crate::windows::open_settings_window,
            crate::launch::open_project_window,
            crate::launch::take_pending_find_project,
            crate::launch::path_kind,
            crate::windows::take_pending_project_opens,
            crate::shell_command::install_shell_command,
            crate::clipboard::write_clipboard,
            crate::perf::perf_first_frame,
            crate::perf::perf_hidden
        ])
        .on_window_event(on_window_event)
        .on_page_load(|webview, payload| on_page_load(webview, payload.event()))
}

/// 새로고침하면 프론트엔드는 연결 id 와 채널을 잃는다. 남은 연결은 아무도 닫지 못하므로 새 페이지가 시작될 때 닫는다.
pub fn on_page_load<R: Runtime>(webview: &Webview<R>, event: PageLoadEvent) {
    if event == PageLoadEvent::Started {
        webview.state::<IpcState>().page_reloaded(webview.label());
    }
}

/// 창이 사라지면 그 창의 연결을 닫는다. 창마다 웹뷰가 하나라 창 label 이 곧 웹뷰 label 이다.
pub fn on_window_event<R: Runtime>(window: &Window<R>, event: &WindowEvent) {
    if matches!(event, WindowEvent::Destroyed) {
        window.state::<IpcState>().close_webview(window.label());
    }
}

// Session 이 메시지를 처리하며 tokio 태스크를 띄우므로 command 는 async 로 둔다 — Tauri 의 async command 는
// tokio 런타임 위에서 돌고, 동기 command 는 메인 스레드에서 돌아 런타임이 없다.
#[tauri::command]
async fn rpc_page<R: Runtime>(webview: Webview<R>, state: State<'_, IpcState>) -> Result<u64, ()> {
    Ok(state.current_page(webview.label()))
}

/// `page` 는 이 문서가 `rpc_page` 로 받은 세대다. 지금 세대가 아니면 연결을 닫고 `PageGone` 을 돌려준다.
#[tauri::command]
async fn rpc_connect<R: Runtime>(
    webview: Webview<R>,
    state: State<'_, IpcState>,
    target: Target,
    channel: Channel<Value>,
    page: u64,
) -> Result<ConnId, IpcError> {
    let label = webview.label().to_owned();
    let conn = state
        .router
        .connect(&target, Caller::new(Role::Owner))
        .await
        .map_err(|error| IpcError::Host { error })?;
    let id = state.next_conn.fetch_add(1, Ordering::Relaxed);
    // Tauri 는 Destroyed 를 창별 훅에 넘기기 전에 매니저에서 창을 지운다. 그래서 이 확인과 훅 중 어느 쪽이 먼저든
    // 닫힌 창의 연결은 남지 않는다.
    let alive = webview.app_handle().get_webview_window(&label).is_some();
    if !state.register(&label, page, alive, id, conn.sink) {
        return Err(IpcError::PageGone);
    }
    let mut source = conn.source;
    tauri::async_runtime::spawn(async move {
        // 연결이 닫히면 Host 가 남은 메시지(취소된 스트림의 stream.end 등)를 보낸 뒤 None 을 돌려준다.
        while let Some(frame) = source.recv().await {
            if channel.send(frame).is_err() {
                return;
            }
        }
        let closed: Value = serde_json::from_str(CLOSED_FRAME).expect("CLOSED_FRAME is JSON");
        let _ = channel.send(closed);
    });
    Ok(id)
}

#[tauri::command]
async fn rpc_send<R: Runtime>(
    webview: Webview<R>,
    state: State<'_, IpcState>,
    conn: ConnId,
    frame: Value,
) -> Result<(), IpcError> {
    let conns = state.conns();
    let sink = conns
        .get(webview.label())
        .and_then(|page| page.conns.get(&conn))
        .ok_or(IpcError::UnknownConnection { conn })?;
    sink.send(frame);
    Ok(())
}

#[tauri::command]
async fn rpc_disconnect<R: Runtime>(
    webview: Webview<R>,
    state: State<'_, IpcState>,
    conn: ConnId,
) -> Result<(), IpcError> {
    let removed = state
        .conns()
        .get_mut(webview.label())
        .and_then(|page| page.conns.remove(&conn));
    match removed {
        Some(_) => Ok(()),
        None => Err(IpcError::UnknownConnection { conn }),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::time::Duration;

    use pw_core::methods::{DemoTaskUpdate, Pong};
    use pw_core::task::{TOPIC_FINISHED, TaskFinishedEvent};
    use pw_core::{Filter, LocalHost, Received, Scope, Subscription, TaskStatus};
    use pw_rpc::protocol::StreamOpened;
    use serde_json::json;
    use tauri::ipc::{CallbackFn, InvokeBody, InvokeResponseBody};
    use tauri::test::{
        INVOKE_KEY, MockRuntime, get_ipc_response, mock_builder, mock_context, noop_assets,
    };
    use tauri::webview::InvokeRequest;
    use tauri::{App, WebviewWindow, WebviewWindowBuilder};

    use super::*;

    struct Fixture {
        app: App<MockRuntime>,
        host: Arc<LocalHost>,
        /// 채널로 나간 메시지: (채널 id, JSON).
        frames: mpsc::Receiver<(u32, Value)>,
    }

    fn fixture() -> Fixture {
        let host = Arc::new(LocalHost::new("local"));
        let router = Arc::new(HostRouter::new());
        router.add_host(host.clone()).unwrap();
        router.bind_project("p1".into(), "local").unwrap();
        let (tx, frames) = mpsc::channel();
        // mock 웹뷰에는 JS 가 없다. 채널로 나가는 메시지를 가로채 테스트로 돌린다.
        let builder =
            mock_builder().channel_interceptor(move |_webview, callback, _index, body| {
                if let InvokeResponseBody::Json(json) = body {
                    let _ = tx.send((callback.0, serde_json::from_str(json).unwrap()));
                }
                true
            });
        let app = install(builder, router)
            .build(mock_context(noop_assets()))
            .unwrap();
        Fixture { app, host, frames }
    }

    fn window(app: &App<MockRuntime>, label: &str) -> WebviewWindow<MockRuntime> {
        WebviewWindowBuilder::new(app, label, Default::default())
            .build()
            .unwrap()
    }

    fn invoke(
        webview: &WebviewWindow<MockRuntime>,
        cmd: &str,
        args: Value,
    ) -> Result<Value, Value> {
        get_ipc_response(
            webview,
            InvokeRequest {
                cmd: cmd.into(),
                callback: CallbackFn(0),
                error: CallbackFn(1),
                // 앱 페이지의 주소. Windows(WebView2)는 커스텀 프로토콜을 `http://tauri.localhost` 로 연다 — 다른 주소는 원격 페이지로
                // 보아 명령 권한이 없다(`rpc_page not allowed`). Tauri `test` 모듈 문서와 같은 분기다.
                url: if cfg!(any(windows, target_os = "android")) {
                    "http://tauri.localhost"
                } else {
                    "tauri://localhost"
                }
                .parse()
                .unwrap(),
                body: InvokeBody::Json(args),
                headers: Default::default(),
                invoke_key: INVOKE_KEY.to_string(),
            },
        )
        .map(|body| body.deserialize::<Value>().unwrap())
    }

    fn connect(webview: &WebviewWindow<MockRuntime>, channel: u32, target: Value) -> ConnId {
        let page = invoke(webview, "rpc_page", json!({})).unwrap();
        let id = invoke(
            webview,
            "rpc_connect",
            json!({"target": target, "channel": format!("__CHANNEL__:{channel}"), "page": page}),
        )
        .unwrap();
        serde_json::from_value(id).unwrap()
    }

    fn send(
        webview: &WebviewWindow<MockRuntime>,
        conn: ConnId,
        frame: Value,
    ) -> Result<Value, Value> {
        invoke(webview, "rpc_send", json!({"conn": conn, "frame": frame}))
    }

    fn next_frame(f: &Fixture, channel: u32) -> Value {
        loop {
            let (id, frame) = f
                .frames
                .recv_timeout(Duration::from_secs(5))
                .expect("no frame on the channel");
            if id == channel {
                return frame;
            }
        }
    }

    fn finished_events(host: &LocalHost) -> Subscription {
        host.bus()
            .subscribe(Filter::all().scope(Scope::Host).topics([TOPIC_FINISHED]))
    }

    fn wait_finished(sub: &mut Subscription) -> TaskStatus {
        tauri::async_runtime::block_on(async {
            match tokio_timeout(sub.recv()).await {
                Some(Received::Event(e)) => {
                    serde_json::from_value::<TaskFinishedEvent>(e.payload.clone())
                        .unwrap()
                        .status
                }
                other => panic!("expected a finished event, got {other:?}"),
            }
        })
    }

    async fn tokio_timeout<F: std::future::Future>(fut: F) -> F::Output {
        tokio::time::timeout(Duration::from_secs(5), fut)
            .await
            .expect("timed out")
    }

    /// 데모 작업을 열고 첫 진행 청크까지 받는다. 스트림 id 를 돌려준다.
    fn start_demo(
        f: &Fixture,
        webview: &WebviewWindow<MockRuntime>,
        conn: ConnId,
        channel: u32,
    ) -> u64 {
        send(
            webview,
            conn,
            json!({"jsonrpc": "2.0", "id": 1, "method": "host.demoTask",
                   "params": {"steps": 1000, "intervalMs": 10}}),
        )
        .unwrap();
        let opened: StreamOpened =
            serde_json::from_value(next_frame(f, channel)["result"].clone()).unwrap();
        send(
            webview,
            conn,
            json!({"jsonrpc": "2.0", "method": "stream.credit",
                   "params": {"streamId": opened.stream_id, "credit": 1}}),
        )
        .unwrap();
        let chunk = next_frame(f, channel);
        assert_eq!(chunk["method"], "stream.chunk");
        assert!(matches!(
            serde_json::from_value(chunk["params"]["data"].clone()).unwrap(),
            DemoTaskUpdate::Progress { .. }
        ));
        opened.stream_id
    }

    #[test]
    fn ping_round_trips_through_ipc() {
        let f = fixture();
        let welcome = window(&f.app, "welcome");
        let conn = connect(&welcome, 7, json!({"hostId": "local"}));

        send(
            &welcome,
            conn,
            json!({"jsonrpc": "2.0", "id": 1, "method": "host.ping", "params": {"nonce": "n1"}}),
        )
        .unwrap();
        let frame = next_frame(&f, 7);
        assert_eq!(frame["id"], 1);
        let pong: Pong = serde_json::from_value(frame["result"].clone()).unwrap();
        assert_eq!(pong.nonce, "n1");
        assert_eq!(pong.host_id, "local");
    }

    #[test]
    fn connecting_by_project_routes_and_unknown_project_is_an_error() {
        let f = fixture();
        let w = window(&f.app, "project-1");
        connect(&w, 1, json!({"projectId": "p1"}));
        let err = invoke(
            &w,
            "rpc_connect",
            json!({"target": {"projectId": "nope"}, "channel": "__CHANNEL__:2", "page": 1}),
        )
        .unwrap_err();
        assert_eq!(
            err,
            json!({"kind": "host", "error": {"kind": "unknownProject", "id": "nope"}})
        );
        assert_eq!(f.app.state::<IpcState>().connection_count("project-1"), 1);
    }

    #[test]
    fn a_window_cannot_use_another_windows_connection() {
        let f = fixture();
        let a = window(&f.app, "welcome");
        let b = window(&f.app, "settings");
        let conn = connect(&a, 1, json!({"hostId": "local"}));
        let frame = json!({"jsonrpc": "2.0", "id": 1, "method": "host.ping"});
        assert_eq!(
            send(&b, conn, frame).unwrap_err(),
            json!({"kind": "unknownConnection", "conn": conn})
        );
        assert!(invoke(&b, "rpc_disconnect", json!({"conn": conn})).is_err());
        assert_eq!(f.app.state::<IpcState>().connection_count("welcome"), 1);
    }

    #[test]
    fn disconnect_closes_the_connection_and_cancels_its_streams() {
        let f = fixture();
        let w = window(&f.app, "welcome");
        let conn = connect(&w, 3, json!({"hostId": "local"}));
        let mut finished = finished_events(&f.host);
        start_demo(&f, &w, conn, 3);

        invoke(&w, "rpc_disconnect", json!({"conn": conn})).unwrap();
        assert_eq!(wait_finished(&mut finished), TaskStatus::Cancelled);
        assert!(send(&w, conn, json!({})).is_err());
    }

    #[test]
    fn closing_a_window_closes_its_connections_and_cancels_their_work() {
        let f = fixture();
        let welcome = window(&f.app, "welcome");
        let other = window(&f.app, "settings");
        let conn = connect(&welcome, 5, json!({"hostId": "local"}));
        connect(&welcome, 6, json!({"hostId": "local"}));
        connect(&other, 9, json!({"hostId": "local"}));
        let mut finished = finished_events(&f.host);
        let stream_id = start_demo(&f, &welcome, conn, 5);

        // mock 런타임은 창을 없애도 Destroyed 를 내지 않는다. 앱에 건 것과 같은 훅을 직접 부른다.
        on_window_event(&welcome.as_ref().window(), &WindowEvent::Destroyed);

        let state = f.app.state::<IpcState>();
        assert_eq!(state.connection_count("welcome"), 0);
        assert_eq!(state.connection_count("settings"), 1);
        assert_eq!(wait_finished(&mut finished), TaskStatus::Cancelled);
        // 닫힌 연결의 스트림은 cancelled 로 끝났다고 웹뷰 쪽에 알린다.
        let end = loop {
            let frame = next_frame(&f, 5);
            if frame["method"] == "stream.end" {
                break frame;
            }
        };
        assert_eq!(end["params"]["streamId"], stream_id);
        assert_eq!(end["params"]["status"], "cancelled");
    }

    #[test]
    fn page_reload_closes_the_pages_connections_and_cancels_their_work() {
        let f = fixture();
        let w = window(&f.app, "welcome");
        let conn = connect(&w, 4, json!({"hostId": "local"}));
        let mut finished = finished_events(&f.host);
        start_demo(&f, &w, conn, 4);

        // 로드가 끝난 것(Finished)으로는 닫지 않는다.
        on_page_load(w.as_ref(), PageLoadEvent::Finished);
        assert_eq!(f.app.state::<IpcState>().connection_count("welcome"), 1);

        // mock 런타임은 페이지 로드 이벤트를 내지 않는다. 앱에 건 것과 같은 훅을 직접 부른다.
        on_page_load(w.as_ref(), PageLoadEvent::Started);
        assert_eq!(f.app.state::<IpcState>().connection_count("welcome"), 0);
        assert_eq!(wait_finished(&mut finished), TaskStatus::Cancelled);
        // Host 쪽에서 닫힌 연결의 채널은 종료 표시로 끝난다.
        let closed: Value = serde_json::from_str(CLOSED_FRAME).unwrap();
        while next_frame(&f, 4) != closed {}
        // 새 페이지는 다시 연결할 수 있다.
        connect(&w, 5, json!({"hostId": "local"}));
        assert_eq!(f.app.state::<IpcState>().connection_count("welcome"), 1);
    }

    /// 버려지면 표시하는 sink. 등록되지 않은 연결이 실제로 닫히는지(버려지는지) 본다.
    struct DropFlag(Arc<std::sync::atomic::AtomicBool>);

    impl FrameSink for DropFlag {
        fn send(&self, _frame: Value) {}
    }

    impl Drop for DropFlag {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    fn flag() -> (Arc<std::sync::atomic::AtomicBool>, Box<dyn FrameSink>) {
        let dropped = Arc::new(std::sync::atomic::AtomicBool::new(false));
        (dropped.clone(), Box::new(DropFlag(dropped)))
    }

    #[test]
    fn a_connect_that_races_a_reload_is_closed_not_registered() {
        let f = fixture();
        let w = window(&f.app, "welcome");
        let state = f.app.state::<IpcState>();
        // rpc_connect 가 연결을 여는 동안(await) 페이지가 다시 로드된 경우.
        let page = state.current_page("welcome");
        on_page_load(w.as_ref(), PageLoadEvent::Started);
        let (dropped, sink) = flag();
        assert!(!state.register("welcome", page, true, 100, sink));
        assert!(dropped.load(Ordering::SeqCst));
        assert_eq!(state.connection_count("welcome"), 0);

        // 세대가 그대로면 등록된다.
        let page = state.current_page("welcome");
        let (dropped, sink) = flag();
        assert!(state.register("welcome", page, true, 101, sink));
        assert!(!dropped.load(Ordering::SeqCst));
        assert_eq!(state.connection_count("welcome"), 1);
    }

    #[test]
    fn a_connect_that_races_a_window_close_is_closed_not_registered() {
        let f = fixture();
        let state = f.app.state::<IpcState>();
        let page = state.current_page("gone");
        let (dropped, sink) = flag();
        assert!(!state.register("gone", page, false, 1, sink));
        assert!(dropped.load(Ordering::SeqCst));
        assert_eq!(state.connection_count("gone"), 0);
        assert!(state.conns().get("gone").is_none());
    }

    /// 리뷰 재현: 옛 문서가 커밋 직전에 보낸 `rpc_connect` 가 Started 뒤에 처리되는 경우. 옛 문서는 응답을 받지 못해 닫을
    /// 수도 없으므로, 그 연결은 등록되면 안 된다. 옛 문서는 자기가 받은 세대를 실어 보낸다.
    #[test]
    fn a_connect_sent_by_the_old_page_is_refused_after_the_reload() {
        let f = fixture();
        let w = window(&f.app, "welcome");
        let state = f.app.state::<IpcState>();
        let old_page = state.current_page("welcome");
        on_page_load(w.as_ref(), PageLoadEvent::Started);

        let err = invoke(
            &w,
            "rpc_connect",
            json!({"target": {"hostId": "local"}, "channel": "__CHANNEL__:8", "page": old_page}),
        )
        .unwrap_err();
        assert_eq!(err, json!({"kind": "pageGone"}));
        assert_eq!(state.connection_count("welcome"), 0);
    }

    #[test]
    fn rpc_page_hands_out_a_new_generation_after_each_reload() {
        let f = fixture();
        let w = window(&f.app, "welcome");
        let first = invoke(&w, "rpc_page", json!({})).unwrap();
        assert_eq!(invoke(&w, "rpc_page", json!({})).unwrap(), first);
        on_page_load(w.as_ref(), PageLoadEvent::Started);
        let second = invoke(&w, "rpc_page", json!({})).unwrap();
        assert_ne!(second, first);
        // 새 문서는 새 세대로 연결할 수 있다.
        connect(&w, 3, json!({"hostId": "local"}));
        assert_eq!(f.app.state::<IpcState>().connection_count("welcome"), 1);
    }
}
