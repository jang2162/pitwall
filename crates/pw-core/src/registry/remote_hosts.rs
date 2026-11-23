//! 원격 호스트의 기억(18-m7-plan M7-08 — 메인 판정 M7-P6). `state.db` 의 `remote_hosts`(v6): 호스트 항목 이름 → 처음 본 머신 id ·
//! 마지막 링크 상태와 그 시각.
//!
//! - **머신 id**: Relay 기기 공개키의 지문. 처음 붙을 때 본 id 를 기억하고, 같은 이름에서 다른 id 가 오면 붙지 않는다(SSH 호스트 키
//!   변경과 같은 취급). 사람이 「이 머신을 새로 믿기」 를 고를 때만 바꾼다([`Registry::remember_remote_machine`] 을 그때 부른다 —
//!   묶음 멤버의 머신 id 옮기기 `rename_bundle_machine` 과 함께, `pw-remote`).
//! - **마지막 링크 상태**: 링크를 열지 않는 가벼운 상태 확인(웰컴 · 체크아웃 목록)이 `ssh -O check` 를 못 할 때 읽는다.
//! - 이 표는 Desktop 의 것이다. 원격 머신의 레지스트리(Relay 의 `state.db`)에는 쓰이지 않는다.

use rusqlite::{OptionalExtension, params};

use super::{Registry, RegistryError, now_ms};

/// 마지막 링크 상태.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkMark {
    /// 링크가 섰다.
    Connected,
    /// 링크가 닫혔다(사람이 끊음 · 쓰는 창이 없어 닫음 · 끊김).
    Disconnected,
    /// 붙지 못했다.
    Failed,
}

impl LinkMark {
    pub fn as_str(self) -> &'static str {
        match self {
            LinkMark::Connected => "connected",
            LinkMark::Disconnected => "disconnected",
            LinkMark::Failed => "failed",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "connected" => LinkMark::Connected,
            "disconnected" => LinkMark::Disconnected,
            "failed" => LinkMark::Failed,
            _ => return None,
        })
    }
}

/// 호스트 하나의 기억.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteHostRecord {
    /// 호스트 항목 이름.
    pub host: String,
    /// 처음 본(또는 사람이 새로 믿은) 머신 id.
    pub machine: Option<String>,
    /// 마지막 링크 상태와 그 시각(ms).
    pub last: Option<(LinkMark, i64)>,
}

/// 호스트 항목 이름의 최대 길이(바이트).
const MAX_HOST_LEN: usize = 256;

fn check_host(host: &str) -> Result<(), RegistryError> {
    if host.is_empty() || host.len() > MAX_HOST_LEN {
        return Err(RegistryError::InvalidPath {
            path: host.into(),
            reason: "a remote host name must be 1 to 256 bytes".into(),
        });
    }
    Ok(())
}

fn record(row: &rusqlite::Row<'_>) -> rusqlite::Result<RemoteHostRecord> {
    let state: Option<String> = row.get(2)?;
    let at: Option<i64> = row.get(3)?;
    Ok(RemoteHostRecord {
        host: row.get(0)?,
        machine: row.get(1)?,
        last: state.as_deref().and_then(LinkMark::parse).zip(at),
    })
}

impl Registry {
    /// 기억한 원격 호스트 전부(이름순).
    pub fn remote_hosts(&self) -> Result<Vec<RemoteHostRecord>, RegistryError> {
        self.read(|conn| {
            let mut stmt = conn.prepare(
                "SELECT host, machine, last_state, last_at FROM remote_hosts ORDER BY host",
            )?;
            let rows = stmt.query_map([], record)?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })
    }

    /// 그 호스트의 기억. 없으면 `None`.
    pub fn remote_host(&self, host: &str) -> Result<Option<RemoteHostRecord>, RegistryError> {
        self.read(|conn| {
            Ok(conn
                .query_row(
                    "SELECT host, machine, last_state, last_at FROM remote_hosts WHERE host = ?1",
                    params![host],
                    record,
                )
                .optional()?)
        })
    }

    /// 그 호스트의 머신 id 를 기억한다(처음 붙을 때 · 사람이 「이 머신을 새로 믿기」 를 골랐을 때).
    pub fn remember_remote_machine(&self, host: &str, machine: &str) -> Result<(), RegistryError> {
        check_host(host)?;
        if machine.is_empty() {
            return Err(RegistryError::InvalidPath {
                path: machine.into(),
                reason: "a machine id must not be empty".into(),
            });
        }
        self.write(|tx| {
            tx.execute(
                "INSERT INTO remote_hosts (host, machine) VALUES (?1, ?2)
                 ON CONFLICT (host) DO UPDATE SET machine = excluded.machine",
                params![host, machine],
            )?;
            Ok(())
        })
    }

    /// 마지막 링크 상태를 지금 시각으로 남긴다.
    pub fn record_remote_link(&self, host: &str, mark: LinkMark) -> Result<(), RegistryError> {
        check_host(host)?;
        let at = now_ms();
        self.write(|tx| {
            tx.execute(
                "INSERT INTO remote_hosts (host, last_state, last_at) VALUES (?1, ?2, ?3)
                 ON CONFLICT (host) DO UPDATE SET last_state = excluded.last_state,
                   last_at = excluded.last_at",
                params![host, mark.as_str(), at],
            )?;
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::EventBus;
    use crate::config::AppPaths;

    fn registry(dir: &std::path::Path) -> Registry {
        Registry::new(AppPaths::under(dir), None, EventBus::new(16))
    }

    #[test]
    fn the_machine_and_the_last_link_are_remembered_apart() {
        let dir = tempfile::tempdir().unwrap();
        let reg = registry(dir.path());
        assert_eq!(reg.remote_host("mini").unwrap(), None);
        reg.record_remote_link("mini", LinkMark::Failed).unwrap();
        reg.remember_remote_machine("mini", "SHA256:abc").unwrap();
        reg.record_remote_link("mini", LinkMark::Connected).unwrap();
        let got = reg.remote_host("mini").unwrap().unwrap();
        assert_eq!(got.machine.as_deref(), Some("SHA256:abc"));
        assert_eq!(got.last.map(|(m, _)| m), Some(LinkMark::Connected));
        // 다시 열어도 남는다.
        drop(reg);
        let reg = registry(dir.path());
        reg.remember_remote_machine("mini", "SHA256:new").unwrap();
        let all = reg.remote_hosts().unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].machine.as_deref(), Some("SHA256:new"));
        assert_eq!(all[0].last.map(|(m, _)| m), Some(LinkMark::Connected));
        assert!(reg.remember_remote_machine("", "x").is_err());
        assert!(reg.remember_remote_machine("mini", "").is_err());
    }
}
