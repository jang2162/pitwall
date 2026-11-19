//! `pw-perf-m6 --app <번들> --out <폴더> <종류> [옵션]`: 17-m6-plan.md M6-22 의 측정 중 `pw-perf`(빈 프로젝트 콜드 스타트 · 유휴 메모리 —
//! M5 끝 측정과 같은 것) 밖의 셋을 잰다. 회 준비 · 띄우기 · 끄기는 `pw_perf::instance`(pw-perf-m5 와 같다). 차례와 결과 복사는
//! `scripts/perf-m6.mjs`(`mise run perf-m6`), 방법은 CONTRIBUTING.md 「성능 기준선」 의 「M6 측정」.
//!
//! 종류:
//! - `idle-procs`(①): 미신뢰로 등록한 빈 프로젝트(실행 · 디버그를 쓰지 않는다)를 띄우고 첫 프레임 `--settle-secs` 뒤부터
//!   `--window-secs` 동안 앱 프로세스(`pitwall` — Host 가 든다)의 깨어남(유휴 · 인터럽트) · CPU 시간, 스레드 수 · 이름, 앱의 자손
//!   프로세스를 1초마다 본다. 실행 관리자의 포트 폴링 스레드(`run-ports`)와 자손 프로세스가 없어야 한다 — 감지 · 포트 폴링 · 실행
//!   관리자가 빈 프로젝트에서 아무것도 띄우지 않는다.
//! - `run-flood`(②): 신뢰로 등록한 빈 프로젝트에 실행 구성 하나(`shell` — `pw-perf-flood` 가 초당 `--flood-rate` 바이트를
//!   `--flood-secs` 초)를 적고 앱 안 시나리오 `run-flood`(src/perf-scenario-run.ts)로 띄운다. 웹뷰가 콘솔의 프레임을 재고, 이
//!   드라이버는 1초마다 앱 프로세스(`pitwall` — Host)의 footprint · 스레드 · 자손을 잰다. 실행이 끝난 뒤 `--after-secs` 더 잰다.
//! - `debug-end`(③): 어댑터마다(`--adapters`, 설치 폴더 `--dap-dir` — `tools_install` 이 받은 것) 실행 구성 하나(`[debug]` 의
//!   launch)를 적고 앱 안 시나리오 `debug-end` 로 디버그 세션을 연다. 끝맺음(`--endings`)은 `exit`(대상이 1초 뒤 스스로 끝난다) ·
//!   `stop`(대상이 60초 자고 시나리오가 4초 뒤 세션을 멈춘다). 25 ms 마다 앱의 자손 프로세스를 보아, `debug_starting` 앞에 자손이
//!   없었는지와 세션이 끝난 때(`debug_ended`)부터 그 세션의 자손(어댑터 · 대상)이 모두 사라진 때까지를 잰다(목표 2초).
//!
//! 결과는 `<out>/result.json` · `summary.md`, 회마다 `<out>/run-<N>/`(표시 파일 · 앱 로그). 종료 코드: 0 — 결과를 썼다(목표 밖이어도
//! 0, 판정은 요약에 적는다). 3 — 결과는 썼지만 유효한 회(화면 잠금 · 가려진 창 제외)가 요청의 절반보다 적다. 1 — 측정이 실패했다.

/// 회 결과의 계산과 요약 글. macOS 의 `drive` 와 시험만 쓴다.
#[cfg(any(target_os = "macos", test))]
mod report {
    use serde_json::{Value, json};

    /// 실행 관리자의 포트 폴링 스레드 이름(`pw_run::runs::ports` — 끝나지 않은 실행이 있는 동안만 돈다).
    pub(super) const PORTS_THREAD: &str = "run-ports";
    /// 세션이 끝난 때부터 그 자손이 모두 사라질 때까지의 목표(17-m6-plan M6-22 ③).
    pub(super) const ADAPTER_GONE_MS: f64 = 2000.0;
    /// 콘솔 프레임의 판정 — 스크롤과 같다(CONTRIBUTING.md 「git 시나리오」 의 「판정」).
    pub(super) const WITHIN_VSYNC: f64 = 0.95;
    pub(super) const VSYNC_MS: f64 = 17.0;
    /// Host 전체 로그 버퍼 상한(M6-P3)과 실행 중 링 하나.
    pub(super) const HOST_RING_TOTAL_MB: f64 = 64.0 * 1.048_576;
    pub(super) const RUN_RING_MB: f64 = 8.0 * 1.048_576;

    pub(super) fn stats(values: &[f64]) -> Value {
        match pw_perf::stats::summarize(values) {
            Some(s) => json!({"n": values.len(), "median": s.median, "min": s.min, "max": s.max}),
            None => json!({"n": 0, "median": null, "min": null, "max": null}),
        }
    }

    fn valid(runs: &[Value]) -> impl Iterator<Item = &Value> {
        runs.iter().filter(|r| r["excluded"].is_null())
    }

