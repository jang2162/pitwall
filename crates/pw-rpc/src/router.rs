//! 메서드 레지스트리. 선언한 메서드에 핸들러를 붙이고, 등록된 메서드를 열거한다.

use std::collections::HashMap;
use std::future::Future;
use std::marker::PhantomData;
use std::pin::Pin;
use std::sync::Arc;

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::sync::Semaphore;
use tokio::sync::mpsc::UnboundedSender;
use tokio_util::sync::CancellationToken;

use crate::error::RpcError;
use crate::events::EventHub;
use crate::message::{Outbound, notification};
use crate::method::{MethodMeta, Role, StreamMethod, UnaryMethod};
use crate::protocol::{STREAM_CHUNK, StreamChunk, StreamId};

pub(crate) type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;
pub(crate) type UnaryFn =
    dyn Fn(Ctx, Value) -> Result<BoxFuture<Result<Value, RpcError>>, RpcError> + Send + Sync;
pub(crate) type StreamFn =
    dyn Fn(Ctx, Value, RawSink) -> Result<BoxFuture<Result<(), RpcError>>, RpcError> + Send + Sync;

pub(crate) enum Handler {
    Unary(Box<UnaryFn>),
    Stream(Box<StreamFn>),
}

pub(crate) struct Entry {
    pub meta: MethodMeta,
    pub handler: Handler,
}

/// 호출자. transport 가 연결을 인증한 뒤 세션을 만들 때 정한다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Caller {
    pub role: Role,
}

impl Caller {
    pub fn new(role: Role) -> Self {
        Self { role }
    }
}

/// 핸들러가 받는 호출 문맥.
#[derive(Clone)]
pub struct Ctx {
    pub(crate) caller: Caller,
    pub(crate) cancel: CancellationToken,
    pub(crate) events: EventHub,
}

impl Ctx {
    pub fn caller(&self) -> Caller {
        self.caller
    }

    /// 이 호출의 취소 토큰.
    ///
    /// 핸들러의 future 는 취소되면 다음 `.await` 지점에서 버려진다. 그러므로 `.await` 없이 오래 도는 일
    /// (`spawn_blocking` 안의 작업 등)만 이 토큰을 직접 확인하면 된다.
    pub fn cancellation(&self) -> &CancellationToken {
        &self.cancel
    }

    pub fn events(&self) -> &EventHub {
        &self.events
    }
}

pub(crate) struct RawSink {
    pub stream_id: StreamId,
    pub credit: Arc<Semaphore>,
    pub out: UnboundedSender<Outbound>,
}

/// 스트림 핸들러가 청크를 보내는 곳.
pub struct StreamSink<T> {
    raw: RawSink,
    _item: PhantomData<fn(T)>,
}

impl<T: Serialize> StreamSink<T> {
    pub fn stream_id(&self) -> StreamId {
        self.raw.stream_id
    }

    /// credit 이 생길 때까지 기다린 뒤 청크 하나를 보내고 credit 1 을 쓴다.
    ///
    /// 백프레셔는 이 대기 하나로 걸린다 — 클라이언트가 credit 을 주지 않으면 핸들러가 여기서 멈춘다.
    pub async fn send(&self, item: T) -> Result<(), RpcError> {
        // 직렬화를 credit 보다 먼저 한다. 보내지 못할 항목이 credit 을 쓰지 않게.
        let data = serde_json::to_value(item).map_err(|e| RpcError::internal(e.to_string()))?;
        let permit = self
            .raw
            .credit
            .acquire()
            .await
            .map_err(|_| RpcError::cancelled())?;
        permit.forget();
        let chunk = StreamChunk {
            stream_id: self.raw.stream_id,
            data,
        };
        self.raw
            .out
            .send(notification(STREAM_CHUNK, &chunk))
            .map_err(|_| RpcError::cancelled())
    }
}

fn decode<P: DeserializeOwned>(params: Value) -> Result<P, RpcError> {
    serde_json::from_value(params).map_err(|e| RpcError::invalid_params(e.to_string()))
}

/// 메서드 레지스트리. 다 채운 뒤 `Arc` 로 세션들이 공유한다.
#[derive(Default)]
pub struct Router {
    entries: HashMap<&'static str, Entry>,
}

impl Router {
    pub fn new() -> Self {
        Self::default()
    }

    /// unary 메서드에 핸들러를 붙인다.
    ///
    /// # Panics
    /// 같은 이름이 이미 등록돼 있으면 패닉한다(프로그래밍 오류).
    pub fn unary<M, F, Fut>(&mut self, _method: M, handler: F) -> &mut Self
    where
        M: UnaryMethod,
        F: Fn(Ctx, M::Params) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<M::Result, RpcError>> + Send + 'static,
    {
        let erased = move |ctx: Ctx,
                           params: Value|
              -> Result<BoxFuture<Result<Value, RpcError>>, RpcError> {
            let fut = handler(ctx, decode(params)?);
            Ok(Box::pin(async move {
                let result = fut.await?;
                serde_json::to_value(result).map_err(|e| RpcError::internal(e.to_string()))
            }))
        };
        self.insert(M::META, Handler::Unary(Box::new(erased)))
    }

    /// 스트림 메서드에 핸들러를 붙인다. 핸들러가 `Ok` 로 끝나면 스트림은 completed, `Err` 면 failed 로 닫힌다.
    ///
    /// # Panics
    /// 같은 이름이 이미 등록돼 있으면 패닉한다(프로그래밍 오류).
    pub fn stream<M, F, Fut>(&mut self, _method: M, handler: F) -> &mut Self
    where
        M: StreamMethod,
        F: Fn(Ctx, M::Params, StreamSink<M::Item>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<(), RpcError>> + Send + 'static,
    {
        let erased = move |ctx: Ctx,
                           params: Value,
                           raw: RawSink|
              -> Result<BoxFuture<Result<(), RpcError>>, RpcError> {
            let sink = StreamSink {
                raw,
                _item: PhantomData,
            };
            Ok(Box::pin(handler(ctx, decode(params)?, sink)))
        };
        self.insert(M::META, Handler::Stream(Box::new(erased)))
    }

    fn insert(&mut self, meta: MethodMeta, handler: Handler) -> &mut Self {
        let previous = self.entries.insert(meta.name, Entry { meta, handler });
        assert!(
            previous.is_none(),
            "rpc method `{}` registered twice",
            meta.name
        );
        self
    }

    /// 등록된 메서드의 메타데이터를 이름순으로.
    pub fn methods(&self) -> Vec<&MethodMeta> {
        let mut metas: Vec<_> = self.entries.values().map(|e| &e.meta).collect();
        metas.sort_by_key(|m| m.name);
        metas
    }

    pub(crate) fn entry(&self, name: &str) -> Option<&Entry> {
        self.entries.get(name)
    }
}
