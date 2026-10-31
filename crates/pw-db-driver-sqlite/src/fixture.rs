//! SQLite 시험 픽스처: 픽스처 스키마([`SCHEMA`])를 넣은 임시 파일 · 인메모리 DB(14-m3-plan.md M3-01).
//!
//! 스키마는 이후 행이 기대값을 적는 대상이다 — 키 있는 테이블 둘(`customers` · `orders`, FK · 보조 인덱스), 키 없는 테이블
//! 하나(`audit_log` — 그리드 편집이 거절할 것, M3-12), 뷰 하나. 바꾸면 그 기대값을 쓰는 시험(M3-08 · M3-12)도 함께 고친다.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use rusqlite::{Connection, OpenFlags};
use tempfile::TempDir;

/// 픽스처 스키마와 행.
pub const SCHEMA: &str = r"
PRAGMA foreign_keys = ON;
CREATE TABLE customers (
    id INTEGER PRIMARY KEY,
    email TEXT NOT NULL UNIQUE,
    name TEXT,
    avatar BLOB
);
CREATE TABLE orders (
    id INTEGER PRIMARY KEY,
    customer_id INTEGER NOT NULL REFERENCES customers(id),
    total NUMERIC NOT NULL,
    placed_at TEXT
);
CREATE INDEX orders_customer ON orders(customer_id);
CREATE TABLE audit_log (
    at TEXT,
    message TEXT
);
CREATE VIEW big_orders AS SELECT * FROM orders WHERE total > 100;
INSERT INTO customers (id, email, name, avatar) VALUES
    (1, 'ada@example.com', 'Ada', x'deadbeef'),
    (2, 'grace@example.com', '그레이스', NULL),
    (3, 'linus@example.com', NULL, NULL);
INSERT INTO orders (id, customer_id, total, placed_at) VALUES
    (10, 1, 250.5, '2026-10-31 09:00:00'),
    (11, 1, 20, '2026-10-31 10:00:00'),
    (12, 2, 9007199254740993, NULL);
INSERT INTO audit_log (at, message) VALUES ('2026-10-31', 'seeded'), ('2026-10-31', 'seeded');
";

/// 픽스처 DB 하나. 놓으면(drop) 임시 파일 · 인메모리 DB 가 사라진다.
pub struct SqliteFixture {
    location: Location,
    /// 인메모리 DB 는 연결이 하나라도 열려 있어야 산다.
    _keeper: Option<Connection>,
}

enum Location {
    File { _dir: TempDir, path: PathBuf },
    Memory { uri: String },
}

impl SqliteFixture {
    /// 임시 폴더 안의 파일 DB(`fixture.db`).
    pub fn file() -> Self {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("fixture.db");
        Connection::open(&path)
            .and_then(|conn| conn.execute_batch(SCHEMA))
            .expect("fixture schema");
        Self {
            location: Location::File { _dir: dir, path },
            _keeper: None,
        }
    }

    /// 이름 있는 공유 인메모리 DB. 같은 픽스처의 연결끼리 데이터를 함께 보고, 다른 픽스처와는 섞이지 않는다.
    pub fn memory() -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let uri = format!(
            "file:pw-fixture-{}-{}?mode=memory&cache=shared",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        );
        let keeper = open_uri(&uri);
        keeper.execute_batch(SCHEMA).expect("fixture schema");
        Self {
            location: Location::Memory { uri },
            _keeper: Some(keeper),
        }
    }

    /// 파일 픽스처의 경로. 인메모리면 `None`.
    pub fn path(&self) -> Option<&Path> {
        match &self.location {
            Location::File { path, .. } => Some(path),
            Location::Memory { .. } => None,
        }
    }

    /// 새 연결(읽기 쓰기).
    pub fn open(&self) -> Connection {
        match &self.location {
            Location::File { path, .. } => Connection::open(path).expect("open fixture"),
            Location::Memory { uri } => open_uri(uri),
        }
    }
}

fn open_uri(uri: &str) -> Connection {
    Connection::open_with_flags(
        uri,
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_URI,
    )
    .expect("open in-memory fixture")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tables(conn: &Connection) -> Vec<(String, String)> {
        let mut statement = conn
            .prepare("SELECT type, name FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%' ORDER BY type, name")
            .unwrap();
        statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    }

    /// 완료 기준 ⑤: 세 OS 에서 픽스처가 만들어지고 다른 연결이 그 스키마 · 행을 본다.
    #[test]
    fn the_file_fixture_holds_the_schema_and_disappears_on_drop() {
        let fixture = SqliteFixture::file();
        let path = fixture.path().unwrap().to_path_buf();
        let conn = fixture.open();
        let expected = [
            ("index", "orders_customer"),
            ("table", "audit_log"),
            ("table", "customers"),
            ("table", "orders"),
            ("view", "big_orders"),
        ]
        .map(|(kind, name)| (kind.to_owned(), name.to_owned()));
        assert_eq!(tables(&conn), expected);
        let big: i64 = conn
            .query_row("SELECT count(*) FROM big_orders", [], |r| r.get(0))
            .unwrap();
        assert_eq!(big, 2);
        let name: String = conn
            .query_row("SELECT name FROM customers WHERE id = 2", [], |r| r.get(0))
            .unwrap();
        assert_eq!(name, "그레이스");
        let fk: i64 = conn
            .query_row(
                "SELECT count(*) FROM pragma_foreign_key_list('orders')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(fk, 1);
        drop(conn);
        drop(fixture);
        assert!(!path.exists(), "the temp file goes with the fixture");
    }

    #[test]
    fn memory_fixtures_are_shared_within_and_isolated_between() {
        let a = SqliteFixture::memory();
        let b = SqliteFixture::memory();
        assert!(a.path().is_none());
        a.open()
            .execute("INSERT INTO audit_log VALUES ('now', 'only in a')", [])
            .unwrap();
        let count = |f: &SqliteFixture| -> i64 {
            f.open()
                .query_row("SELECT count(*) FROM audit_log", [], |r| r.get(0))
                .unwrap()
        };
        assert_eq!(count(&a), 3);
        assert_eq!(count(&b), 2);
    }
}