    fn nums<'a>(
        runs: impl Iterator<Item = &'a Value>,
        f: impl Fn(&Value) -> Option<f64>,
    ) -> Vec<f64> {
        runs.filter_map(f).collect()
    }

    /// 이름 목록을 (이름, 수)로 — 같은 이름의 스레드가 여럿이다(`tokio-runtime-worker` 등).
    pub(super) fn name_counts(names: &[String]) -> Value {
        let mut counts = std::collections::BTreeMap::<String, usize>::new();
        for n in names {
            *counts
                .entry(if n.is_empty() {
                    "(unnamed)".into()
                } else {
                    n.clone()
                })
                .or_default() += 1;
        }
        json!(counts)
    }

    /// `idle-procs` 의 요약: 회 값의 중앙값, 모든 회에서 본 자손 · 포트 폴링 스레드.
    pub(super) fn idle_summary(runs: &[Value]) -> Value {
        // 프로세스 · 스레드 · 깨어남은 창이 가려져도 같은 것을 본다 — 제외하는 것은 메모리(footprint)뿐이다.
        let all = || runs.iter().filter(|r| r["error"].is_null());
        let descendants: Vec<Value> = all()
            .flat_map(|r| r["descendants"].as_array().cloned().unwrap_or_default())
            .collect();
        let ports = all().any(|r| r["ports_thread_seen"] == json!(true));
        let no_spawn = descendants.is_empty() && !ports;
        json!({
            "idle_wakeups_per_s": stats(&nums(all(), |r| r["host"]["idle_wakeups_per_s"].as_f64())),
            "interrupt_wakeups_per_s": stats(&nums(all(), |r| r["host"]["interrupt_wakeups_per_s"].as_f64())),
            "cpu_ms_per_s": stats(&nums(all(), |r| r["host"]["cpu_ms_per_s"].as_f64())),
            "threads": stats(&nums(all(), |r| r["host"]["threads_end"].as_f64())),
            "footprint_mb": stats(&nums(valid(runs), |r| r["footprint_mb"].as_f64())),
            "pitwall_mb": stats(&nums(valid(runs), |r| r["pitwall_mb"].as_f64())),
            "descendants": descendants,
            "ports_thread_seen": ports,
            "verdict": if all().next().is_none() { "no valid run" } else if no_spawn { "nothing spawned" } else { "something spawned" },
        })
    }

    /// 표본들에서 `before` 앞(기준) · 전체 최댓값 · 마지막.
    pub(super) fn host_growth(samples: &[Value], before: f64) -> Value {
        let mb = |s: &Value| s["pitwall_mb"].as_f64();
        let base: Vec<f64> = samples
            .iter()
            .filter(|s| s["epoch_ms"].as_f64().is_some_and(|t| t < before))
            .filter_map(mb)
            .collect();
        let base = base.last().copied();
        let max = samples
            .iter()
            .filter_map(mb)
            .fold(None::<f64>, |a, x| Some(a.map_or(x, |a| a.max(x))));
        let last = samples.iter().filter_map(mb).next_back();
        let delta = match (base, max) {
            (Some(b), Some(m)) => json!(((m - b) * 100.0).round() / 100.0),
            _ => Value::Null,
        };
        json!({"base_mb": base, "max_mb": max, "last_mb": last, "max_delta_mb": delta})
    }

    /// 한 회의 콘솔 판정: 프레임 간격의 `withinVsync` · `vsyncMs`.
    pub(super) fn frames_ok(frames: &Value) -> Option<bool> {
        let i = &frames["interval"];
        Some(i["withinVsync"].as_f64()? >= WITHIN_VSYNC && i["vsyncMs"].as_f64()? <= VSYNC_MS)
    }

    pub(super) fn flood_summary(runs: &[Value]) -> Value {
        let v = || valid(runs).filter(|r| r["error"].is_null());
        let within = nums(v(), |r| r["frames"]["interval"]["withinVsync"].as_f64());
        let vsync = nums(v(), |r| r["frames"]["interval"]["vsyncMs"].as_f64());
        // Host 메모리는 창이 가려져도 같다 — 실행이 시작된 회 모두.
        let started = || runs.iter().filter(|r| !r["run_started_epoch_ms"].is_null());
        let deltas = nums(started(), |r| r["host"]["max_delta_mb"].as_f64());
        let frames_pass = match (
            pw_perf::stats::summarize(&within),
            pw_perf::stats::summarize(&vsync),
        ) {
            (Some(w), Some(s)) => json!(w.median >= WITHIN_VSYNC && s.median <= VSYNC_MS),
            _ => Value::Null,
        };
        let host_pass = pw_perf::stats::summarize(&deltas)
            .map_or(Value::Null, |d| json!(d.max <= HOST_RING_TOTAL_MB));
        json!({
            "within_vsync": stats(&within),
            "vsync_ms": stats(&vsync),
            "interval_p95_ms": stats(&nums(v(), |r| r["frames"]["interval"]["p95"].as_f64())),
            "interval_max_ms": stats(&nums(v(), |r| r["frames"]["interval"]["max"].as_f64())),
            "work_p95_ms": stats(&nums(v(), |r| r["frames"]["work"]["p95"].as_f64())),
            "block_max_ms": stats(&nums(v(), |r| r["frames"]["blocks"]["maxMs"].as_f64())),
            "console_mb_per_s": stats(&nums(v(), |r| {
                Some(r["frames"]["renderedBytes"].as_f64()? / 1e6 / (r["frames"]["measureMs"].as_f64()? / 1000.0))
            })),
            "written_mb_per_s": stats(&nums(started(), |r| Some(r["flood"]["bytes"].as_f64()? / 1e6 / r["flood"]["secs"].as_f64()?))),
            "host_max_delta_mb": stats(&deltas),
            "host_after_delta_mb": stats(&nums(started(), |r| Some(r["host"]["last_mb"].as_f64()? - r["host"]["base_mb"].as_f64()?))),
            "webcontent_max_delta_mb": stats(&nums(v(), |r| r["webcontent"]["max_delta_mb"].as_f64())),
            "frames_pass": frames_pass,
            "host_within_ring_total": host_pass,
            "host_within_one_ring_plus_8mb": pw_perf::stats::summarize(&deltas).map_or(Value::Null, |d| json!(d.max <= RUN_RING_MB + 8.0)),
        })
    }

    /// 자손 기록(pid → 이름 · 처음 · 마지막으로 본 때 · 처음 없어진 때)에서 세션의 판정.
    pub(super) fn debug_judgement(procs: &[Value], starting: f64, ended: f64) -> Value {
        // 누르기 전에 떠 있던 자손 — 그 앞에 이미 끝난 것(신뢰 프로젝트를 열 때 런타임 환경을 읽는 로그인 셸)은 세지 않는다.
        let before: Vec<&Value> = procs
            .iter()
            .filter(|p| p["first_ms"].as_f64().is_some_and(|t| t < starting))
            .filter(|p| p["gone_ms"].as_f64().is_none_or(|g| g > starting))
            .collect();
        let session: Vec<&Value> = procs
            .iter()
            .filter(|p| p["first_ms"].as_f64().is_some_and(|t| t >= starting))
            .collect();
        let alive_at_end = session.iter().any(|p| p["gone_ms"].is_null());
        let gone = session
            .iter()
            .filter_map(|p| p["gone_ms"].as_f64())
            .fold(None::<f64>, |a, x| Some(a.map_or(x, |a| a.max(x))));
        let gone_after = gone.map(|g| ((g - ended).max(0.0) * 10.0).round() / 10.0);
        json!({
            "before_start": before.len(),
            "session_processes": session.iter().map(|p| json!({"name": p["name"], "pid": p["pid"]})).collect::<Vec<_>>(),
            "still_alive": alive_at_end,
            "all_gone_after_end_ms": gone_after,
            "pass": !alive_at_end && before.is_empty() && !session.is_empty()
                && gone_after.is_some_and(|g| g <= ADAPTER_GONE_MS),
        })
    }

    pub(super) fn debug_summary(runs: &[Value]) -> Value {
        let mut groups = std::collections::BTreeMap::<String, Vec<&Value>>::new();
        // 어댑터가 끝나는 시간은 창의 가려짐과 상관없다 — 오류 없이 끝난 회 모두.
        for r in runs.iter().filter(|r| r["error"].is_null()) {
            groups
                .entry(format!(
                    "{} / {}",
                    r["adapter"].as_str().unwrap_or("?"),
                    r["ending"].as_str().unwrap_or("?")
                ))
                .or_default()
                .push(r);
        }
        let rows: serde_json::Map<String, Value> = groups
            .into_iter()
            .map(|(k, rs)| {
                let gone: Vec<f64> = rs
                    .iter()
                    .filter_map(|r| r["judgement"]["all_gone_after_end_ms"].as_f64())
                    .collect();
                let pass = rs.iter().all(|r| r["judgement"]["pass"] == json!(true));
                (
                    k,
                    json!({"runs": rs.len(), "gone_after_end_ms": stats(&gone), "pass": pass}),
                )
            })
            .collect();
        json!(rows)
    }

    pub(super) fn f1(v: &Value) -> String {
        v.as_f64().map_or("—".into(), |x| format!("{x:.1}"))
    }

    pub(super) fn f2(v: &Value) -> String {
        v.as_f64().map_or("—".into(), |x| format!("{x:.2}"))
    }

    pub(super) fn range(s: &Value) -> String {
        format!(
            "**{}** ({} – {}, n={})",
            f1(&s["median"]),
            f1(&s["min"]),
            f1(&s["max"]),
            s["n"]
        )
    }

    fn range2(s: &Value) -> String {
        format!(
            "**{}** ({} – {}, n={})",
            f2(&s["median"]),
            f2(&s["min"]),
            f2(&s["max"]),
            s["n"]
        )
    }

    pub(super) fn markdown(result: &Value) -> String {
        let mut out = String::new();
        let head = &result["environment"];
        out.push_str(&format!(
            "## M6 measurement: {}\n\n- commit: `{}` (dirty {})\n- os: {}\n- hardware: {}\n- app: `{}`\n- definition: {}\n\n",
            result["kind"].as_str().unwrap_or("?"),
            head["commit"].as_str().unwrap_or("?"),
            head["dirty"],
            head["os"].as_str().unwrap_or("?"),
            head["hardware"].as_str().unwrap_or("?"),
            result["app"].as_str().unwrap_or("?"),
            result["definition"].as_str().unwrap_or(""),
        ));
        let s = &result["summary"];
        match result["kind"].as_str() {
            Some("idle-procs") => {
                out.push_str(&format!(
                    "- verdict: **{}** (descendant processes {}, `{PORTS_THREAD}` thread seen {})\n- `pitwall` idle wakeups/s {}, interrupt wakeups/s {}, CPU ms/s {}, threads {}\n- app instance footprint {} MB, `pitwall` {} MB\n\n",
                    s["verdict"].as_str().unwrap_or("?"),
                    s["descendants"].as_array().map_or(0, Vec::len),
                    s["ports_thread_seen"],
                    range2(&s["idle_wakeups_per_s"]),
                    range2(&s["interrupt_wakeups_per_s"]),
                    range2(&s["cpu_ms_per_s"]),
                    range(&s["threads"]),
                    range(&s["footprint_mb"]),
                    range(&s["pitwall_mb"]),
                ));
                out.push_str("| Run | cold ms | idle wk/s | intr wk/s | CPU ms/s | threads | descendants | excluded |\n|---|---|---|---|---|---|---|---|\n");
                for r in result["runs"].as_array().into_iter().flatten() {
                    out.push_str(&format!(
                        "| {} | {} | {} | {} | {} | {} | {} | {} |\n",
                        r["index"],
                        f1(&r["cold_start_ms"]),
                        f2(&r["host"]["idle_wakeups_per_s"]),
                        f2(&r["host"]["interrupt_wakeups_per_s"]),
                        f2(&r["host"]["cpu_ms_per_s"]),
                        r["host"]["threads_end"],
                        r["descendants"].as_array().map_or(0, Vec::len),
                        r["excluded"].as_str().unwrap_or(""),
                    ));
                }
                if let Some(names) = result["runs"][0]["host"]["thread_names"].as_object() {
                    out.push_str("\nThreads of `pitwall` (run 0, end of window): ");
                    let list: Vec<String> =
                        names.iter().map(|(k, v)| format!("`{k}` × {v}")).collect();
                    out.push_str(&list.join(", "));
                    out.push('\n');
                }
            }
            Some("run-flood") => {
                out.push_str(&format!(
                    "- console frames: withinVsync {} (≥ {WITHIN_VSYNC}), vsyncMs {} (≤ {VSYNC_MS}) → **{}**\n- interval p95 {} ms, max {} ms; rAF work p95 {} ms; longest main-thread block {} ms\n- written {} MB/s by the program; console rendered {} MB/s during the frame window\n- Host (`pitwall`) footprint max − before the run: {} MB (one live ring {:.1} MB, Host total {:.1} MB) → within the Host total **{}**, within one ring + 8 MB {}\n- Host after the run − before: {} MB; WebContent max − before: {} MB\n\n",
                    range2(&s["within_vsync"]),
                    range(&s["vsync_ms"]),
                    s["frames_pass"],
                    range(&s["interval_p95_ms"]),
                    range(&s["interval_max_ms"]),
                    range(&s["work_p95_ms"]),
                    range(&s["block_max_ms"]),
                    range(&s["written_mb_per_s"]),
                    range(&s["console_mb_per_s"]),
                    range(&s["host_max_delta_mb"]),
                    RUN_RING_MB,
                    HOST_RING_TOTAL_MB,
                    s["host_within_ring_total"],
                    s["host_within_one_ring_plus_8mb"],
                    range(&s["host_after_delta_mb"]),
                    range(&s["webcontent_max_delta_mb"]),
                ));
                out.push_str("| Run | withinVsync | vsync ms | p95 ms | max ms | console MB | Host base → max → after MB | notes | excluded / error |\n|---|---|---|---|---|---|---|---|---|\n");
                for r in result["runs"].as_array().into_iter().flatten() {
                    let i = &r["frames"]["interval"];
                    out.push_str(&format!(
                        "| {} | {} | {} | {} | {} | {} | {} → {} → {} | {} | {}{} |\n",
                        r["index"],
                        f2(&i["withinVsync"]),
                        f1(&i["vsyncMs"]),
                        f1(&i["p95"]),
                        f1(&i["max"]),
                        r["frames"]["renderedBytes"]
                            .as_f64()
                            .map_or("—".into(), |b| format!("{:.1}", b / 1e6)),
                        f1(&r["host"]["base_mb"]),
                        f1(&r["host"]["max_mb"]),
                        f1(&r["host"]["last_mb"]),
                        r["done"]["notes"].as_array().map_or(0, Vec::len),
                        r["excluded"].as_str().unwrap_or(""),
                        r["error"].as_str().unwrap_or(""),
                    ));
                }
            }
            Some("debug-end") => {
                out.push_str("| adapter / ending | runs | session end → all its processes gone (ms) | pass (≤ 2000 ms, none before start) |\n|---|---|---|---|\n");
                if let Some(rows) = s.as_object() {
                    for (k, v) in rows {
                        out.push_str(&format!(
                            "| {k} | {} | {} | {} |\n",
                            v["runs"],
                            range(&v["gone_after_end_ms"]),
                            v["pass"]
                        ));
                    }
                }
                out.push_str("\n| Run | adapter | ending | processes before start | session processes | gone after end ms | error |\n|---|---|---|---|---|---|---|\n");
                for r in result["runs"].as_array().into_iter().flatten() {
                    let j = &r["judgement"];
                    let names: Vec<String> = j["session_processes"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(|p| p["name"].as_str().unwrap_or("?").to_owned())
                        .collect();
                    out.push_str(&format!(
                        "| {} | {} | {} | {} | {} | {} | {} |\n",
                        r["index"],
                        r["adapter"].as_str().unwrap_or("?"),
                        r["ending"].as_str().unwrap_or("?"),
                        j["before_start"],
                        names.join(", "),
                        f1(&j["all_gone_after_end_ms"]),
                        r["error"].as_str().unwrap_or(""),
                    ));
                }
            }
            _ => {}
        }
        out
    }
}

