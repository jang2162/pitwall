//! 표지 기억을 사람이 보고 푸는 RPC(16-m5-plan 결정 M5-D37 — 사용자 결정 2026-11-18, M5-17b). 기억 자체는
//! `crate::gate` 의 `MarkMemory`(Host 의 상태 DB 에 영속).
//!
//! - `db.connection.marks`: 연결 하나의 푼 대상에 기억된 표지와 지금 연결 설정이 다는 표지.
//! - `db.connection.forgetMarks`: 그 대상의 기억을 지운다(사람이 연결 편집 다이얼로그에서 확인한 뒤). 지금 설정에 표지가 붙어
//!   있으면 곧바로 다시 기억한다(설정을 다시 읽는다) — 결과의 `configured` 가 그것을 말한다. 그 표지는 설정에서 지워야 풀린다.
//!
//! **둘 다 그 프로젝트 창의 연결만 받는다**([`require_project_window`] — MCP 세션 · 파일 범위 · 다른 프로젝트 창은
//! `PERMISSION_DENIED`). MCP 툴로 열지 않는다(`mcp` 를 달지 않는다). 기억을 푸는 것은 사람만 한다.

use std::sync::Arc;

use pw_rpc::{ConnectionScope, Router, RpcError, rpc_method};
use serde::{Deserialize, Serialize};

use crate::config::{
    Connections, DbConnectionParams, check_connection_id, not_found, require_project_window,
};
use crate::gate::{DbMarks, DbTarget};

/// 연결 하나의 대상에 대한 표지 기억.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub struct DbConnectionMarkMemory {
    /// `db.connection.marks`: 그 대상에 기억된 표지(앞 실행들의 것 포함). `db.connection.forgetMarks`: 지운 표지.
    pub remembered: DbMarks,
    /// 지금 연결 설정(병합 값과 각 레이어 파일)이 다는 표지. 기억을 풀어도 곧바로 다시 기억된다.
    pub configured: DbMarks,
}

rpc_method! {
    /// 연결 하나의 푼 대상에 기억된 프로덕션 · read-only 표지(에이전트에게 적용하는 것 — 결정 M5-D37). 그 프로젝트 창의 연결만.
    pub DbConnectionMarks: "db.connection.marks" {
        params: DbConnectionParams,
        result: DbConnectionMarkMemory,
        tier: Read,
        roles: [Owner],
    }

    /// 연결 하나의 푼 대상에 기억된 표지를 지운다. 그 프로젝트 창의 연결만 — MCP 세션은 permission denied. 상태 DB 를 쓸 수
    /// 없으면 storage unavailable(지우지 않는다).
    pub DbConnectionForgetMarks: "db.connection.forgetMarks" {
        params: DbConnectionParams,
        result: DbConnectionMarkMemory,
        tier: Write,
        roles: [Owner],
    }
}

impl Connections {
    /// 그 연결의 대상과 지금 설정의 표지. 설정을 읽어 올리므로 지금 표지를 기억에 더한다. 블로킹이다.
    fn mark_target(&self, params: &DbConnectionParams) -> Result<(DbTarget, DbMarks), RpcError> {
        check_connection_id(&params.connection_id)?;
        let loaded = self.load(&params.project_id, params.worktree.as_deref())?;
        if let Err(problem) = &loaded.connections {
            return Err(RpcError::internal(format!(
                "cannot read the project config: {problem}"
            )));
        }
        let connection = loaded
            .get(&params.connection_id)
            .ok_or_else(|| not_found(&params.connection_id))?;
        let config = connection.config.as_ref().map_err(|_| {
            RpcError::invalid_params(crate::gate::invalid_settings(&params.connection_id))
        })?;
        Ok((
            DbTarget::of(config, &loaded.root),
            DbMarks::of_loaded(connection, config),
        ))
    }

    /// `db.connection.marks`.
    pub fn mark_memory(
        &self,
        scope: &ConnectionScope,
        params: &DbConnectionParams,
    ) -> Result<DbConnectionMarkMemory, RpcError> {
        require_project_window(scope, &params.project_id)?;
        let (target, configured) = self.mark_target(params)?;
        Ok(DbConnectionMarkMemory {
            remembered: self.marks.remembered(&target)?,
            configured,
        })
    }

    /// `db.connection.forgetMarks`.
    pub fn forget_marks(
        &self,
        scope: &ConnectionScope,
        params: &DbConnectionParams,
    ) -> Result<DbConnectionMarkMemory, RpcError> {
        require_project_window(scope, &params.project_id)?;
        let (target, configured) = self.mark_target(params)?;
        let remembered = self.marks.forget(&target)?;
        // 지금 설정이 다는 표지는 곧바로 다시 기억한다 — 풀린 틈에 파일에서 표지를 지우고 다시 켜는 길을 남기지 않는다.
        if configured != DbMarks::default() {
            self.load(&params.project_id, params.worktree.as_deref())?;
        }
        tracing::info!(target: "pw_db", project = %params.project_id, connection = %params.connection_id, ?remembered, "the person forgot the remembered db marks of a target");
        Ok(DbConnectionMarkMemory {
            remembered,
            configured,
        })
    }
}

/// 핸들러를 붙인다(`crate::config::register` 가 부른다).
pub(crate) fn register(router: &mut Router, connections: Arc<Connections>) {
    let marks = Arc::clone(&connections);
    router
        .unary(DbConnectionMarks, move |ctx, params: DbConnectionParams| {
            let (c, scope) = (Arc::clone(&marks), ctx.scope().clone());
            crate::config::blocking(move || c.mark_memory(&scope, &params))
        })
        .unary(
            DbConnectionForgetMarks,
            move |ctx, params: DbConnectionParams| {
                let (c, scope) = (Arc::clone(&connections), ctx.scope().clone());
                crate::config::blocking(move || c.forget_marks(&scope, &params))
            },
        );
}
