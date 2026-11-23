//! 원격 호스트의 기억(18-m7-plan M7-08 — 메인 판정 M7-P6). `state.db` 의 `remote_hosts`(v6): 호스트 항목 이름 → 처음 본 머신 id ·
//! 마지막 링크 상태와 그 시각.
//!
//! - **머신 id**: Relay 기기 공개키의 지문. 처음 붙을 때 본 id 를 기억하고, 같은 이름에서 다른 id 가 오면 붙지 않는다(SSH 호스트 키
//!   변경과 같은 취급). 사람이 「이 머신을 새로 믿기」 를 고를 때만 바꾼다([`Registry::remember_remote_machine`] 을 그때 부른다 —
//!   묶음 멤버의 머신 id 옮기기 `rename_bundle_machine` 과 함께, `pw-remote`).
//! - **마지막 링크 상태**: 링크를 열지 않는 가벼운 상태 확인(웰컴 · 체크아웃 목록)이 `ssh -O check` 를 못 할 때 읽는다.
//! - 이 표는 Desktop 의 것이다. 원격 머신의 레지스트리(Relay 의 `state.db`)에는 쓰이지 않는다.

use rusqlite::{OptionalExtension, params};

use super::bundle::{LOCAL_MACHINE, rename_machine_in};
use super::{ChangeKind, Registry, RegistryError, now_ms};

/// 시험: 검사와 쓰기 사이에서 멈추는 시간(ms). 검사 · 쓰기가 한 트랜잭션이 아니면 다른 쓰기가 이 틈에 끼어든다(리뷰 F-b).
#[cfg(test)]
static CHECK_PAUSE_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn pause_after_check() {
    #[cfg(test)]
    {
        let ms = CHECK_PAUSE_MS.load(std::sync::atomic::Ordering::SeqCst);
        if ms > 0 {
            std::thread::sleep(std::time::Duration::from_millis(ms));
        }
    }
}

fn conflict(path: &str, reason: String) -> RegistryError {
    RegistryError::InvalidPath {
        path: path.into(),
        reason,
    }
}

/// `machine` 을 `host` 밖의 다른 호스트 이름이 기억하고 있으면 그 이름.
fn other_host_with(
    tx: &rusqlite::Connection,
    host: &str,
    machine: &str,
) -> Result<Option<String>, RegistryError> {
    Ok(tx
        .query_row(
            "SELECT host FROM remote_hosts WHERE machine = ?1 AND host <> ?2 ORDER BY host LIMIT 1",
            params![machine, host],
            |row| row.get(0),
        )
        .optional()?)
}