#[cfg(target_os = "macos")]
mod drive {
    use std::ffi::OsString;
    use std::path::{Path, PathBuf};
    use std::time::{Duration, Instant};

    use pw_perf::instance::{self, App, Run, epoch_ms};
    use pw_perf::{macos, marks};
    use serde_json::{Value, json};

    pub struct Opts {
        pub binary: PathBuf,
        pub app: PathBuf,
        pub out: PathBuf,
        pub kind: String,
        pub runs: usize,
        pub settle_secs: u64,
        pub window_secs: u64,
        pub flood_rate: u64,
        pub flood_secs: u64,
        pub after_secs: u64,
        pub dap_dir: PathBuf,
        pub adapters: Vec<String>,
        pub endings: Vec<String>,
        /// `idle-procs` 의 프로젝트를 신뢰로 등록한다(실행 구성 없이 — 신뢰 프로젝트는 열 때 런타임 환경을 읽는다).
        pub trusted: bool,
    }

    fn scenario(name: &str) -> [(OsString, OsString); 1] {
        [(pw_perf::SCENARIO_ENV.into(), name.into())]
    }

    fn mark_data(run: &Run, event: &str) -> Option<marks::Mark> {
        let all = marks::parse(&std::fs::read_to_string(&run.marks).unwrap_or_default());
        marks::first(&all, event, None).cloned()
    }

