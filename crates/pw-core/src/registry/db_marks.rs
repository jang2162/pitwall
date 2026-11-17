//! DB 표지 기억의 영속 저장(16-m5-plan 결정 M5-D37 — 사용자 결정 2026-11-18, M5-17b). 한 번 프로덕션 · read-only 였던 접속
//! 대상은 앱을 다시 켜도 기억하고, 사람이 앱에서 직접 풀 때만 풀린다. 판정 · 열쇠는 pw-db 의 몫이다(`pw_db::gate` 의 표지 기억 —
//! 열쇠 글은 거기서 만든다). 이 모듈은 `state.db` 의 `db_marks`(v3)를 읽고 쓰기만 한다.
//!
//! - **조여지기만 한다**: [`Registry::add_db_marks`] 는 이미 있는 행의 표지를 끄지 않는다(`MAX`). 표지를 지우는 길은
//!   [`Registry::forget_db_marks`] 하나이고, 그것을 부르는 RPC 는 그 프로젝트 창의 연결만 받는다(pw-db).
//! - 쓰지 못하는 저장소(`readOnly` · `memory`)면 [`RegistryError::Unavailable`] — 부른 쪽(pw-db)이 에이전트의 DB 툴을 거절한다
//!   (fail-closed).

use rusqlite::{OptionalExtension, params};

use super::{Registry, RegistryError, now_ms};

/// 한 대상에 기억된 표지.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StoredDbMarks {
    pub production: bool,
    pub read_only: bool,
}

impl StoredDbMarks {
    pub fn is_empty(self) -> bool {
        !self.production && !self.read_only
    }
}

impl Registry {
    /// 기억된 표지 전부(대상 열쇠 글, 표지).
    pub fn db_marks(&self) -> Result<Vec<(String, StoredDbMarks)>, RegistryError> {
        self.read(|conn| {
            let mut stmt =
                conn.prepare("SELECT target, production, read_only FROM db_marks ORDER BY target")?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    StoredDbMarks {
                        production: row.get(1)?,
                        read_only: row.get(2)?,
                    },
                ))
            })?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
    }

    /// 표지를 더한다(조이기만 — 이미 켜진 표지는 끄지 않는다). 빈 표지는 건너뛴다.
    pub fn add_db_marks(&self, marks: &[(String, StoredDbMarks)]) -> Result<(), RegistryError> {
        self.ensure_writable()?;
        let now = now_ms();
        self.write(|tx| {
            for (target, m) in marks.iter().filter(|(_, m)| !m.is_empty()) {
                tx.execute(
                    "INSERT INTO db_marks (target, production, read_only, seen_at) VALUES (?1, ?2, ?3, ?4)
                     ON CONFLICT (target) DO UPDATE SET
                         production = MAX(production, excluded.production),
                         read_only = MAX(read_only, excluded.read_only),
                         seen_at = excluded.seen_at",
                    params![target, m.production, m.read_only, now],
                )?;
            }
            Ok(())
        })
    }

    /// 그 대상의 기억을 지우고 지운 표지를 돌려준다(없었으면 빈 표지).
    pub fn forget_db_marks(&self, target: &str) -> Result<StoredDbMarks, RegistryError> {
        self.ensure_writable()?;
        self.write(|tx| {
            let before = tx
                .query_row(
                    "SELECT production, read_only FROM db_marks WHERE target = ?1",
                    [target],
                    |row| {
                        Ok(StoredDbMarks {
                            production: row.get(0)?,
                            read_only: row.get(1)?,
                        })
                    },
                )
                .optional()?
                .unwrap_or_default();
            tx.execute("DELETE FROM db_marks WHERE target = ?1", [target])?;
            Ok(before)
        })
    }
}