fn remembered_in(tx: &rusqlite::Connection, host: &str) -> Result<Option<String>, RegistryError> {
    Ok(tx
        .query_row(
            "SELECT machine FROM remote_hosts WHERE host = ?1",
            params![host],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()?
        .flatten())
}

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

    /// 처음 붙은 호스트의 머신 id 를 기억한다(증명된 링크의 키 지문). 검사와 쓰기가 한 트랜잭션이다(리뷰 F-b): 그 호스트가 이미
    /// 다른 id 를 기억하거나, 다른 호스트 이름이 같은 id 를 기억하면 거절한다(한 머신은 이름 하나 — 두 이름의 묶음 멤버가 섞이지
    /// 않게). 같은 id 를 이미 기억하면 그대로.
    pub fn remember_first_remote_machine(
        &self,
        host: &str,
        machine: &str,
    ) -> Result<(), RegistryError> {
        check_host(host)?;
        if machine.is_empty() || machine == LOCAL_MACHINE {
            return Err(conflict(machine, "not a remote machine id".into()));
        }
        self.write(|tx| {
            match remembered_in(tx, host)? {
                Some(known) if known == machine => return Ok(()),
                Some(known) => {
                    return Err(conflict(
                        host,
                        format!("host `{host}` already remembers machine {known}"),
                    ));
                }
                None => {}
            }
            if let Some(other) = other_host_with(tx, host, machine)? {
                return Err(conflict(
                    machine,
                    format!("machine {machine} is already known as host `{other}`"),
                ));
            }
            pause_after_check();
            tx.execute(
                "INSERT INTO remote_hosts (host, machine) VALUES (?1, ?2)
                 ON CONFLICT (host) DO UPDATE SET machine = excluded.machine",
                params![host, machine],
            )?;
            Ok(())
        })
    }

    /// 「이 머신을 새로 믿기」(메인 판정 M7-P6): 그 호스트가 기억한 id 가 `expected` 일 때만, `found`(핸드셰이크가 증명한 새 키의
    /// 지문)로 묶음 멤버 · 마지막 보내기 대상을 옮기고 새 id 를 기억한다. 검사(기억 · 다른 호스트 이름이 `found` 나 `expected` 를
    /// 기억하는가)와 쓰기가 **한 트랜잭션**이다 — 두 믿기 · 믿기와 첫 연결이 겹쳐도 한 머신의 멤버가 섞이지 않는다(리뷰 F-b).
    /// 바뀐 묶음 id 들.
    pub fn trust_remote_machine(
        &self,
        host: &str,
        expected: &str,
        found: &str,
    ) -> Result<Vec<String>, RegistryError> {
        check_host(host)?;
        if [expected, found]
            .iter()
            .any(|m| m.is_empty() || *m == LOCAL_MACHINE)
            || expected == found
        {
            return Err(conflict(
                found,
                format!("cannot move host `{host}` from machine `{expected}` to `{found}`"),
            ));
        }
        let changed = self.write(|tx| {
            if remembered_in(tx, host)?.as_deref() != Some(expected) {
                return Err(conflict(
                    host,
                    format!("host `{host}` does not remember machine {expected}"),
                ));
            }
            for machine in [found, expected] {
                if let Some(other) = other_host_with(tx, host, machine)? {
                    return Err(conflict(
                        machine,
                        format!(
                            "machine {machine} is also known as host `{other}`; remove that host or connect it under one name"
                        ),
                    ));
                }
            }
            pause_after_check();
            let changed = rename_machine_in(tx, expected, found)?;
            tx.execute(
                "UPDATE remote_hosts SET machine = ?2 WHERE host = ?1",
                params![host, found],
            )?;
            Ok(changed)
        })?;
        for id in &changed {
            self.publish(ChangeKind::Bundle, id);
        }
        Ok(changed)
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
    use std::sync::{Arc, Barrier};

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

    fn two_at_once<A, B>(a: A, b: B) -> (Result<(), RegistryError>, Result<(), RegistryError>)
    where
        A: FnOnce() -> Result<(), RegistryError> + Send + 'static,
        B: FnOnce() -> Result<(), RegistryError> + Send + 'static,
    {
        CHECK_PAUSE_MS.store(300, std::sync::atomic::Ordering::SeqCst);
        let gate = Arc::new(Barrier::new(2));
        let (ga, gb) = (Arc::clone(&gate), gate);
        let ta = std::thread::spawn(move || {
            ga.wait();
            a()
        });
        let tb = std::thread::spawn(move || {
            gb.wait();
            b()
        });
        let out = (ta.join().unwrap(), tb.join().unwrap());
        CHECK_PAUSE_MS.store(0, std::sync::atomic::Ordering::SeqCst);
        out
    }

    // 리뷰 F-b: 두 호스트가 같은 새 머신 Z 를 동시에 믿으면 하나만 된다 — 검사와 쓰기가 한 트랜잭션이라 늦은 쪽은 먼저 쓴 쪽을 본다.
    // (같은 파일을 여는 두 레지스트리 = 두 프로세스와 같다.)
    #[test]
    fn two_trusts_of_one_machine_at_once_move_only_one_host() {
        let dir = tempfile::tempdir().unwrap();
        let reg = registry(dir.path());
        reg.remember_remote_machine("mini", "SHA256:x").unwrap();
        reg.remember_remote_machine("other", "SHA256:w").unwrap();
        let (a, b) = (registry(dir.path()), registry(dir.path()));
        let (ra, rb) = two_at_once(
            move || {
                a.trust_remote_machine("mini", "SHA256:x", "SHA256:z")
                    .map(drop)
            },
            move || {
                b.trust_remote_machine("other", "SHA256:w", "SHA256:z")
                    .map(drop)
            },
        );
        assert!(ra.is_ok() != rb.is_ok(), "{ra:?} {rb:?}");
        let holders: Vec<_> = reg
            .remote_hosts()
            .unwrap()
            .into_iter()
            .filter(|r| r.machine.as_deref() == Some("SHA256:z"))
            .collect();
        assert_eq!(holders.len(), 1, "{holders:?}");
    }

    // 리뷰 F-b: 믿기(mini → Z)와 다른 호스트(b)의 첫 연결(Z)이 겹쳐도 Z 를 두 이름이 갖지 않는다.
    #[test]
    fn a_trust_racing_a_first_connection_leaves_one_name_per_machine() {
        let dir = tempfile::tempdir().unwrap();
        let reg = registry(dir.path());
        reg.remember_remote_machine("mini", "SHA256:x").unwrap();
        let (a, b) = (registry(dir.path()), registry(dir.path()));
        let (ra, rb) = two_at_once(
            move || {
                a.trust_remote_machine("mini", "SHA256:x", "SHA256:z")
                    .map(drop)
            },
            move || b.remember_first_remote_machine("b", "SHA256:z"),
        );
        assert!(ra.is_ok() != rb.is_ok(), "{ra:?} {rb:?}");
        let holders = reg
            .remote_hosts()
            .unwrap()
            .into_iter()
            .filter(|r| r.machine.as_deref() == Some("SHA256:z"))
            .count();
        assert_eq!(holders, 1);
    }

    #[test]
    fn trusting_needs_the_remembered_id_and_one_name_per_machine() {
        let dir = tempfile::tempdir().unwrap();
        let reg = registry(dir.path());
        assert!(
            reg.trust_remote_machine("mini", "SHA256:x", "SHA256:z")
                .is_err()
        );
        reg.remember_remote_machine("mini", "SHA256:x").unwrap();
        assert!(
            reg.trust_remote_machine("mini", "SHA256:other", "SHA256:z")
                .is_err()
        );
        reg.remember_remote_machine("z", "SHA256:z").unwrap();
        assert!(
            reg.trust_remote_machine("mini", "SHA256:x", "SHA256:z")
                .is_err()
        );
        reg.trust_remote_machine("mini", "SHA256:x", "SHA256:y")
            .unwrap();
        assert_eq!(
            reg.remote_host("mini").unwrap().unwrap().machine.as_deref(),
            Some("SHA256:y")
        );
        // 첫 연결: 이미 다른 id 를 기억한 호스트 · 다른 이름이 가진 id 는 거절, 같은 id 는 그대로.
        assert!(
            reg.remember_first_remote_machine("mini", "SHA256:q")
                .is_err()
        );
        assert!(reg.remember_first_remote_machine("n", "SHA256:z").is_err());
        reg.remember_first_remote_machine("mini", "SHA256:y")
            .unwrap();
        reg.remember_first_remote_machine("n", "SHA256:n").unwrap();
    }
}