    /// 앱 프로세스 하나의 깨어남 · CPU · 스레드.
    fn host_probe(pid: u32) -> Option<(macos::Usage, Vec<String>)> {
        Some((macos::usage(pid)?, macos::thread_names(pid)))
    }

    fn descendants_named(pid: u32) -> Vec<Value> {
        instance::descendants(pid)
            .into_iter()
            .map(|p| json!({"pid": p, "name": instance::name_of(p)}))
            .collect()
    }

    pub fn idle_procs(
        opts: &Opts,
        scratch: &Path,
        write: &dyn Fn(&[Value]),
    ) -> Result<Vec<Value>, String> {
        let mut runs = Vec::new();
        for i in 0..opts.runs {
            let run = instance::setup(scratch, &format!("run-{i}"), opts.trusted)?;
            let load_start = instance::load1();
            let app = match instance::launch(&opts.binary, &run, &[]) {
                Ok(app) => app,
                Err(e) => {
                    // 첫 프레임 전에 창이 가려지면(사람이 기기를 쓰는 중) 회가 멎는다 — 그 회의 앱을 끄고 다음 회로 간다.
                    let strays = instance::kill_strays(&opts.binary);
                    instance::keep(&run, &opts.out.join(format!("run-{i}")));
                    eprintln!("pw-perf-m6: run {i}: launch failed: {e}");
                    runs.push(json!({"index": i, "error": format!("launch: {e}"), "excluded": "launch failed", "strays_killed": strays}));
                    write(&runs);
                    continue;
                }
            };
            let mut locked =
                instance::idle_until(Instant::now() + Duration::from_secs(opts.settle_secs));
            let group = instance::sample(&instance::members(app.group));
            let (u0, _) = host_probe(app.pid).ok_or("cannot read the app process")?;
            let t0 = Instant::now();
            let threads_start = macos::thread_names(app.pid).len();
            let mut seen: Vec<Value> = Vec::new();
            let mut ports = false;
            for s in 1..=opts.window_secs {
                locked |= instance::idle_until(t0 + Duration::from_secs(s));
                for d in descendants_named(app.pid) {
                    if !seen.iter().any(|x| x["pid"] == d["pid"]) {
                        seen.push(d);
                    }
                }
                ports |= macos::thread_names(app.pid)
                    .iter()
                    .any(|n| n == super::PORTS_THREAD);
            }
            let secs = t0.elapsed().as_secs_f64();
            let (u1, names) = host_probe(app.pid).ok_or("cannot read the app process")?;
            let last_epoch = epoch_ms();
            let hidden = app.frame_hidden || instance::hidden_before(&run, app.pid, last_epoch);
            let load_end = instance::load1();
            let down = instance::shut_down(&opts.binary, &app, &run)?;
            instance::keep(&run, &opts.out.join(format!("run-{i}")));
            let per_s =
                |a: u64, b: u64| ((b.saturating_sub(a)) as f64 / secs * 100.0).round() / 100.0;
            let pitwall_mb = group["processes"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|p| p["pid"] == json!(app.pid))
                .filter_map(|p| p["footprint_mb"].as_f64())
                .sum::<f64>();
            eprintln!(
                "pw-perf-m6: idle-procs run {i}: cold {:.0} ms, idle wakeups {:.2}/s, threads {}, descendants {}{}",
                app.frame_ms,
                per_s(u0.idle_wakeups, u1.idle_wakeups),
                names.len(),
                seen.len(),
                if locked || hidden { " (excluded)" } else { "" }
            );
            runs.push(json!({
                "index": i, "cold_start_ms": app.frame_ms, "app_pid": app.pid,
                "footprint_mb": group["footprint_mb"], "pitwall_mb": pitwall_mb, "group": group,
                "host": {
                    "window_s": secs,
                    "idle_wakeups_per_s": per_s(u0.idle_wakeups, u1.idle_wakeups),
                    "interrupt_wakeups_per_s": per_s(u0.interrupt_wakeups, u1.interrupt_wakeups),
                    "cpu_ms_per_s": ((u1.cpu_ns.saturating_sub(u0.cpu_ns)) as f64 / 1e6 / secs * 100.0).round() / 100.0,
                    "threads_start": threads_start, "threads_end": names.len(),
                    "thread_names": super::name_counts(&names),
                },
                "descendants": seen, "ports_thread_seen": ports,
                "excluded": instance::excluded(locked, hidden),
                "load1_start": load_start, "load1_end": load_end, "shutdown": down,
            }));
            write(&runs);
        }
        Ok(runs)
    }

    fn flood_program() -> Result<PathBuf, String> {
        let me = std::env::current_exe().map_err(|e| e.to_string())?;
        let p = me.with_file_name("pw-perf-flood");
        p.exists()
            .then_some(p.clone())
            .ok_or_else(|| format!("{} is missing — build pw-perf's bins", p.display()))
    }

    fn write_config(project: &Path, id: &str, body: &str) -> Result<(), String> {
        let dir = project.join(".pitwall/run");
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        std::fs::write(dir.join(format!("{id}.toml")), body).map_err(|e| e.to_string())
    }

    fn toml_str(s: &str) -> String {
        serde_json::to_string(s).unwrap_or_default()
    }

