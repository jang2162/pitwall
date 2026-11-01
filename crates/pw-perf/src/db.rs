//! DB 시나리오(`pw-perf --db`): 앱이 프로젝트 창에서 돌린 시나리오(`apps/desktop/src/perf-scenario-db.ts`)의 표시를 읽어 14-m3-plan.md
//! M3-21 이 release 번들에서 다시 재는 것 — M3-16 의 실행 → 첫 행 · 10만 행 스크롤 · 스트리밍 중 스크롤, M3-13 의 테이블 1만 개 스키마
//! 첫 화면, M3-18 의 키 100만 개 첫 화면 · 키 트리 스크롤 — 을 회마다 모으고 요약한다. 무엇을 어떻게 재는지는 CONTRIBUTING.md
//! 「성능 기준선」 절의 「DB 시나리오」.
//!
//! 연결은 회마다 만든 임시 프로젝트의 `.pitwall/db/<id>.toml` 에 적는다([`write_connection`] — id = 이름, 비밀번호 없음, TLS 끔). 앱
//! 시나리오는 탐색기에 보이는 연결로 단계를 고른다.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::git::{FrameStats, RunFailure, Scroll, TARGET_VSYNC_MS, Viewport, refresh_share};
use crate::marks::Mark;
use crate::report::{Build, Commit, Hardware, Memory, Os, environment};
use crate::stats::{Summary, summarize};

/// 앱에 가짜 postgres 를 달라는 변수(`apps/desktop/src-tauri/src/perf_fake_db.rs` 의 `ENV`).
pub const FAKE_DB_ENV: &str = "PITWALL_PERF_FAKE_DB";
/// 가짜를 등록한 앱이 남기는 로그 줄(`perf_fake_db.rs` 의 `LOG_MARK`). 없으면 그 번들은 feature 없이 만든 것이다.
pub const FAKE_DB_LOG: &str = "perf-fake-db: fake postgres driver registered";

/// 앱 시나리오가 찾는 연결 id(`perf-scenario-db.ts`).
pub const GRID: &str = "perf-grid";
pub const SCHEMA: &str = "perf-schema";
pub const REDIS: &str = "perf-redis";
pub const IDLE: &str = "perf-idle";

/// 결과 행 수(가짜의 `ROWS`, 실 PG 픽스처의 행 수) — 그리드가 이만큼을 다 받아야 그 회를 잰 것으로 본다.
pub const GRID_ROWS: u64 = 100_000;

/// 그리드 칸의 최소 높이(px). 이보다 낮으면 행을 몇 개 그리지 않아 첫 화면 · 스크롤 수치가 사람이 보는 화면의 것이 아니다 — 콘솔의 결과
/// 칸이 flex 용기가 아니어서 그리드가 10px 로 줄었던 결함(M3-21 이 찾아 고침)을 실기에서 다시 잡는다.
pub const MIN_GRID_HEIGHT: f64 = 120.0;

/// 실행 → 첫 행이 그려진 프레임(06 「쿼리 결과 첫 화면」, M3-16).
pub const TARGET_GRID_OPEN_MS: f64 = 500.0;
/// 「테이블」 묶음 펼치기 → 첫 화면(06 「스키마 탐색기 첫 화면」, M3-13).
pub const TARGET_EXPLORER_MS: f64 = 500.0;
/// Redis 첫 화면(06 「Redis 키 브라우저 첫 화면」, M3-18).
pub const TARGET_REDIS_OPEN_MS: f64 = 500.0;
/// 스크롤 판정의 몫(06 「스크롤 60fps」 — 간격 중앙값 [`TARGET_VSYNC_MS`] 와 함께).
pub const TARGET_SCROLL_SHARE: f64 = 0.95;
/// 스트리밍 중 스크롤을 판정할 최소 프레임 수(1초 남짓). 쉬지 않고 보내는 로컬 실 PG 는 10만 행을 0.35초에 보내 17 프레임쯤만 스트리밍
/// 중이라, 긴 프레임 하나가 몫을 0.88 로 끌어내린다 — 그런 표본으로는 판정하지 않는다(M3-21 실기). 그래서 실 PG 픽스처의 `perf_stream` 은
/// 5,000행마다 150 ms 쉰다(`scripts/perf-m3.mjs` 의 `pgSeed`, 3초 남짓).
pub const MIN_STREAM_FRAMES: f64 = 60.0;

