//! 테스트 공용: 시험용 메서드 선언과 in-memory 세션 하네스.

#![allow(dead_code)] // 테스트 파일마다 쓰는 도우미가 다르다.

use std::sync::Arc;
use std::time::Duration;

use pw_rpc::{Caller, EventHub, Outbound, Response, Role, Router, RpcError, Session, rpc_method};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::mpsc::UnboundedReceiver;

#[derive(Debug, Serialize, Deserialize, PartialEq, specta::Type, schemars::JsonSchema)]
pub struct EchoParams {
    pub text: String,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, specta::Type, schemars::JsonSchema)]
pub struct CountParams {
    pub n: u32,
    /// 이 번호의 청크를 보낸 뒤 오류로 끝낸다.
    #[serde(default)]
    pub fail_after: Option<u32>,
}

rpc_method! {
    /// 입력을 그대로 돌려준다.
    pub Echo: "test.echo" {
        params: EchoParams,
        result: EchoParams,
        tier: Read,
    }

    /// 설정을 쓴다. owner 만.
    pub WriteSetting: "test.writeSetting" {
        params: (),
        result: (),
        tier: Write,
    }

    /// mobile 에게도 허용한 조작.
    pub Restart: "test.restart" {
        params: (),
        result: (),
        tier: Exec,
        roles: [Owner, Mobile],
    }

    /// 0 부터 n-1 까지 청크로 보낸다.
    pub Count: "test.count" {
        params: CountParams,
        stream: u32,
        tier: Read,
    }

    /// owner 전용 스트림.
    pub DangerStream: "test.dangerStream" {
        params: (),
        stream: u32,
        tier: Dangerous,
    }

    /// 애플리케이션 오류를 돌려준다.
    pub Fail: "test.fail" {
        params: (),
        result: (),
        tier: Read,
    }

    /// 패닉한다.
    pub Panic: "test.panic" {
        params: (),
        result: (),
        tier: Read,
    }
}

pub const APP_ERROR: i32 = 1001;

/// 시험용 메서드 전부를 등록한 라우터. 테스트별 핸들러를 더 붙일 수 있게 `Router` 로 돌려준다.
pub fn base_router() -> Router {
    let mut router = Router::new();
    router
        .unary(Echo, |_ctx, p: EchoParams| async move { Ok(p) })
        .unary(WriteSetting, |_ctx, ()| async { Ok(()) })
        .unary(Restart, |_ctx, ()| async { Ok(()) })
        .unary(Fail, |_ctx, ()| async {
            Err(RpcError::new(APP_ERROR, "boom"))
        })
        .unary(Panic, |_ctx, ()| async { panic!("handler panic for test") })
        .stream(Count, |_ctx, p: CountParams, sink| async move {
            for i in 0..p.n {
                sink.send(i).await?;
                if p.fail_after == Some(i) {
                    return Err(RpcError::new(APP_ERROR, "stream boom"));
                }
            }
            Ok(())
        })
        .stream(
            DangerStream,
            |_ctx, (), sink| async move { sink.send(0).await },
        );
    router
}

pub struct Harness {
    pub session: Session,
    pub rx: UnboundedReceiver<Outbound>,
    pub events: EventHub,
}

impl Harness {
    pub fn new(router: Router, role: Role) -> Self {
        Self::with_events(Arc::new(router), EventHub::new(), role)
    }

    pub fn with_events(router: Arc<Router>, events: EventHub, role: Role) -> Self {
        let (session, rx) = Session::new(router, events.clone(), Caller::new(role));
        Self {
            session,
            rx,
            events,
        }
    }

    pub fn request(&self, id: i64, method: &str, params: Value) {
        self.session
            .handle_value(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
    }

    pub fn notify(&self, method: &str, params: Value) {
        self.session
            .handle_value(json!({"jsonrpc": "2.0", "method": method, "params": params}));
    }

    /// 다음 메시지. 1초 안에 오지 않으면 실패.
    pub async fn next(&mut self) -> Outbound {
        tokio::time::timeout(Duration::from_secs(1), self.rx.recv())
            .await
            .expect("timed out waiting for an outbound message")
            .expect("outbound channel closed")
    }

    pub async fn next_response(&mut self) -> Response {
        let msg = self.next().await;
        msg.clone()
            .into_response()
            .unwrap_or_else(|| panic!("expected a response, got {msg:?}"))
    }

    pub async fn next_notification<T: serde::de::DeserializeOwned>(&mut self, method: &str) -> T {
        let msg = self.next().await;
        let note = msg
            .clone()
            .into_notification()
            .unwrap_or_else(|| panic!("expected a notification, got {msg:?}"));
        assert_eq!(note.method, method, "{msg:?}");
        note.params_as().expect("notification params")
    }

    /// 지금 내보낼 메시지가 없음을 확인한다.
    ///
    /// `start_paused` 테스트에서는 모든 태스크가 멈춘 뒤에야 시간이 흐르므로, 핸들러가 보낼 수 있는 것은
    /// 다 보낸 다음에 판정된다 — 시간 여유에 기대지 않는다.
    pub async fn assert_quiet(&mut self) {
        let got = tokio::time::timeout(Duration::from_millis(50), self.rx.recv()).await;
        assert!(got.is_err(), "expected no message, got {got:?}");
    }
}

pub fn err_code(response: &Response) -> i32 {
    response
        .outcome
        .as_ref()
        .expect_err("expected an error response")
        .code
}