    /// 앱 · WebContent · 자손을 1초마다 재며 `done` 표시(또는 `scenario_error`)를 기다린다. 표시를 본 뒤 `after` 더 잰다.
    fn sample_until(
        app: &App,
        run: &Run,
        done: &str,
        timeout: Duration,
        after: Duration,
    ) -> (Vec<Value>, Option<marks::Mark>, bool) {
        let started = Instant::now();
        let mut samples = Vec::new();
        let mut locked = false;
        let mut finished: Option<(Instant, Option<marks::Mark>)> = None;
        loop {
            let tick = Instant::now();
            let group = instance::sample(&instance::members(app.group));
            let procs = group["processes"].as_array().cloned().unwrap_or_default();
            let mb = |pred: &dyn Fn(&Value) -> bool| -> f64 {
                procs
                    .iter()
                    .filter(|p| pred(p))
                    .filter_map(|p| p["footprint_mb"].as_f64())
                    .sum()
            };
            let threads = macos::thread_names(app.pid);
            samples.push(json!({
                "epoch_ms": group["epoch_ms"],
                "footprint_mb": group["footprint_mb"],
                "pitwall_mb": mb(&|p| p["pid"] == json!(app.pid)),
                "webcontent_mb": mb(&|p| p["name"].as_str().is_some_and(|n| n.contains("WebContent"))),
                "threads": threads.len(),
                "ports_thread": threads.iter().any(|n| n == super::PORTS_THREAD),
                "descendants": instance::descendants(app.pid).len(),
            }));
            locked |= instance::screen_locked();
            if finished.is_none() {
                if let Some(m) = mark_data(run, done) {
                    finished = Some((Instant::now(), Some(m)));
                } else if mark_data(run, "scenario_error").is_some() || started.elapsed() > timeout
                {
                    finished = Some((Instant::now(), None));
                }
            }
            if let Some((at, ref m)) = finished
                && (m.is_none() || at.elapsed() >= after)
            {
                return (samples, m.clone(), locked);
            }
            std::thread::sleep(Duration::from_secs(1).saturating_sub(tick.elapsed()));
        }
    }

    fn scenario_error(run: &Run) -> Option<String> {
        mark_data(run, "scenario_error").map(|m| m.data.map(|d| d.to_string()).unwrap_or_default())
    }

    pub fn run_flood(
        opts: &Opts,
        scratch: &Path,
        write: &dyn Fn(&[Value]),
    ) -> Result<Vec<Value>, String> {
        let flood = flood_program()?;
        let mut runs = Vec::new();
        for i in 0..opts.runs {
            let run = instance::setup(scratch, &format!("run-{i}"), true)?;
            let stamp = run.root.join("flood.json");
            let command = format!(
                "{} --rate {} --secs {} --stamp {}",
                flood.display(),
                opts.flood_rate,
                opts.flood_secs,
                stamp.display()
            );
            write_config(
                &run.project,
                "flood",
                &format!(
                    "schema = 1\nname = \"flood\"\ntype = \"shell\"\ncommand = {}\n",
                    toml_str(&command)
                ),
            )?;
            let load_start = instance::load1();
            let app = match instance::launch(&opts.binary, &run, &scenario("run-flood")) {
                Ok(app) => app,
                Err(e) => {
                    // 첫 프레임 전에 창이 가려지면(사람이 기기를 쓰는 중) 회가 멎는다 — 그 회의 앱을 끄고 다음 회로 간다.
                    let strays = instance::kill_strays(&opts.binary);
                    instance::keep(&run, &opts.out.join(format!("run-{i}")));
                    eprintln!("pw-perf-m6: run {i}: launch failed: {e}");
                    runs.push(json!({"index": i, "run_started_epoch_ms": null, "error": format!("launch: {e}"), "excluded": "launch failed", "strays_killed": strays}));
                    write(&runs);
                    continue;
                }
            };
            let timeout = Duration::from_secs(opts.flood_secs + 300);
            let (samples, done, locked) = sample_until(
                &app,
                &run,
                "flood_done",
                timeout,
                Duration::from_secs(opts.after_secs),
            );
            let started = mark_data(&run, "run_started");
            let frames = mark_data(&run, "flood_frames").and_then(|m| m.data);
            let error = scenario_error(&run)
                .or_else(|| done.is_none().then(|| "no flood_done in time".to_owned()));
            let before = started.as_ref().map_or(f64::MAX, |m| m.epoch_ms);
            let host = super::host_growth(&samples, before);
            let web: Vec<Value> = samples
                .iter()
                .map(|s| json!({"epoch_ms": s["epoch_ms"], "pitwall_mb": s["webcontent_mb"]}))
                .collect();
            let webcontent = super::host_growth(&web, before);
            let last_epoch = epoch_ms();
            let hidden = app.frame_hidden || instance::hidden_before(&run, app.pid, last_epoch);
            let stamped: Value = std::fs::read_to_string(&stamp)
                .ok()
                .and_then(|t| serde_json::from_str(&t).ok())
                .unwrap_or(Value::Null);
            let load_end = instance::load1();
            let down = instance::shut_down(&opts.binary, &app, &run)?;
            instance::keep(&run, &opts.out.join(format!("run-{i}")));
            let ports_during = samples.iter().any(|s| s["ports_thread"] == json!(true));
            let ports_at_end = samples
                .last()
                .is_some_and(|s| s["ports_thread"] == json!(true));
            eprintln!(
                "pw-perf-m6: run-flood run {i}: frames {}, Host {} → {} MB, {}{}",
                frames.as_ref().map_or("—".into(), |f| format!(
                    "withinVsync {} vsync {}",
                    f["interval"]["withinVsync"], f["interval"]["vsyncMs"]
                )),
                super::f1(&host["base_mb"]),
                super::f1(&host["max_mb"]),
                error.clone().unwrap_or_default(),
                if locked || hidden { " (excluded)" } else { "" }
            );
            runs.push(json!({
                "index": i, "cold_start_ms": app.frame_ms, "app_pid": app.pid,
                "run_started_epoch_ms": started.map(|m| m.epoch_ms),
                "frames_pass": frames.as_ref().and_then(super::frames_ok),
                "frames": frames, "done": done.and_then(|m| m.data), "flood": stamped,
                "host": host, "webcontent": webcontent,
                "ports_thread_during_run": ports_during, "ports_thread_at_end": ports_at_end,
                "samples": samples, "error": error,
                "excluded": instance::excluded(locked, hidden),
                "load1_start": load_start, "load1_end": load_end, "shutdown": down,
            }));
            write(&runs);
        }
        Ok(runs)
    }

    /// 어댑터의 설치 경로(`tools_install` 이 `--dap-dir` 아래 `data/tools` 에 받은 것)와 설정 id.
    fn adapter_bin(dap_dir: &Path, adapter: &str) -> Result<(String, PathBuf), String> {
        let tools = dap_dir.join("data/tools");
        let (id, package, rel): (&str, &str, &[&str]) = match adapter {
            "codelldb" => ("codelldb", "codelldb", &["extension/adapter/codelldb"]),
            "js-debug" => (
                "js-debug",
                "js-debug-adapter",
                &["pitwall-bin/js-debug-adapter"],
            ),
            "debugpy" => ("debugpy", "debugpy", &["pitwall-bin/debugpy-adapter"]),
            other => {
                return Err(format!(
                    "unknown adapter {other} (codelldb, js-debug, debugpy)"
                ));
            }
        };
        let versions = std::fs::read_dir(tools.join(package)).map_err(|e| {
            format!(
                "{}: {e} — install it with tools_install",
                tools.join(package).display()
            )
        })?;
        for v in versions.flatten() {
            let p = rel.iter().fold(v.path(), |p, r| p.join(r));
            if p.exists() {
                return Ok((
                    id.to_owned(),
                    std::fs::canonicalize(&p).map_err(|e| e.to_string())?,
                ));
            }
        }
        Err(format!("no installed {package} under {}", tools.display()))
    }