/// 결과 파일만 받은 사람도 읽을 수 있게 싣는 정의(정본은 CONTRIBUTING.md 「성능 기준선」 의 「DB 시나리오」).
pub const DEFINITIONS: DbDefinitions = DbDefinitions {
    grid_open: "in the webview: a new SQL console of `perf-grid` (⌘⇧L, connected first) with `SELECT * FROM perf_rows` (100,000 rows × 10 columns); from a click on [Run] to the animation frame after the result grid canvas first drew the first row's second cell (`r0c1`) — includes the run-scope RPC, the Host's execution, the first chunk, IPC and drawing",
    grid_scroll: "after the status line says done with every row: fling = top to bottom in 2000 frames (about 50 rows a frame); judged as 06 「스크롤 60fps」: the share of frames within one display refresh (interval ≤ 1.5 × the median interval) ≥ 0.95 and the median interval ≤ 17 ms; the share within 16.7 ms is reference only",
    grid_stream: "`SELECT * FROM perf_stream` (the same rows; the fake driver waits 30 ms per fetch, about 4 s for 100k rows; the real-PG fixture's view pauses 150 ms on the server every 5,000 rows and sends each burst as fast as it reads, about 3 s — without pauses a local database sends 100k rows in about 0.35 s); once the first row is drawn, scroll down 20 rows a frame (never past the received end) until the stream ends; judged as `grid_scroll` but only when at least 60 frames were scrolled while streaming (a shorter stream has no verdict, `no value`)",
    explorer: "in the webview: ⌘8, expand the `perf-schema` connection (opens it) and the schema `public`; from a click on the ▸ of the Tables group (10,000 tables) to the frame after the first table row appeared",
    redis_open: "in the webview: double-click the `perf-redis` connection in the explorer (a tab opens unconnected); from a click on [Connect] to the frame after the key tree showed its first row (connect + the unfiltered scan's first batch, 1,000,000 keys `k:0` … `k:999999`)",
    redis_scroll: "after the scan ended: expand the group `k:` (expandMs, sorting included), then fling top to bottom in 2000 frames; judged as `grid_scroll`",
    memory: "phys_footprint of the app instance right after the scenario finished (100k-row result, explorer, a Redis tab with 1M keys); not an idle baseline",
    judgement: "medians over valid measured runs; a run fails if the grid settled at a row count other than 100,000 or the Redis scan did not end as done",
    trust: "the project is registered trusted (an untrusted project cannot open connections, M3-D13); connections have no password and TLS off",
};

#[derive(Debug, Clone, Serialize)]
pub struct DbDefinitions {
    pub grid_open: &'static str,
    pub grid_scroll: &'static str,
    pub grid_stream: &'static str,
    pub explorer: &'static str,
    pub redis_open: &'static str,
    pub redis_scroll: &'static str,
    pub memory: &'static str,
    pub judgement: &'static str,
    pub trust: &'static str,
}

/// 측정 프로젝트에 적을 연결 하나.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Connection {
    /// 파일 이름이자 이름(앱 시나리오가 이것으로 찾는다).
    pub id: String,
    /// `postgres` · `redis`.
    pub driver: String,
    pub host: String,
    pub port: u16,
    pub database: Option<String>,
    pub user: Option<String>,
}

impl Connection {
    /// `host:port[/database]` 를 읽는다. 사용자는 PG 면 `postgres`(픽스처 컨테이너는 trust 인증), Redis 는 없다.
    pub fn parse(id: &str, driver: &str, address: &str) -> Result<Self, String> {
        let (hostport, database) = match address.split_once('/') {
            Some((hp, db)) if !db.is_empty() => (hp, Some(db.to_owned())),
            Some((hp, _)) => (hp, None),
            None => (address, None),
        };
        let (host, port) = hostport
            .rsplit_once(':')
            .ok_or_else(|| format!("{address}: expected host:port[/database]"))?;
        let port: u16 = port
            .parse()
            .map_err(|e| format!("{address}: bad port: {e}"))?;
        if host.is_empty() || host.starts_with('-') || host.contains(char::is_whitespace) {
            return Err(format!("{address}: bad host"));
        }
        Ok(Self {
            id: id.to_owned(),
            driver: driver.to_owned(),
            host: host.to_owned(),
            port,
            database,
            user: (driver == "postgres").then(|| "postgres".to_owned()),
        })
    }

    /// 가짜 postgres(앱의 `perf-fake-db` 가 받는다 — 주소는 쓰지 않는다).
    pub fn fake(id: &str) -> Self {
        Self {
            id: id.to_owned(),
            driver: "postgres".into(),
            host: "fake.invalid".into(),
            port: 5432,
            database: Some("perf".into()),
            user: Some("perf".into()),
        }
    }

    /// `.pitwall/db/<id>.toml` 의 글.
    pub fn toml(&self) -> String {
        let quote = |s: &str| serde_json::to_string(s).expect("a string serializes");
        let mut text = format!(
            "schema = 1\nname = {}\ndriver = {}\nhost = {}\nport = {}\nsslmode = \"disable\"\n",
            quote(&self.id),
            quote(&self.driver),
            quote(&self.host),
            self.port
        );
        if let Some(db) = &self.database {
            text.push_str(&format!("database = {}\n", quote(db)));
        }
        if let Some(user) = &self.user {
            text.push_str(&format!("user = {}\n", quote(user)));
        }
        text
    }
}

