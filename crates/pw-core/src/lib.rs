//! Pitwall 코어: Host 추상, in-process [`LocalHost`], 프로젝트 문맥, 이벤트 버스, 장기 작업, Host Router.
//!
//! - [`Host`]: 연결을 열고 JSON-RPC 메시지를 주고받는 헤드리스 서비스. 로컬 · 원격이 같은 trait 를 구현한다.
//! - [`HostRouter`]: project_id → Host.
//! - [`EventBus`]: 범위 · 토픽 기반 발행/구독. 느린 구독자는 오래된 이벤트부터 잃는다.
//! - [`TaskSpawner`] · [`TaskHandle`]: 진행률 · 취소가 있는 장기 작업.
//! - [`methods`]: Host 자신이 처리하는 RPC 메서드.
//! - [`config`]: 설정 계층(파일 위치 · 병합 · 스키마 마이그레이션 · 감시).

pub mod bus;
pub mod config;
mod host;
mod local;
pub mod methods;
mod router;
pub mod task;

pub use bus::{EventBus, Filter, Received, Scope, Subscription};
pub use host::{BoxFuture, Connection, FrameSink, FrameSource, Host, HostError, HostId, Target};
pub use local::{HostConfig, LocalHost, ProjectContext};
pub use router::HostRouter;
pub use task::{TaskCtx, TaskHandle, TaskProgress, TaskSpawner, TaskStatus};

/// 프로젝트 id. 프로젝트 레지스트리(M0-09)가 발급한다.
pub type ProjectId = String;