    /// 대상 프로그램과 `[debug]` 의 나머지 줄. `secs` 동안 자고 끝난다.
    fn debug_target(
        adapter: &str,
        bin: &Path,
        project: &Path,
        secs: u64,
    ) -> Result<String, String> {
        match adapter {
            "codelldb" => {
                let src = project.join("wait.c");
                std::fs::write(
                    &src,
                    format!("#include <unistd.h>\nint main(void) {{ sleep({secs}); return 0; }}\n"),
                )
                .map_err(|e| e.to_string())?;
                let exe = project.join("wait");
                let o = std::process::Command::new("cc")
                    .args(["-g", "-O0", "-o"])
                    .arg(&exe)
                    .arg(&src)
                    .output()
                    .map_err(|e| format!("cc: {e}"))?;
                if !o.status.success() {
                    return Err(format!("cc failed: {}", String::from_utf8_lossy(&o.stderr)));
                }
                Ok(format!("program = {}", toml_str(&exe.to_string_lossy())))
            }
            "js-debug" => {
                let js = project.join("wait.js");
                std::fs::write(&js, format!("setTimeout(() => {{}}, {});\n", secs * 1000))
                    .map_err(|e| e.to_string())?;
                Ok(format!("program = {}", toml_str(&js.to_string_lossy())))
            }
            "debugpy" => {
                let py = project.join("wait.py");
                std::fs::write(&py, format!("import time\ntime.sleep({secs})\n"))
                    .map_err(|e| e.to_string())?;
                // 설치기가 만든 debugpy 전용 venv 의 python(런처 옆 `venv/` — live_dap.rs 의 `debugpy_python` 과 같다).
                let python = bin
                    .parent()
                    .and_then(Path::parent)
                    .map(|d| d.join("venv/bin/python"))
                    .filter(|p| p.exists())
                    .ok_or("the debugpy venv python is missing")?;
                Ok(format!(
                    "program = {}\npython = {}",
                    toml_str(&py.to_string_lossy()),
                    toml_str(&python.to_string_lossy())
                ))
            }
            other => Err(format!("unknown adapter {other}")),
        }
    }

    pub fn debug_end(
        opts: &Opts,
        scratch: &Path,
        write: &dyn Fn(&[Value]),
    ) -> Result<Vec<Value>, String> {
        let mut runs = Vec::new();
        let mut i = 0;
        for adapter in &opts.adapters {
            let (id, bin) = adapter_bin(&opts.dap_dir, adapter)?;
            for ending in &opts.endings {
                let secs = match ending.as_str() {
                    "exit" => 1,
                    "stop" => 60,
                    other => return Err(format!("unknown ending {other} (exit, stop)")),
                };
                for _ in 0..opts.runs {
                    let run = instance::setup(scratch, &format!("run-{i}"), true)?;
                    std::fs::create_dir_all(&run.paths.config).map_err(|e| e.to_string())?;
                    std::fs::write(
                        run.paths.config.join("settings.toml"),
                        format!("[dap.{id}]\npath = {}\n", toml_str(&bin.to_string_lossy())),
                    )
                    .map_err(|e| e.to_string())?;
                    let rest = debug_target(adapter, &bin, &run.project, secs)?;
                    write_config(
                        &run.project,
                        "app",
                        &format!(
                            "schema = 1\nname = \"app\"\ntype = \"shell\"\ncommand = \"true\"\n[debug]\nadapter = \"{id}\"\n{rest}\n"
                        ),
                    )?;
                    let load_start = instance::load1();
                    let app = match instance::launch(&opts.binary, &run, &scenario("debug-end")) {
                        Ok(app) => app,
                        Err(e) => {
                            // 첫 프레임 전에 창이 가려지면(사람이 기기를 쓰는 중) 회가 멎는다 — 그 회의 앱을 끄고 다음 회로 간다.
                            let strays = instance::kill_strays(&opts.binary);
                            instance::keep(&run, &opts.out.join(format!("run-{i}")));
                            eprintln!("pw-perf-m6: run {i}: launch failed: {e}");
                            runs.push(json!({"index": i, "adapter": adapter, "ending": ending, "error": format!("launch: {e}"), "excluded": "launch failed", "strays_killed": strays}));
                            write(&runs);
                            i += 1;
                            continue;
                        }
                    };
                    // 25 ms 마다 자손을 본다: pid → 이름 · 처음 · 마지막으로 본 때 · 처음 없어진 때.
                    let mut procs: Vec<Value> = Vec::new();
                    let started = Instant::now();
                    let mut done_at: Option<Instant> = None;
                    let mut locked = false;
                    let mut last_lock_check = Instant::now();
                    loop {
                        let now = epoch_ms();
                        let live = instance::descendants(app.pid);
                        for &p in &live {
                            match procs.iter_mut().find(|x| x["pid"] == json!(p) && x["gone_ms"].is_null()) {
                                Some(x) => {
                                    x["last_ms"] = json!(now);
                                    if x["name"] == json!("?") {
                                        x["name"] = json!(instance::name_of(p));
                                    }
                                }
                                None => procs.push(json!({"pid": p, "name": instance::name_of(p), "first_ms": now, "last_ms": now, "gone_ms": null})),
                            }
                        }
                        for x in procs.iter_mut().filter(|x| x["gone_ms"].is_null()) {
                            if !x["pid"]
                                .as_u64()
                                .is_some_and(|p| live.contains(&(p as u32)))
                            {
                                x["gone_ms"] = json!(now);
                            }
                        }
                        if last_lock_check.elapsed() > Duration::from_millis(500) {
                            locked |= instance::screen_locked();
                            last_lock_check = Instant::now();
                        }
                        if done_at.is_none()
                            && (mark_data(&run, "scenario_done").is_some()
                                || mark_data(&run, "scenario_error").is_some()
                                || started.elapsed() > Duration::from_secs(120))
                        {
                            done_at = Some(Instant::now());
                        }
                        // 끝 표시 뒤 5초 — 늦게 사라지는 프로세스도 잡는다.
                        if done_at.is_some_and(|d| d.elapsed() > Duration::from_secs(5)) {
                            break;
                        }
                        std::thread::sleep(Duration::from_millis(25));
                    }
                    // 웹뷰가 누르기 직전에 잰 벽시계(표시가 앱에 닿은 때보다 IPC 하나만큼 이르다).
                    let starting =
                        mark_data(&run, "debug_starting").and_then(|m| m.data?["epochMs"].as_f64());
                    let ended = mark_data(&run, "debug_ended");
                    let error = scenario_error(&run);
                    let judgement = match (
                        starting,
                        ended
                            .as_ref()
                            .and_then(|m| m.data.as_ref()?["epochMs"].as_f64()),
                    ) {
                        (Some(s), Some(e)) => super::debug_judgement(&procs, s, e),
                        _ => json!({"pass": false}),
                    };
                    let hidden =
                        app.frame_hidden || instance::hidden_before(&run, app.pid, epoch_ms());
                    let load_end = instance::load1();
                    let down = instance::shut_down(&opts.binary, &app, &run)?;
                    instance::keep(&run, &opts.out.join(format!("run-{i}")));
                    eprintln!(
                        "pw-perf-m6: debug-end run {i} {adapter}/{ending}: gone after end {} ms, pass {}{}",
                        super::f1(&judgement["all_gone_after_end_ms"]),
                        judgement["pass"],
                        error
                            .as_ref()
                            .map(|e| format!(", error {e}"))
                            .unwrap_or_default()
                    );
                    runs.push(json!({
                        "index": i, "adapter": adapter, "ending": ending, "adapter_path": bin.display().to_string(),
                        "cold_start_ms": app.frame_ms, "app_pid": app.pid,
                        "running": mark_data(&run, "debug_running").and_then(|m| m.data),
                        "ended": ended.and_then(|m| m.data), "starting_epoch_ms": starting,
                        "processes": procs, "judgement": judgement, "error": error,
                        "excluded": instance::excluded(locked, hidden),
                        "load1_start": load_start, "load1_end": load_end, "shutdown": down,
                    }));
                    write(&runs);
                    i += 1;
                }
            }
        }
        Ok(runs)
    }
}