/// 연결을 프로젝트의 `.pitwall/db/<id>.toml` 로 쓴다.
pub fn write_connection(project: &Path, connection: &Connection) -> std::io::Result<()> {
    let dir = project.join(".pitwall/db");
    std::fs::create_dir_all(&dir)?;
    std::fs::write(
        dir.join(format!("{}.toml", connection.id)),
        connection.toml(),
    )
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GridOpen {
    pub ms: f64,
    pub first_draw_ms: f64,
    pub rows_at_first: u64,
    pub viewport: Viewport,
    /// 프로젝트 창 웹뷰의 크기(그리드가 몇 행을 그리는지 가르는 바깥).
    #[serde(default)]
    pub window: Option<Viewport>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GridRows {
    pub rows: u64,
    pub all_rows_ms: f64,
    pub state: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Flung {
    pub viewport: Viewport,
    pub scroll_height: f64,
    pub fling: Scroll,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GridStream {
    pub first_ms: f64,
    pub frames: u64,
    pub rows_at_end: u64,
    pub all_rows_ms: f64,
    pub interval: FrameStats,
    pub work: FrameStats,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExplorerTables {
    pub ms: f64,
    pub connect_ms: f64,
    pub dom_rows: u64,
    pub first_table: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RedisOpen {
    pub ms: f64,
    pub first_rows: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RedisKeys {
    pub all_keys_ms: f64,
    pub state: Option<String>,
    pub text: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RedisScroll {
    pub expand_ms: f64,
    pub viewport: Viewport,
    pub scroll_height: f64,
    pub last_key: Option<String>,
    pub fling: Scroll,
}

/// 한 회에 시나리오가 잰 것. 그 연결이 없어 건너뛴 단계는 `None` 이다.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DbMeasure {
    pub grid_open: Option<GridOpen>,
    pub grid_rows: Option<GridRows>,
    pub grid_scroll: Option<Flung>,
    pub grid_stream: Option<GridStream>,
    pub explorer: Option<ExplorerTables>,
    pub redis_open: Option<RedisOpen>,
    pub redis_keys: Option<RedisKeys>,
    pub redis_scroll: Option<RedisScroll>,
}

/// 측정하는 쪽이 적은 연결 — 시나리오가 그 단계를 모두 남겼어야 한다.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
pub struct Expect {
    pub grid: bool,
    pub schema: bool,
    pub redis: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum DbProgress {
    Waiting,
    Done(Box<DbMeasure>),
    Failed(String),
}

#[derive(Deserialize)]
struct Failure {
    step: String,
    message: String,
}

fn data<T: for<'de> Deserialize<'de>>(
    marks: &[Mark],
    pid: u32,
    event: &str,
) -> Result<Option<T>, String> {
    let Some(mark) = marks.iter().find(|m| m.event == event && m.pid == pid) else {
        return Ok(None);
    };
    let value = mark.data.clone().unwrap_or(serde_json::Value::Null);
    serde_json::from_value(value)
        .map(Some)
        .map_err(|e| format!("the app's `{event}` mark is malformed: {e}"))
}

/// `pid` 의 앱이 남긴 DB 시나리오 표시에서 진행을 읽는다. 끝났으면 `expect` 의 단계가 모두 있고 행 · 키를 다 받았는지 본다.
pub fn progress(marks: &[Mark], pid: u32, expect: Expect) -> DbProgress {
    match read(marks, pid, expect) {
        Ok(p) => p,
        Err(e) => DbProgress::Failed(e),
    }
}

fn read(marks: &[Mark], pid: u32, expect: Expect) -> Result<DbProgress, String> {
    if let Some(f) = data::<Failure>(marks, pid, "scenario_error")? {
        return Err(format!(
            "the scenario failed at `{}`: {}",
            f.step, f.message
        ));
    }
    if marks
        .iter()
        .all(|m| m.event != "scenario_done" || m.pid != pid)
    {
        return Ok(DbProgress::Waiting);
    }
    let m = DbMeasure {
        grid_open: data(marks, pid, "grid_open")?,
        grid_rows: data(marks, pid, "grid_rows")?,
        grid_scroll: data(marks, pid, "grid_scroll")?,
        grid_stream: data(marks, pid, "grid_stream_scroll")?,
        explorer: data(marks, pid, "explorer_tables")?,
        redis_open: data(marks, pid, "redis_open")?,
        redis_keys: data(marks, pid, "redis_keys")?,
        redis_scroll: data(marks, pid, "redis_scroll")?,
    };
    check(&m, expect)?;
    Ok(DbProgress::Done(Box::new(m)))
}

/// 적은 연결의 단계가 모두 있고, 그리드가 [`GRID_ROWS`] 를 다 받았고(스트리밍 결과도), Redis 스캔이 `done` 으로 끝났는가.
pub fn check(m: &DbMeasure, expect: Expect) -> Result<(), String> {
    let missing = |event: &str| format!("the scenario finished without a `{event}` mark");
    if expect.grid {
        let open = m.grid_open.as_ref().ok_or_else(|| missing("grid_open"))?;
        if open.viewport.height < MIN_GRID_HEIGHT {
            return Err(format!(
                "the result grid is only {} px tall (at least {MIN_GRID_HEIGHT} px) — the console's result pane does not give it room",
                open.viewport.height
            ));
        }
        let rows = m.grid_rows.as_ref().ok_or_else(|| missing("grid_rows"))?;
        m.grid_scroll
            .as_ref()
            .ok_or_else(|| missing("grid_scroll"))?;
        let stream = m
            .grid_stream
            .as_ref()
            .ok_or_else(|| missing("grid_stream_scroll"))?;
        if rows.rows != GRID_ROWS {
            return Err(format!(
                "the result grid settled at {} rows, not {GRID_ROWS}",
                rows.rows
            ));
        }
        if stream.rows_at_end != GRID_ROWS {
            return Err(format!(
                "the streamed result ended at {} rows, not {GRID_ROWS}",
                stream.rows_at_end
            ));
        }
    }
    if expect.schema {
        m.explorer
            .as_ref()
            .ok_or_else(|| missing("explorer_tables"))?;
    }
    if expect.redis {
        m.redis_open.as_ref().ok_or_else(|| missing("redis_open"))?;
        let keys = m.redis_keys.as_ref().ok_or_else(|| missing("redis_keys"))?;
        m.redis_scroll
            .as_ref()
            .ok_or_else(|| missing("redis_scroll"))?;
        if keys.state.as_deref() != Some("done") {
            return Err(format!(
                "the Redis scan ended as {:?} ({:?}), not done",
                keys.state, keys.text
            ));
        }
    }
    Ok(())
}

/// 한 회.
#[derive(Debug, Clone, Serialize)]
pub struct DbRun {
    pub index: usize,
    pub warmup: bool,
    /// 요약에서 뺀 이유(창이 가려졌다, 화면이 잠겼다). `None` 이면 유효한 회다.
    pub excluded: Option<String>,
    pub app_pid: u32,
    pub group: u32,
    /// 시작 → 프로젝트 창 첫 프레임(참고).
    pub cold_start_ms: f64,
    pub measure: DbMeasure,
    /// 시나리오가 끝난 직후의 메모리.
    pub memory: Memory,
    pub killed: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct DbSummary {
    pub grid_open_ms: Option<Summary>,
    pub grid_first_draw_ms: Option<Summary>,
    pub grid_all_rows_ms: Option<Summary>,
    pub grid_fling_within_vsync: Option<Summary>,
    pub grid_fling_vsync_ms: Option<Summary>,
    pub grid_fling_within16_7: Option<Summary>,
    pub grid_fling_p95_ms: Option<Summary>,
    pub grid_stream_within_vsync: Option<Summary>,
    pub grid_stream_vsync_ms: Option<Summary>,
    pub grid_stream_within16_7: Option<Summary>,
    pub grid_stream_frames: Option<Summary>,
    pub grid_stream_all_rows_ms: Option<Summary>,
    pub explorer_ms: Option<Summary>,
    pub explorer_connect_ms: Option<Summary>,
    pub redis_open_ms: Option<Summary>,
    pub redis_all_keys_ms: Option<Summary>,
    pub redis_expand_ms: Option<Summary>,
    pub redis_fling_within_vsync: Option<Summary>,
    pub redis_fling_vsync_ms: Option<Summary>,
    pub redis_fling_within16_7: Option<Summary>,
    pub cold_start_ms: Option<Summary>,
    pub footprint_bytes: Option<Summary>,
}

pub fn summarize_db(runs: &[DbRun]) -> DbSummary {
    let measured: Vec<&DbRun> = runs
        .iter()
        .filter(|r| !r.warmup && r.excluded.is_none())
        .collect();
    let values = |f: &dyn Fn(&DbMeasure) -> Option<f64>| {
        summarize(
            &measured
                .iter()
                .filter_map(|r| f(&r.measure))
                .collect::<Vec<_>>(),
        )
    };
    let run_values =
        |f: &dyn Fn(&DbRun) -> f64| summarize(&measured.iter().map(|r| f(r)).collect::<Vec<_>>());
    DbSummary {
        grid_open_ms: values(&|m| m.grid_open.as_ref().map(|g| g.ms)),
        grid_first_draw_ms: values(&|m| m.grid_open.as_ref().map(|g| g.first_draw_ms)),
        grid_all_rows_ms: values(&|m| m.grid_rows.as_ref().map(|g| g.all_rows_ms)),
        grid_fling_within_vsync: values(&|m| {
            m.grid_scroll
                .as_ref()
                .and_then(|g| g.fling.interval.within_vsync)
        }),
        grid_fling_vsync_ms: values(&|m| {
            m.grid_scroll
                .as_ref()
                .and_then(|g| g.fling.interval.vsync_ms)
        }),
        grid_fling_within16_7: values(&|m| {
            m.grid_scroll.as_ref().map(|g| g.fling.interval.within16_7)
        }),
        grid_fling_p95_ms: values(&|m| m.grid_scroll.as_ref().map(|g| g.fling.interval.p95)),
        grid_stream_within_vsync: values(&|m| {
            m.grid_stream.as_ref().and_then(|g| g.interval.within_vsync)
        }),
        grid_stream_vsync_ms: values(&|m| m.grid_stream.as_ref().and_then(|g| g.interval.vsync_ms)),
        grid_stream_within16_7: values(&|m| m.grid_stream.as_ref().map(|g| g.interval.within16_7)),
        grid_stream_frames: values(&|m| m.grid_stream.as_ref().map(|g| g.frames as f64)),
        grid_stream_all_rows_ms: values(&|m| m.grid_stream.as_ref().map(|g| g.all_rows_ms)),
        explorer_ms: values(&|m| m.explorer.as_ref().map(|e| e.ms)),
        explorer_connect_ms: values(&|m| m.explorer.as_ref().map(|e| e.connect_ms)),
        redis_open_ms: values(&|m| m.redis_open.as_ref().map(|r| r.ms)),
        redis_all_keys_ms: values(&|m| m.redis_keys.as_ref().map(|r| r.all_keys_ms)),
        redis_expand_ms: values(&|m| m.redis_scroll.as_ref().map(|r| r.expand_ms)),
        redis_fling_within_vsync: values(&|m| {
            m.redis_scroll
                .as_ref()
                .and_then(|r| r.fling.interval.within_vsync)
        }),
        redis_fling_vsync_ms: values(&|m| {
            m.redis_scroll
                .as_ref()
                .and_then(|r| r.fling.interval.vsync_ms)
        }),
        redis_fling_within16_7: values(&|m| {
            m.redis_scroll.as_ref().map(|r| r.fling.interval.within16_7)
        }),
        cold_start_ms: run_values(&|r| r.cold_start_ms),
        footprint_bytes: run_values(&|r| r.memory.footprint_bytes as f64),
    }
}

/// 중앙값이 목표 안인가(값이 없으면 `None`).
#[derive(Debug, Clone, Serialize)]
pub struct DbTargets {
    pub grid_open_ms: f64,
    pub explorer_ms: f64,
    pub redis_open_ms: f64,
    pub scroll_share: f64,
    pub vsync_ms: f64,
    pub grid_open_met: Option<bool>,
    pub grid_scroll_met: Option<bool>,
    pub grid_stream_met: Option<bool>,
    pub explorer_met: Option<bool>,
    pub redis_open_met: Option<bool>,
    pub redis_scroll_met: Option<bool>,
}

/// 스크롤 판정: 갱신을 놓치지 않은 몫이 [`TARGET_SCROLL_SHARE`] 이상이고 간격 중앙값이 [`TARGET_VSYNC_MS`] 이하(60 fps 이상).
fn scroll_met(within: Option<Summary>, vsync_ms: Option<Summary>) -> Option<bool> {
    Some(within?.median >= TARGET_SCROLL_SHARE && vsync_ms?.median <= TARGET_VSYNC_MS)
}

pub fn db_targets(s: &DbSummary) -> DbTargets {
    DbTargets {
        grid_open_ms: TARGET_GRID_OPEN_MS,
        explorer_ms: TARGET_EXPLORER_MS,
        redis_open_ms: TARGET_REDIS_OPEN_MS,
        scroll_share: TARGET_SCROLL_SHARE,
        vsync_ms: TARGET_VSYNC_MS,
        grid_open_met: s.grid_open_ms.map(|v| v.median < TARGET_GRID_OPEN_MS),
        grid_scroll_met: scroll_met(s.grid_fling_within_vsync, s.grid_fling_vsync_ms),
        grid_stream_met: s
            .grid_stream_frames
            .filter(|f| f.median >= MIN_STREAM_FRAMES)
            .and_then(|_| scroll_met(s.grid_stream_within_vsync, s.grid_stream_vsync_ms)),
        explorer_met: s.explorer_ms.map(|v| v.median < TARGET_EXPLORER_MS),
        redis_open_met: s.redis_open_ms.map(|v| v.median < TARGET_REDIS_OPEN_MS),
        redis_scroll_met: scroll_met(s.redis_fling_within_vsync, s.redis_fling_vsync_ms),
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct DbSettings {
    pub runs: usize,
    pub warmup: usize,
    /// 사람이 붙인 이름(`--label` — 예: 「fake driver bundle」).
    pub label: Option<String>,
    /// postgres 연결이 가짜(`perf-fake-db`)였는가.
    pub fake_db: bool,
    pub connections: Vec<Connection>,
}

/// DB 시나리오의 `result.json`.
#[derive(Debug, Clone, Serialize)]
pub struct DbReport {
    pub schema: u32,
    pub scenario: &'static str,
    pub definitions: DbDefinitions,
    pub build: Build,
    pub commit: Commit,
    pub os: Os,
    pub hardware: Hardware,
    pub settings: DbSettings,
    pub runs: Vec<DbRun>,
    pub failures: Vec<RunFailure>,
    pub valid_runs: usize,
    pub summary: DbSummary,
    pub targets: DbTargets,
    /// 유효한 회가 `--runs` 의 절반보다 적을 때. 있으면 pw-perf 는 3 으로 끝난다.
    pub warning: Option<String>,
    /// 끝까지 하지 못하고 멈춘 까닭. 그때까지의 회는 남긴다.
    pub stopped: Option<String>,
}

fn met(value: Option<bool>) -> &'static str {
    match value {
        Some(true) => "within target",
        Some(false) => "over target",
        None => "no value",
    }
}

fn cell(s: Option<Summary>, unit: &str) -> String {
    s.map_or("—".into(), |s| {
        format!(
            "{:.1}{unit} ({:.1}–{:.1}, n={})",
            s.median, s.min, s.max, s.n
        )
    })
}

fn share_cell(s: Option<Summary>) -> String {
    s.map_or("—".into(), |s| {
        format!("{:.2} ({:.2}–{:.2}, n={})", s.median, s.min, s.max, s.n)
    })
}

/// DB 시나리오의 `summary.md`.
pub fn db_report_markdown(report: &DbReport) -> String {
    let mut md = String::from("## Pitwall DB scenario (M3-21: M3-13 · M3-16 · M3-18)\n\n");
    if let Some(stopped) = &report.stopped {
        md.push_str(&format!(
            "> **Stopped early:** {stopped}. The tables hold the runs before it.\n\n"
        ));
    }
    if let Some(warning) = &report.warning {
        md.push_str(&format!("> **Warning:** {warning}.\n\n"));
    }
    if report.build.profile != "release" {
        md.push_str(&format!(
            "> **Not a baseline:** measured a `{}` build. The targets are judged on release bundles.\n\n",
            report.build.profile
        ));
    }
    md.push_str(&environment(
        &report.build,
        &report.commit,
        &report.os,
        &report.hardware,
    ));
    let st = &report.settings;
    md.push_str(&format!(
        "\n- runs: {} measured ({} valid) + {} warm-up · medians over valid measured runs · project registered trusted\n",
        st.runs, report.valid_runs, st.warmup
    ));
    if let Some(label) = &st.label {
        md.push_str(&format!("- label: {label}\n"));
    }
    let conns: Vec<String> = st
        .connections
        .iter()
        .map(|c| {
            if st.fake_db && c.driver == "postgres" {
                format!("`{}` fake postgres (perf-fake-db)", c.id)
            } else {
                format!(
                    "`{}` {} {}:{}{}",
                    c.id,
                    c.driver,
                    c.host,
                    c.port,
                    c.database
                        .as_deref()
                        .map_or(String::new(), |d| format!("/{d}"))
                )
            }
        })
        .collect();
    md.push_str(&format!("- connections: {}\n", conns.join(" · ")));
    let s = &report.summary;
    let t = &report.targets;
    let scroll_target =
        format!("≥ {TARGET_SCROLL_SHARE} and median interval ≤ {TARGET_VSYNC_MS} ms");
    let rows: Vec<(&str, String, String, Option<bool>)> = vec![
        (
            "run → first row frame (M3-16, 100k × 10)",
            cell(s.grid_open_ms, " ms"),
            format!("< {TARGET_GRID_OPEN_MS} ms"),
            t.grid_open_met,
        ),
        (
            "run → first row drawn (canvas fillText, reference)",
            cell(s.grid_first_draw_ms, " ms"),
            "—".into(),
            None,
        ),
        (
            "run → every row received",
            cell(s.grid_all_rows_ms, " ms"),
            "—".into(),
            None,
        ),
        (
            "grid fling: frames within one display refresh",
            share_cell(s.grid_fling_within_vsync),
            scroll_target.clone(),
            t.grid_scroll_met,
        ),
        (
            "grid fling: median frame interval",
            cell(s.grid_fling_vsync_ms, " ms"),
            "—".into(),
            None,
        ),
        (
            "grid fling: frames within 16.7 ms (reference)",
            share_cell(s.grid_fling_within16_7),
            "—".into(),
            None,
        ),
        (
            "grid fling: frame interval p95",
            cell(s.grid_fling_p95_ms, " ms"),
            "—".into(),
            None,
        ),
        (
            "grid scroll while streaming: frames within one display refresh (judged at ≥ 60 frames)",
            share_cell(s.grid_stream_within_vsync),
            scroll_target.clone(),
            t.grid_stream_met,
        ),
        (
            "grid scroll while streaming: median frame interval",
            cell(s.grid_stream_vsync_ms, " ms"),
            "—".into(),
            None,
        ),
        (
            "grid scroll while streaming: frames within 16.7 ms (reference)",
            share_cell(s.grid_stream_within16_7),
            "—".into(),
            None,
        ),
        (
            "grid scroll while streaming: frames scrolled",
            cell(s.grid_stream_frames, ""),
            "—".into(),
            None,
        ),
        (
            "streamed run → every row received",
            cell(s.grid_stream_all_rows_ms, " ms"),
            "—".into(),
            None,
        ),
        (
            "explorer: Tables group (10,000) ▸ → first screen (M3-13)",
            cell(s.explorer_ms, " ms"),
            format!("< {TARGET_EXPLORER_MS} ms"),
            t.explorer_met,
        ),
        (
            "explorer: connection ▸ → schema row (opens the connection, reference)",
            cell(s.explorer_connect_ms, " ms"),
            "—".into(),
            None,
        ),
        (
            "Redis: [Connect] → first key tree row (M3-18, 1M keys)",
            cell(s.redis_open_ms, " ms"),
            format!("< {TARGET_REDIS_OPEN_MS} ms"),
            t.redis_open_met,
        ),
        (
            "Redis: [Connect] → scan done",
            cell(s.redis_all_keys_ms, " ms"),
            "—".into(),
            None,
        ),
        (
            "Redis: expand group k: (1M keys) → first key row (reference)",
            cell(s.redis_expand_ms, " ms"),
            "—".into(),
            None,
        ),
        (
            "Redis key tree fling: frames within one display refresh",
            share_cell(s.redis_fling_within_vsync),
            scroll_target,
            t.redis_scroll_met,
        ),
        (
            "Redis key tree fling: median frame interval",
            cell(s.redis_fling_vsync_ms, " ms"),
            "—".into(),
            None,
        ),
        (
            "Redis key tree fling: frames within 16.7 ms (reference)",
            share_cell(s.redis_fling_within16_7),
            "—".into(),
            None,
        ),
        (
            "cold start → first frame (reference)",
            cell(s.cold_start_ms, " ms"),
            "—".into(),
            None,
        ),
    ];
    md.push_str("\n| Item | Median (range, n) | Target | Result |\n|---|---|---|---|\n");
    for (item, value, target, result) in rows {
        if value == "—" {
            continue;
        }
        let result = if target == "—" { "—" } else { met(result) };
        md.push_str(&format!("| {item} | {value} | {target} | {result} |\n"));
    }
    md.push_str(&format!(
        "| footprint after the scenario | {} | — | — |\n",
        s.footprint_bytes.map_or("—".into(), |f| format!(
            "{:.1} MB ({:.1}–{:.1})",
            f.median / 1e6,
            f.min / 1e6,
            f.max / 1e6
        ))
    ));
    md.push_str("\n| Run | Excluded | Grid open ms | Grid fling ≤1 refresh | Stream ≤1 refresh | Explorer ms | Redis open ms | Redis fling ≤1 refresh | Grid viewport | Footprint MB |\n|---|---|---|---|---|---|---|---|---|---|\n");
    for r in &report.runs {
        let m = &r.measure;
        let opt = |v: Option<f64>| v.map_or("—".into(), |v| format!("{v:.1}"));
        md.push_str(&format!(
            "| {}{} | {} | {} | {} | {} | {} | {} | {} | {} | {:.1} |\n",
            r.index,
            if r.warmup { " (warm-up)" } else { "" },
            r.excluded.as_deref().unwrap_or("—"),
            opt(m.grid_open.as_ref().map(|g| g.ms)),
            m.grid_scroll
                .as_ref()
                .map_or("—".into(), |g| refresh_share(&g.fling.interval)),
            m.grid_stream
                .as_ref()
                .map_or("—".into(), |g| refresh_share(&g.interval)),
            opt(m.explorer.as_ref().map(|e| e.ms)),
            opt(m.redis_open.as_ref().map(|e| e.ms)),
            m.redis_scroll
                .as_ref()
                .map_or("—".into(), |g| refresh_share(&g.fling.interval)),
            m.grid_scroll.as_ref().map_or("—".into(), |g| format!(
                "{}×{}",
                g.viewport.width, g.viewport.height
            )),
            r.memory.footprint_bytes as f64 / 1e6
        ));
    }
    if !report.failures.is_empty() {
        md.push_str("\n| Failed run | Reason |\n|---|---|\n");
        for f in &report.failures {
            md.push_str(&format!(
                "| {}{} | {} |\n",
                f.index,
                if f.warmup { " (warm-up)" } else { "" },
                f.reason.replace('|', "\\|").replace('\n', " ")
            ));
        }
    }
    md
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mark(event: &str, pid: u32, data: serde_json::Value) -> Mark {
        Mark {
            event: event.into(),
            pid,
            epoch_ms: 1.0,
            window: None,
            hidden: None,
            data: Some(data),
        }
    }

    fn stats(vsync: f64, within: f64) -> serde_json::Value {
        serde_json::json!({"n": 2000, "p50": vsync, "p95": vsync + 1.0, "p99": vsync + 2.0, "max": 30.0,
            "within16_7": 0.5, "over18": 0.01, "vsyncMs": vsync, "withinVsync": within})
    }

    fn full_run(pid: u32, rows: u64, scan: &str) -> Vec<Mark> {
        let viewport = serde_json::json!({"width": 800.0, "height": 400.0});
        vec![
            mark(
                "grid_open",
                pid,
                serde_json::json!({"ms": 120.5, "firstDrawMs": 110.0, "rowsAtFirst": 500, "viewport": viewport}),
            ),
            mark(
                "grid_rows",
                pid,
                serde_json::json!({"rows": rows, "allRowsMs": 900.0, "state": "done"}),
            ),
            mark(
                "grid_scroll",
                pid,
                serde_json::json!({"viewport": viewport, "scrollHeight": 2_400_000.0,
                    "fling": {"interval": stats(17.0, 0.99), "work": stats(2.0, 1.0)}}),
            ),
            mark(
                "grid_stream_scroll",
                pid,
                serde_json::json!({"firstMs": 80.0, "frames": 300, "rowsAtEnd": rows, "allRowsMs": 5000.0,
                    "interval": stats(17.0, 0.97), "work": stats(3.0, 1.0)}),
            ),
            mark(
                "explorer_tables",
                pid,
                serde_json::json!({"ms": 60.0, "connectMs": 40.0, "domRows": 40, "firstTable": "table_00000"}),
            ),
            mark(
                "redis_open",
                pid,
                serde_json::json!({"ms": 200.0, "firstRows": 1}),
            ),
            mark(
                "redis_keys",
                pid,
                serde_json::json!({"allKeysMs": 9000.0, "state": scan, "text": "1,000,000 keys"}),
            ),
            mark(
                "redis_scroll",
                pid,
                serde_json::json!({"expandMs": 500.0, "viewport": viewport, "scrollHeight": 22_000_000.0,
                    "lastKey": "k:999999", "fling": {"interval": stats(17.0, 0.96), "work": stats(2.0, 1.0)}}),
            ),
            mark("scenario_done", pid, serde_json::json!({})),
        ]
    }

    const ALL: Expect = Expect {
        grid: true,
        schema: true,
        redis: true,
    };

    #[test]
    fn a_finished_scenario_is_read_into_a_measure() {
        let DbProgress::Done(m) = progress(&full_run(7, GRID_ROWS, "done"), 7, ALL) else {
            panic!("not done")
        };
        assert_eq!(m.grid_open.unwrap().ms, 120.5);
        assert_eq!(
            m.explorer.unwrap().first_table.as_deref(),
            Some("table_00000")
        );
        assert_eq!(
            m.redis_scroll.unwrap().last_key.as_deref(),
            Some("k:999999")
        );
        // 다른 pid 의 표시는 보지 않는다.
        assert_eq!(
            progress(&full_run(8, GRID_ROWS, "done"), 7, ALL),
            DbProgress::Waiting
        );
    }

    #[test]
    fn a_run_without_every_row_or_a_finished_scan_fails() {
        // 첫 결과만 덜 받았다(스트리밍 결과는 다 받음).
        let mut marks = full_run(7, GRID_ROWS, "done");
        marks[1] = mark(
            "grid_rows",
            7,
            serde_json::json!({"rows": 99_500, "allRowsMs": 900.0, "state": "done"}),
        );
        let DbProgress::Failed(e) = progress(&marks, 7, ALL) else {
            panic!("short grid accepted")
        };
        assert!(e.contains("settled at 99500 rows"), "{e}");
        // 스트리밍 결과만 덜 받았다.
        let mut marks = full_run(7, GRID_ROWS, "done");
        marks[3].data.as_mut().unwrap()["rowsAtEnd"] = 99_000.into();
        let DbProgress::Failed(e) = progress(&marks, 7, ALL) else {
            panic!("short stream accepted")
        };
        assert!(e.contains("streamed result ended at 99000 rows"), "{e}");
        let DbProgress::Failed(e) = progress(&full_run(7, GRID_ROWS, "capped"), 7, ALL) else {
            panic!("capped scan accepted")
        };
        assert!(e.contains("capped"), "{e}");
    }

    #[test]
    fn a_squashed_result_grid_fails_the_run() {
        let mut marks = full_run(7, GRID_ROWS, "done");
        marks[0] = mark(
            "grid_open",
            7,
            serde_json::json!({"ms": 120.5, "firstDrawMs": 110.0, "rowsAtFirst": 500,
                "viewport": {"width": 928.0, "height": 10.0}}),
        );
        let DbProgress::Failed(e) = progress(&marks, 7, ALL) else {
            panic!("a 10 px grid accepted")
        };
        assert!(e.contains("only 10 px tall"), "{e}");
    }

    #[test]
    fn a_missing_step_of_a_written_connection_fails_and_others_may_be_skipped() {
        let marks: Vec<Mark> = full_run(7, GRID_ROWS, "done")
            .into_iter()
            .filter(|m| !m.event.starts_with("redis_"))
            .collect();
        let DbProgress::Failed(e) = progress(&marks, 7, ALL) else {
            panic!("missing redis steps accepted")
        };
        assert!(e.contains("redis_open"), "{e}");
        let only_sql = Expect {
            redis: false,
            ..ALL
        };
        assert!(matches!(progress(&marks, 7, only_sql), DbProgress::Done(_)));
    }

    #[test]
    fn a_scenario_error_is_reported_with_its_step() {
        let marks = vec![mark(
            "scenario_error",
            7,
            serde_json::json!({"step": "grid_open", "message": "timed out"}),
        )];
        assert_eq!(
            progress(&marks, 7, ALL),
            DbProgress::Failed("the scenario failed at `grid_open`: timed out".into())
        );
    }

    #[test]
    fn targets_follow_the_medians() {
        let DbProgress::Done(m) = progress(&full_run(7, GRID_ROWS, "done"), 7, ALL) else {
            panic!()
        };
        let run = |index, warmup| DbRun {
            index,
            warmup,
            excluded: None,
            app_pid: 7,
            group: 7,
            cold_start_ms: 400.0,
            measure: (*m).clone(),
            memory: Memory {
                footprint_bytes: 900_000_000,
                rss_bytes: 0,
                processes: Vec::new(),
            },
            killed: false,
        };
        let s = summarize_db(&[run(0, true), run(1, false), run(2, false)]);
        assert_eq!(s.grid_open_ms.unwrap().n, 2);
        let t = db_targets(&s);
        assert_eq!(t.grid_open_met, Some(true));
        assert_eq!(t.grid_scroll_met, Some(true));
        assert_eq!(t.grid_stream_met, Some(true));
        assert_eq!(t.explorer_met, Some(true));
        assert_eq!(t.redis_open_met, Some(true));
        assert_eq!(t.redis_scroll_met, Some(true));
        // 500 ms 는 「안」 이 아니다(06 의 「< 500ms」).
        let mut slow = (*m).clone();
        slow.grid_open.as_mut().unwrap().ms = 500.0;
        slow.redis_scroll
            .as_mut()
            .unwrap()
            .fling
            .interval
            .within_vsync = Some(0.94);
        let s = summarize_db(&[DbRun {
            measure: slow,
            ..run(1, false)
        }]);
        let t = db_targets(&s);
        assert_eq!(t.grid_open_met, Some(false));
        assert_eq!(t.redis_scroll_met, Some(false));
        // 스트리밍 중 프레임이 적으면 판정하지 않는다.
        let mut quick = (*m).clone();
        quick.grid_stream.as_mut().unwrap().frames = 15;
        quick.grid_stream.as_mut().unwrap().interval.within_vsync = Some(0.8);
        let t = db_targets(&summarize_db(&[DbRun {
            measure: quick,
            ..run(1, false)
        }]));
        assert_eq!(t.grid_stream_met, None);
        // 건너뛴 단계는 값이 없다.
        assert_eq!(db_targets(&summarize_db(&[])).explorer_met, None);
    }

    #[test]
    fn connections_are_written_as_project_config_files() {
        let pg = Connection::parse(GRID, "postgres", "127.0.0.1:55432/perf").unwrap();
        assert_eq!(
            pg.toml(),
            "schema = 1\nname = \"perf-grid\"\ndriver = \"postgres\"\nhost = \"127.0.0.1\"\nport = 55432\nsslmode = \"disable\"\ndatabase = \"perf\"\nuser = \"postgres\"\n"
        );
        let redis = Connection::parse(REDIS, "redis", "localhost:6390").unwrap();
        assert_eq!(redis.database, None);
        assert_eq!(redis.user, None);
        assert!(Connection::parse(REDIS, "redis", "localhost").is_err());
        assert!(Connection::parse(REDIS, "redis", "-oProxy:1").is_err());
        assert!(Connection::parse(REDIS, "redis", "h:notaport").is_err());
        let dir = std::env::temp_dir().join(format!("pw-perf-db-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        write_connection(&dir, &Connection::fake(SCHEMA)).unwrap();
        let text = std::fs::read_to_string(dir.join(".pitwall/db/perf-schema.toml")).unwrap();
        assert!(text.contains("driver = \"postgres\""), "{text}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