#[cfg(any(target_os = "macos", test))]
use report::*;
#[cfg(target_os = "macos")]
use serde_json::{Value, json};

#[cfg(target_os = "macos")]
fn parse(args: &[String]) -> Result<drive::Opts, String> {
    let mut app = None;
    let mut out = None;
    let mut kind = None;
    let mut opts = drive::Opts {
        binary: std::path::PathBuf::new(),
        app: std::path::PathBuf::new(),
        out: std::path::PathBuf::new(),
        kind: String::new(),
        runs: 3,
        settle_secs: 10,
        window_secs: 30,
        flood_rate: 10_000_000,
        flood_secs: 30,
        after_secs: 15,
        dap_dir: std::path::PathBuf::from("target/dap-adapters"),
        adapters: vec!["codelldb".into(), "js-debug".into(), "debugpy".into()],
        endings: vec!["exit".into(), "stop".into()],
        trusted: false,
    };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = || it.next().ok_or_else(|| format!("{a} needs a value"));
        let mut number = |what: &str| -> Result<u64, String> {
            value()?
                .parse()
                .map_err(|_| format!("{what} needs a number"))
        };
        match a.as_str() {
            "--app" => app = Some(std::path::PathBuf::from(value()?)),
            "--out" => out = Some(std::path::PathBuf::from(value()?)),
            "--runs" => opts.runs = number("--runs")?.max(1) as usize,
            "--settle-secs" => opts.settle_secs = number("--settle-secs")?,
            "--window-secs" => opts.window_secs = number("--window-secs")?.max(1),
            "--flood-rate" => opts.flood_rate = number("--flood-rate")?,
            "--flood-secs" => opts.flood_secs = number("--flood-secs")?.max(1),
            "--after-secs" => opts.after_secs = number("--after-secs")?,
            "--trusted" => opts.trusted = true,
            "--dap-dir" => opts.dap_dir = std::path::PathBuf::from(value()?),
            "--adapters" => opts.adapters = value()?.split(',').map(str::to_owned).collect(),
            "--endings" => opts.endings = value()?.split(',').map(str::to_owned).collect(),
            k @ ("idle-procs" | "run-flood" | "debug-end") => kind = Some(k.to_owned()),
            other => return Err(format!("unknown argument {other}")),
        }
    }
    opts.app = app.ok_or("--app <Pitwall.app> is required")?;
    opts.out = out.ok_or("--out <folder> is required")?;
    opts.kind = kind.ok_or("give the kind: idle-procs, run-flood or debug-end")?;
    opts.binary = std::fs::canonicalize(opts.app.join("Contents/MacOS/pitwall"))
        .map_err(|e| format!("{}: {e}", opts.app.join("Contents/MacOS/pitwall").display()))?;
    Ok(opts)
}

#[cfg(target_os = "macos")]
fn measure(opts: &drive::Opts) -> Result<u8, String> {
    std::fs::create_dir_all(&opts.out).map_err(|e| e.to_string())?;
    let scratch = std::env::temp_dir().join(format!("pw-m6-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(&scratch).map_err(|e| e.to_string())?;
    let definition = match opts.kind.as_str() {
        "idle-procs" => format!(
            "per run a fresh app on an empty {} project (no run configuration, no debug); from first frame + {}s for {}s: `pitwall` (the app process — the Host is in it) idle and interrupt wakeups and CPU time (rusage deltas), its threads by name, and the app's descendant processes every 1 s; {} runs",
            if opts.trusted { "trusted" } else { "untrusted" },
            opts.settle_secs,
            opts.window_secs,
            opts.runs
        ),
        "run-flood" => format!(
            "per run a fresh app on an empty trusted project with one shell run configuration `pw-perf-flood --rate {} --secs {}` started from the title bar (in-app scenario run-flood); the webview measures rAF intervals for 20 s from the first console output; the driver samples `pitwall` and WebContent phys_footprint every 1 s from launch until {}s after the console caught up; Host growth = max − the last sample before `run_started`; {} runs",
            opts.flood_rate, opts.flood_secs, opts.after_secs, opts.runs
        ),
        _ => format!(
            "per run a fresh app on an empty trusted project with one run configuration whose [debug] launches a target that sleeps (exit: 1 s, ends by itself; stop: 60 s, the scenario stops the session after 4 s) under the adapter set by `[dap.<id>] path`; the app's descendants are polled every 25 ms; pass = no descendant before `debug_starting`, and every descendant that appeared after it is gone ≤ 2000 ms after `debug_ended` (exit: when the webview saw the session end; stop: when it called stop); adapters {:?}, endings {:?}, {} runs each",
            opts.adapters, opts.endings, opts.runs
        ),
    };
    let base = json!({
        "schema": "pitwall-perf-m6/1",
        "kind": opts.kind,
        "app": opts.app.display().to_string(),
        "environment": pw_perf::instance::environment(),
        "definition": definition,
        "hid_idle_start_s": pw_perf::instance::hid_idle_secs(),
    });
    let summarize = |runs: &[Value]| match opts.kind.as_str() {
        "idle-procs" => idle_summary(runs),
        "run-flood" => flood_summary(runs),
        _ => debug_summary(runs),
    };
    let save = |runs: &[Value]| -> Result<Value, String> {
        let mut r = base.clone();
        r["summary"] = summarize(runs);
        r["runs"] = json!(runs);
        r["hid_idle_now_s"] = json!(pw_perf::instance::hid_idle_secs());
        std::fs::write(
            opts.out.join("result.json"),
            format!("{}\n", serde_json::to_string_pretty(&r).unwrap_or_default()),
        )
        .and_then(|()| std::fs::write(opts.out.join("summary.md"), markdown(&r)))
        .map_err(|e| format!("cannot write the result: {e}"))?;
        Ok(r)
    };
    let write = |runs: &[Value]| {
        let _ = save(runs);
    };
    let outcome = match opts.kind.as_str() {
        "idle-procs" => drive::idle_procs(opts, &scratch, &write),
        "run-flood" => drive::run_flood(opts, &scratch, &write),
        _ => drive::debug_end(opts, &scratch, &write),
    };
    let _ = std::fs::remove_dir_all(&scratch);
    let runs = outcome?;
    let result = save(&runs)?;
    eprint!("{}", markdown(&result));
    let valid = runs.iter().filter(|r| r["excluded"].is_null()).count();
    Ok(if valid * 2 < runs.len() { 3 } else { 0 })
}

/// `pw-perf-m6 resummarize <result.json>`: 저장된 회 기록(자손 · 표시)으로 판정과 요약을 다시 계산해 그 폴더의 `result.json` ·
/// `summary.md` 를 고쳐 쓴다 — 판정 규칙을 고친 뒤 다시 재지 않고 같은 기록에 적용한다.
#[cfg(target_os = "macos")]
fn resummarize(path: &std::path::Path) -> Result<u8, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut result: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    let mut runs = result["runs"].as_array().cloned().unwrap_or_default();
    let kind = result["kind"].as_str().unwrap_or_default().to_owned();
    if kind == "debug-end" {
        for r in &mut runs {
            let (Some(procs), Some(start), Some(end)) = (
                r["processes"].as_array().cloned(),
                r["starting_epoch_ms"].as_f64(),
                r["ended"]["epochMs"].as_f64(),
            ) else {
                continue;
            };
            r["judgement"] = debug_judgement(&procs, start, end);
        }
    }
    result["summary"] = match kind.as_str() {
        "idle-procs" => idle_summary(&runs),
        "run-flood" => flood_summary(&runs),
        _ => debug_summary(&runs),
    };
    result["runs"] = json!(runs);
    result["resummarized"] = json!(true);
    let dir = path.parent().unwrap_or(std::path::Path::new("."));
    std::fs::write(
        path,
        format!(
            "{}\n",
            serde_json::to_string_pretty(&result).unwrap_or_default()
        ),
    )
    .and_then(|()| std::fs::write(dir.join("summary.md"), markdown(&result)))
    .map_err(|e| e.to_string())?;
    eprint!("{}", markdown(&result));
    Ok(0)
}

fn main() -> std::process::ExitCode {
    #[cfg(target_os = "macos")]
    {
        let args: Vec<String> = std::env::args().skip(1).collect();
        if args.first().map(String::as_str) == Some("resummarize") {
            return match args
                .get(1)
                .ok_or_else(|| "resummarize <result.json>".to_owned())
                .and_then(|p| resummarize(std::path::Path::new(p)))
            {
                Ok(code) => std::process::ExitCode::from(code),
                Err(e) => {
                    eprintln!("pw-perf-m6: {e}");
                    std::process::ExitCode::from(1)
                }
            };
        }
        match parse(&args).and_then(|o| measure(&o)) {
            Ok(code) => std::process::ExitCode::from(code),
            Err(e) => {
                eprintln!("pw-perf-m6: {e}");
                std::process::ExitCode::from(1)
            }
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        eprintln!("pw-perf-m6: macOS only");
        std::process::ExitCode::from(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn host_growth_is_the_max_over_the_last_sample_before_the_run() {
        let samples = vec![
            json!({"epoch_ms": 1.0, "pitwall_mb": 60.0}),
            json!({"epoch_ms": 2.0, "pitwall_mb": 61.0}),
            json!({"epoch_ms": 3.0, "pitwall_mb": 70.5}),
            json!({"epoch_ms": 4.0, "pitwall_mb": 64.0}),
        ];
        let g = host_growth(&samples, 2.5);
        assert_eq!(g["base_mb"], json!(61.0));
        assert_eq!(g["max_mb"], json!(70.5));
        assert_eq!(g["last_mb"], json!(64.0));
        assert_eq!(g["max_delta_mb"], json!(9.5));
    }

    #[test]
    fn a_debug_session_passes_only_when_its_processes_go_within_two_seconds() {
        let p = |first: f64, gone: Option<f64>| json!({"pid": 1, "name": "codelldb", "first_ms": first, "gone_ms": gone});
        assert_eq!(
            debug_judgement(&[p(110.0, Some(1500.0))], 100.0, 1000.0)["pass"],
            json!(true)
        );
        assert_eq!(
            debug_judgement(&[p(110.0, Some(3100.0))], 100.0, 1000.0)["pass"],
            json!(false)
        );
        assert_eq!(
            debug_judgement(&[p(110.0, None)], 100.0, 1000.0)["pass"],
            json!(false)
        );
        // 누르기 전에 끝난 자손(프로젝트를 열 때의 로그인 셸)은 「시작 전의 자손」 이 아니다.
        assert_eq!(
            debug_judgement(
                &[p(20.0, Some(80.0)), p(110.0, Some(1100.0))],
                100.0,
                1000.0
            )["pass"],
            json!(true)
        );
        // 디버그를 시작하기 전에 이미 자손이 있었다 — 「debug.start 때만 뜬다」 가 아니다.
        assert_eq!(
            debug_judgement(
                &[p(50.0, Some(900.0)), p(110.0, Some(1100.0))],
                100.0,
                1000.0
            )["pass"],
            json!(false)
        );
        // 세션 프로세스를 하나도 보지 못했으면 판정하지 않는다(어댑터가 너무 빨리 끝났거나 못 봤다).
        assert_eq!(debug_judgement(&[], 100.0, 1000.0)["pass"], json!(false));
        assert_eq!(
            debug_judgement(&[p(110.0, Some(1500.0))], 100.0, 1000.0)["all_gone_after_end_ms"],
            json!(500.0)
        );
    }

    #[test]
    fn the_idle_verdict_names_any_descendant_or_ports_thread() {
        let run = |desc: serde_json::Value, ports: bool| json!({"excluded": null, "descendants": desc, "ports_thread_seen": ports, "host": {}});
        assert_eq!(
            idle_summary(&[run(json!([]), false)])["verdict"],
            json!("nothing spawned")
        );
        assert_eq!(
            idle_summary(&[run(json!([{"pid": 5, "name": "zsh"}]), false)])["verdict"],
            json!("something spawned")
        );
        assert_eq!(
            idle_summary(&[run(json!([]), true)])["verdict"],
            json!("something spawned")
        );
    }

    #[test]
    fn console_frames_use_the_scroll_judgement() {
        assert_eq!(
            frames_ok(&json!({"interval": {"withinVsync": 0.97, "vsyncMs": 16.0}})),
            Some(true)
        );
        assert_eq!(
            frames_ok(&json!({"interval": {"withinVsync": 0.90, "vsyncMs": 16.0}})),
            Some(false)
        );
        assert_eq!(
            frames_ok(&json!({"interval": {"withinVsync": 0.99, "vsyncMs": 33.0}})),
            Some(false)
        );
        assert_eq!(frames_ok(&json!({})), None);
    }
}
