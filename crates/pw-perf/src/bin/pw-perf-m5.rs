//! `pw-perf-m5 --app <번들> --out <폴더> <종류> [옵션]`: M5-22 의 기록 항목(판정하지 않는다 — 결정 M5-D19)을 잰다. `pw-perf` 의
//! 시나리오가 아니라 따로 짓는 실행 파일이다 — `pw-perf` 의 회 준비(임시 `PITWALL_*_DIR` 넷, 미신뢰로 등록한 빈 프로젝트, 책임
//! 프로세스를 끊은 띄우기, `first_frame` 표시, 책임 pid 묶음의 `phys_footprint` 합, pid 로 끄기와 그 회의 웹뷰 저장소 지우기)를
//! 라이브러리로 그대로 쓴다. 차례와 결과 복사는 `scripts/perf-m5.mjs`(`mise run perf-m5`), 방법은 CONTRIBUTING.md 「성능 기준선」 의
//! 「M5 기록 항목」.
//!
//! 종류:
//! - `cli-headless`: 앱 없이 `<번들>/Contents/MacOS/pitwall tool project_list` 의 벽시계(띄우기 직전 → 프로세스 종료). 준비 1 + `--calls`.
//! - `cli-app`: 같은 것을 앱이 떠 있을 때(첫 프레임 `--settle-secs` 뒤부터).
//! - `mcp-idle`: 회마다 앱을 새로 띄우고 첫 프레임 `--settle-secs` 뒤(표본 0)에 `pitwall mcp` 세션 하나를 붙이거나(세션 회) 아무것도
//!   하지 않고(대조 회), 그 뒤 `--every-secs` 마다 `--hold-secs` 까지 앱 인스턴스(앱 + WebKit)의 footprint 를 잰다. 준비 회(대조)
//!   `--warmup` 번 뒤 세션 · 대조를 `--pairs` 짝 번갈아 돈다. 회의 값은 붙은 뒤 `--from-secs` 이후 표본의 중앙값이다 — 붙은 뒤
//!   10초 표본은 WebKit 이 가라앉기 전이라 세션 쪽만 크게 읽힌다(M5-22 첫 측정 §3 ②).
//!
//! 결과는 `<out>/result.json` · `summary.md`, 회마다 `<out>/run-<N>/`(표시 파일 · 앱 로그). 종료 코드: 0 — 결과를 썼다. 3 — 결과는
//! 썼지만 유효한 회(화면 잠금 · 가려진 창 제외)가 요청의 절반보다 적다. 1 — 측정이 실패했다. 앱은 `PITWALL_PERF_PARENT` 로 이
//! 프로세스를 보고 있어 이 프로세스가 신호로 끝나면 스스로 정상 종료한다.

use serde_json::{Value, json};

/// 회 하나의 차례: 준비 회(대조) 뒤 세션 · 대조를 번갈아(세션이 먼저 — 첫 측정과 같다).
fn run_order(warmup: usize, pairs: usize) -> Vec<(bool, bool)> {
    let mut order: Vec<(bool, bool)> = (0..warmup).map(|_| (true, false)).collect();
    for _ in 0..pairs {
        order.push((false, true));
        order.push((false, false));
    }
    order
}

fn median(values: &[f64]) -> Option<f64> {
    pw_perf::stats::summarize(values).map(|s| s.median)
}

fn stats(values: &[f64]) -> Value {
    match pw_perf::stats::summarize(values) {
        Some(s) => json!({"n": values.len(), "median": s.median, "min": s.min, "max": s.max}),
        None => json!({"n": 0, "median": null, "min": null, "max": null}),
    }
}

/// 회의 값: 붙은 뒤 `from_secs` 이후 표본(`offset_s` ≥ `from_secs`)의 footprint 중앙값.
fn settled_mb(samples: &[Value], from_secs: f64) -> Option<f64> {
    let late: Vec<f64> = samples
        .iter()
        .filter(|s| s["offset_s"].as_f64().is_some_and(|o| o >= from_secs))
        .filter_map(|s| s["footprint_mb"].as_f64())
        .collect();
    median(&late)
}

fn at_offset(samples: &[Value], offset: f64) -> Option<f64> {
    samples
        .iter()
        .find(|s| s["offset_s"].as_f64() == Some(offset))
        .and_then(|s| s["footprint_mb"].as_f64())
}

/// 프로세스 이름이 `name` 인 것의 footprint 합(표본 하나).
fn process_mb(sample: &Value, name: &str) -> f64 {
    sample["processes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|p| p["name"].as_str().is_some_and(|n| n.contains(name)))
        .filter_map(|p| p["footprint_mb"].as_f64())
        .sum()
}

/// `mcp-idle` 의 요약: 준비 · 제외 아닌 회만, 세션 · 대조 각각의 회 값(붙은 뒤 `from_secs` 이후 중앙값)과 그 중앙값의 차. 참고로
/// 붙은 뒤 10초 표본도 같은 꼴로 싣는다.
fn mcp_summary(runs: &[Value], from_secs: f64) -> Value {
    let valid = |mcp: bool| {
        runs.iter()
            .filter(move |r| {
                !r["warmup"].as_bool().unwrap_or(false)
                    && r["excluded"].is_null()
                    && r["mcp"].as_bool() == Some(mcp)
            })
            .collect::<Vec<_>>()
    };
    let side = |mcp: bool| {
        let rs = valid(mcp);
        let settled: Vec<f64> = rs
            .iter()
            .filter_map(|r| settled_mb(r["samples"].as_array()?, from_secs))
            .collect();
        let at10: Vec<f64> = rs
            .iter()
            .filter_map(|r| at_offset(r["samples"].as_array()?, 10.0))
            .collect();
        let at0: Vec<f64> = rs
            .iter()
            .filter_map(|r| at_offset(r["samples"].as_array()?, 0.0))
            .collect();
        (settled, at10, at0)
    };
    let (s_settled, s_10, s_0) = side(true);
    let (c_settled, c_10, c_0) = side(false);
    let delta = |a: &[f64], b: &[f64]| match (median(a), median(b)) {
        (Some(x), Some(y)) => json!(((x - y) * 100.0).round() / 100.0),
        _ => Value::Null,
    };
    let proxies: Vec<f64> = valid(true)
        .iter()
        .filter_map(|r| r["session"]["proxy"]["footprint_mb"].as_f64())
        .collect();
    json!({
        "from_secs": from_secs,
        "session": {"settled": stats(&s_settled), "at10": stats(&s_10), "at0": stats(&s_0)},
        "control": {"settled": stats(&c_settled), "at10": stats(&c_10), "at0": stats(&c_0)},
        "settled_delta_mb": delta(&s_settled, &c_settled),
        "at10_delta_mb": delta(&s_10, &c_10),
        "proxy_footprint_mb": stats(&proxies),
    })
}

/// 준비 회를 뺀 벽시계의 요약과 준비 회 값.
fn cli_summary(calls: &[Value]) -> Value {
    let ms: Vec<f64> = calls
        .iter()
        .filter(|c| !c["warmup"].as_bool().unwrap_or(false))
        .filter_map(|c| c["ms"].as_f64())
        .collect();
    let warmup = calls
        .iter()
        .find(|c| c["warmup"].as_bool() == Some(true))
        .and_then(|c| c["ms"].as_f64());
    json!({"wall_ms": stats(&ms), "warmup_ms": warmup})
}

/// CLI 회 하나가 쓸 만한가: 종료 0 이고 출력에 등록한 프로젝트 폴더가 있다.
fn cli_call_ok(exit: Option<i32>, stdout: &str, project: &str) -> Result<(), String> {
    if exit != Some(0) {
        return Err(format!("exited {exit:?}"));
    }
    let listed: Value =
        serde_json::from_str(stdout).map_err(|e| format!("output is not JSON: {e}"))?;
    let canonical = std::fs::canonicalize(project)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| project.to_owned());
    if !(has_string(&listed, project) || has_string(&listed, &canonical)) {
        return Err(format!(
            "the registered project {project} is not in the output"
        ));
    }
    Ok(())
}

/// JSON 값 어딘가에 문자열 `s` 가 그대로 있는가.
fn has_string(v: &Value, s: &str) -> bool {
    match v {
        Value::String(x) => x == s,
        Value::Array(a) => a.iter().any(|x| has_string(x, s)),
        Value::Object(o) => o.values().any(|x| has_string(x, s)),
        _ => false,
    }
}

fn f1(v: &Value) -> String {
    v.as_f64().map_or("—".into(), |x| format!("{x:.1}"))
}

fn range(s: &Value) -> String {
    format!(
        "**{}** ({} – {}, n={})",
        f1(&s["median"]),
        f1(&s["min"]),
        f1(&s["max"]),
        s["n"]
    )
}

fn markdown(result: &Value) -> String {
    let mut out = String::new();
    let head = &result["environment"];
    out.push_str(&format!(
        "## M5 record-only measurement: {}\n\n",
        result["kind"].as_str().unwrap_or("?")
    ));
    out.push_str(&format!(
        "- commit: `{}` (dirty {})\n- os: {}\n- hardware: {}\n- app: `{}`\n- definition: {}\n\n",
        head["commit"].as_str().unwrap_or("?"),
        head["dirty"],
        head["os"].as_str().unwrap_or("?"),
        head["hardware"].as_str().unwrap_or("?"),
        result["app"].as_str().unwrap_or("?"),
        result["definition"].as_str().unwrap_or(""),
    ));
    match result["kind"].as_str() {
        Some("mcp-idle") => {
            let s = &result["summary"];
            out.push_str(&format!(
                "| | after {}s+ (per-run median) | 10 s after attach | at attach (first frame + settle) |\n|---|---|---|---|\n",
                s["from_secs"]
            ));
            for side in ["session", "control"] {
                out.push_str(&format!(
                    "| {side} | {} | {} | {} |\n",
                    range(&s[side]["settled"]),
                    range(&s[side]["at10"]),
                    range(&s[side]["at0"])
                ));
            }
            out.push_str(&format!(
                "\n- session − control, settled: **{} MB** (10 s after attach: {} MB)\n- `pitwall mcp` proxy process itself: {} MB footprint\n\n",
                f1(&s["settled_delta_mb"]),
                f1(&s["at10_delta_mb"]),
                range(&s["proxy_footprint_mb"]),
            ));
            out.push_str("| Run | kind | samples after attach (MB, every step) | pitwall MB first → last | load 1-min start → end | excluded |\n|---|---|---|---|---|---|\n");
            for r in result["runs"].as_array().into_iter().flatten() {
                let samples = r["samples"].as_array().cloned().unwrap_or_default();
                let fps: Vec<String> = samples.iter().map(|x| f1(&x["footprint_mb"])).collect();
                let (first, last) = (samples.first(), samples.last());
                out.push_str(&format!(
                    "| {} | {} | {} | {} → {} | {} → {} | {} |\n",
                    r["index"],
                    if r["warmup"].as_bool() == Some(true) {
                        "warm-up"
                    } else if r["mcp"].as_bool() == Some(true) {
                        "session"
                    } else {
                        "control"
                    },
                    fps.join(" / "),
                    first.map_or("—".into(), |x| format!("{:.1}", process_mb(x, "pitwall"))),
                    last.map_or("—".into(), |x| format!("{:.1}", process_mb(x, "pitwall"))),
                    f1(&r["load1_start"]),
                    f1(&r["load1_end"]),
                    r["excluded"].as_str().unwrap_or(""),
                ));
            }
        }
        _ => {
            let s = &result["summary"];
            out.push_str(&format!(
                "- wall clock (spawn → exit): {} ms, warm-up {} ms\n- CLI log: `connected to the running app` × {}, `serving headless` × {}\n\n",
                range(&s["wall_ms"]),
                f1(&s["warmup_ms"]),
                result["log_connected"],
                result["log_headless"],
            ));
            out.push_str(
                "| Call | ms | exit | stdout bytes | load 1-min |\n|---|---|---|---|---|\n",
            );
            for c in result["calls"].as_array().into_iter().flatten() {
                out.push_str(&format!(
                    "| {}{} | {} | {} | {} | {} |\n",
                    c["i"],
                    if c["warmup"].as_bool() == Some(true) {
                        " (warm-up)"
                    } else {
                        ""
                    },
                    f1(&c["ms"]),
                    c["exit"],
                    c["stdout_bytes"],
                    f1(&c["load1"]),
                ));
            }
        }
    }
    out
}

#[cfg(target_os = "macos")]
mod drive {
    use std::ffi::OsString;
    use std::io::{BufRead, BufReader, Write};
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    use pw_core::config::AppPaths;
    use pw_perf::{macos, marks};
    use serde_json::{Value, json};

    pub struct Opts {
        pub binary: PathBuf,
        pub app: PathBuf,
        pub out: PathBuf,
        pub kind: String,
        pub calls: usize,
        pub settle_secs: u64,
        pub pairs: usize,
        pub warmup: usize,
        pub hold_secs: u64,
        pub every_secs: u64,
        pub from_secs: u64,
    }

    fn epoch_ms() -> f64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0.0, |d| d.as_secs_f64() * 1000.0)
    }

    fn screen_locked() -> bool {
        Command::new("ioreg")
            .args(["-n", "Root", "-d1", "-a"])
            .output()
            .is_ok_and(|o| pw_perf::screen_locked(&String::from_utf8_lossy(&o.stdout)))
    }

    pub fn load1() -> f64 {
        let mut l = [0f64; 3];
        // SAFETY: 셋 칸 배열에 셋을 쓴다.
        unsafe { libc::getloadavg(l.as_mut_ptr(), 3) };
        (l[0] * 100.0).round() / 100.0
    }

    /// 사람 입력이 없었던 초(`HIDIdleTime`). 측정 동안 줄어들면 누군가 기기를 만졌다.
    pub fn hid_idle_secs() -> Option<f64> {
        let out = Command::new("ioreg")
            .args(["-c", "IOHIDSystem", "-d", "4"])
            .output()
            .ok()?;
        let text = String::from_utf8_lossy(&out.stdout);
        let line = text.lines().find(|l| l.contains("\"HIDIdleTime\""))?;
        let ns: f64 = line.split('=').nth(1)?.trim().parse().ok()?;
        Some((ns / 1e9).round())
    }

    pub struct Run {
        root: PathBuf,
        paths: AppPaths,
        pub project: PathBuf,
        marks: PathBuf,
    }

    pub fn setup(scratch: &Path, name: &str) -> Result<Run, String> {
        let root = scratch.join(name);
        let paths = AppPaths::under(&root);
        let socket = pw_mcp::socket::endpoint("key", &paths.state, &std::env::temp_dir(), 0)
            .map_err(|e| format!("socket path: {e}"))?;
        if !socket.starts_with(&paths.state) {
            return Err(format!(
                "{} is too long for the MCP socket — give a shorter TMPDIR",
                root.display()
            ));
        }
        let project = root.join("project");
        std::fs::create_dir_all(&project).map_err(|e| e.to_string())?;
        std::fs::write(project.join("README.md"), "perf baseline project\n")
            .map_err(|e| e.to_string())?;
        pw_perf::register_untrusted(&paths, &project)?;
        Ok(Run {
            marks: root.join("marks.jsonl"),
            root,
            paths,
            project,
        })
    }

    /// 물려받은 `PITWALL_*` 를 지우고 그 회의 네 폴더를 준 환경.
    fn base_env(paths: &AppPaths) -> Vec<(OsString, OsString)> {
        let mut env: Vec<(OsString, OsString)> = std::env::vars_os()
            .filter(|(k, _)| !k.to_string_lossy().starts_with("PITWALL_"))
            .collect();
        for (k, v) in [
            ("PITWALL_CONFIG_DIR", &paths.config),
            ("PITWALL_DATA_DIR", &paths.data),
            ("PITWALL_STATE_DIR", &paths.state),
            ("PITWALL_CACHE_DIR", &paths.cache),
        ] {
            env.push((k.into(), v.as_os_str().into()));
        }
        env
    }

    fn log_of(paths: &AppPaths, prefix: &str) -> String {
        let mut text = String::new();
        if let Ok(entries) = std::fs::read_dir(paths.state.join("logs")) {
            for e in entries.flatten() {
                if e.file_name().to_string_lossy().starts_with(prefix) {
                    text.push_str(&std::fs::read_to_string(e.path()).unwrap_or_default());
                }
            }
        }
        text
    }

    pub struct App {
        launcher: u32,
        pub pid: u32,
        group: u32,
        pub frame_ms: f64,
        frame_hidden: bool,
    }

    pub fn launch(opts: &Opts, run: &Run) -> Result<App, String> {
        if screen_locked() {
            return Err("the screen is locked".into());
        }
        let mut env = base_env(&run.paths);
        env.push((pw_perf::MARK_ENV.into(), run.marks.as_os_str().into()));
        env.push((
            "PITWALL_PERF_PARENT".into(),
            std::process::id().to_string().into(),
        ));
        let t0 = epoch_ms();
        let started = Instant::now();
        let launcher =
            macos::spawn_disclaimed(&opts.binary, &[run.project.as_os_str()], &run.root, &env)
                .map_err(|e| format!("cannot launch: {e}"))?;
        let frame = loop {
            let all = marks::parse(&std::fs::read_to_string(&run.marks).unwrap_or_default());
            if let Some(f) = marks::first(&all, "first_frame", None) {
                break f.clone();
            }
            if started.elapsed() > Duration::from_secs(60) {
                return Err(format!(
                    "no first frame in 60 s: {}",
                    marks::stall_note(&all, epoch_ms(), screen_locked())
                ));
            }
            std::thread::sleep(Duration::from_millis(2));
        };
        let pid = frame.pid;
        if macos::exe_path(pid).as_deref() != Some(opts.binary.as_path()) {
            return Err(format!(
                "the first frame came from pid {pid}, not this bundle"
            ));
        }
        let key = pw_perf::log_field(
            &log_of(&run.paths, "core."),
            "single-instance key",
            "identifier",
        );
        if !key.as_deref().is_some_and(|k| k.contains(".data-")) {
            return Err(format!(
                "the single-instance key {key:?} is not split by $DATA (old bundle?)"
            ));
        }
        let group = macos::responsible_pid(pid).unwrap_or(0);
        if group != pid && group != launcher {
            return Err(format!(
                "the app's responsible process {group} is neither the app nor the launcher"
            ));
        }
        Ok(App {
            launcher,
            pid,
            group,
            frame_ms: frame.epoch_ms - t0,
            frame_hidden: frame.hidden == Some(true),
        })
    }

    fn members(group: u32) -> Vec<u32> {
        macos::all_pids()
            .into_iter()
            .filter(|&p| macos::responsible_pid(p) == Some(group))
            .collect()
    }

    /// pid 들의 footprint · RSS 합과 프로세스별 값.
    pub fn sample(pids: &[u32]) -> Value {
        let mut procs: Vec<(u32, String, u64, u64)> = pids
            .iter()
            .filter_map(|&pid| {
                let u = macos::usage(pid)?;
                let name = macos::exe_path(pid)
                    .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
                    .unwrap_or_else(|| "?".into());
                Some((pid, name, u.footprint, u.resident))
            })
            .collect();
        procs.sort_by_key(|p| std::cmp::Reverse(p.2));
        let fp: u64 = procs.iter().map(|p| p.2).sum();
        let rss: u64 = procs.iter().map(|p| p.3).sum();
        json!({
            "epoch_ms": epoch_ms(),
            "footprint_mb": fp as f64 / 1e6,
            "rss_mb": rss as f64 / 1e6,
            "processes": procs.iter().map(|p| json!({"pid": p.0, "name": p.1, "footprint_mb": p.2 as f64 / 1e6, "rss_mb": p.3 as f64 / 1e6})).collect::<Vec<_>>(),
        })
    }

    /// `until` 까지 0.5초마다 화면 잠금을 본다. 한 번이라도 잠겼으면 true.
    fn idle_until(until: Instant) -> bool {
        let mut locked = false;
        while Instant::now() < until {
            locked |= screen_locked();
            std::thread::sleep(
                Duration::from_millis(500).min(until.saturating_duration_since(Instant::now())),
            );
        }
        locked | screen_locked()
    }

    fn request_quit(pid: u32) {
        let script = format!(
            "ObjC.import('AppKit'); const a = $.NSRunningApplication.runningApplicationWithProcessIdentifier({pid}); if (a) a.terminate;"
        );
        let _ = Command::new("osascript")
            .args(["-l", "JavaScript", "-e", &script])
            .output();
    }

    /// pw-perf 와 같은 순서로 끈다: AppKit 종료 요청 → SIGTERM → SIGKILL. 남은 프로세스 · 단일 인스턴스 소켓이 있으면 Err.
    pub fn shut_down(opts: &Opts, app: &App, run: &Run) -> Result<Value, String> {
        let _ = macos::try_wait(app.launcher);
        let groups = [app.launcher, app.pid, app.group];
        let all = || {
            let mut v: Vec<u32> = groups.iter().flat_map(|&g| members(g)).collect();
            v.sort_unstable();
            v.dedup();
            v
        };
        let apps: Vec<u32> = all()
            .into_iter()
            .filter(|&p| macos::exe_path(p).as_deref() == Some(opts.binary.as_path()))
            .collect();
        for &a in &apps {
            request_quit(a);
        }
        let wait = |pids: &[u32], secs| {
            let until = Instant::now() + Duration::from_secs(secs);
            while pids.iter().any(|&p| macos::alive(p)) && Instant::now() < until {
                std::thread::sleep(Duration::from_millis(50));
            }
        };
        wait(&apps, 10);
        let mut signalled = Vec::new();
        for (sig, secs) in [(libc::SIGTERM, 5), (libc::SIGKILL, 5)] {
            let alive: Vec<u32> = apps.iter().copied().filter(|&p| macos::alive(p)).collect();
            if alive.is_empty() {
                break;
            }
            signalled.push(sig);
            for &p in &alive {
                macos::signal(p, sig);
            }
            wait(&alive, secs);
        }
        if macos::try_wait(app.launcher).ok().flatten().is_none() {
            macos::signal(app.launcher, libc::SIGKILL);
            let _ = macos::try_wait(app.launcher);
        }
        let until = Instant::now() + Duration::from_secs(10);
        while !all().is_empty() && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(100));
        }
        let leftovers = all();
        let log = log_of(&run.paths, "core.");
        let socket_left = pw_perf::log_field(&log, "single-instance key", "identifier")
            .is_some_and(|key| pw_perf::single_instance_socket(&key).exists());
        // 기본이 아닌 $DATA 의 그 회만의 웹뷰 저장소(사람의 기본 저장소는 건드리지 않는다).
        if let (Some(home), Some(store)) = (
            std::env::var_os("HOME"),
            pw_perf::log_field(&log, "webview data store", "store"),
        ) {
            let root =
                PathBuf::from(home).join("Library/WebKit/dev.pitwall.desktop/WebsiteDataStore");
            let _ = std::fs::remove_dir_all(root.join(store.to_lowercase()));
            let _ = std::fs::remove_dir(&root);
        }
        if !leftovers.is_empty() || socket_left {
            return Err(format!(
                "after shutting the app down: processes {leftovers:?} left, single-instance socket left {socket_left}"
            ));
        }
        Ok(json!({"signals": signalled}))
    }

    /// 회의 표시 파일 · 로그를 `dest` 로 옮기고 임시 폴더를 지운다.
    pub fn keep(run: &Run, dest: &Path) {
        let _ = std::fs::create_dir_all(dest.join("logs"));
        let _ = std::fs::copy(&run.marks, dest.join("marks.jsonl"));
        if let Ok(entries) = std::fs::read_dir(run.paths.state.join("logs")) {
            for e in entries.flatten() {
                let _ = std::fs::copy(e.path(), dest.join("logs").join(e.file_name()));
            }
        }
        let _ = std::fs::remove_dir_all(&run.root);
    }

    fn hidden_before(run: &Run, pid: u32, epoch: f64) -> bool {
        marks::parse(&std::fs::read_to_string(&run.marks).unwrap_or_default())
            .iter()
            .any(|m| m.event == "hidden" && m.pid == pid && m.epoch_ms <= epoch)
    }

    /// `pitwall mcp --project <p>` 세션 하나: initialize → initialized → tools/list 뒤 stdin 을 열어 둔다.
    struct Session {
        child: std::process::Child,
        ready_ms: f64,
        tools: usize,
    }

    fn open_session(opts: &Opts, run: &Run) -> Result<Session, String> {
        let started = Instant::now();
        let mut child = Command::new(&opts.binary)
            .arg("mcp")
            .arg("--project")
            .arg(&run.project)
            .env_clear()
            .envs(base_env(&run.paths))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("cannot start pitwall mcp: {e}"))?;
        let mut stdin = child.stdin.take().ok_or("no stdin")?;
        let mut out = BufReader::new(child.stdout.take().ok_or("no stdout")?);
        let mut send = |v: Value| -> Result<(), String> {
            writeln!(stdin, "{v}")
                .and_then(|()| stdin.flush())
                .map_err(|e| format!("mcp stdin: {e}"))
        };
        let mut read = || -> Result<Value, String> {
            let mut line = String::new();
            if out.read_line(&mut line).map_err(|e| e.to_string())? == 0 {
                return Err("pitwall mcp closed its output".into());
            }
            serde_json::from_str(&line).map_err(|e| format!("mcp output {line:?}: {e}"))
        };
        send(
            json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"pw-perf-m5","version":"0"}}}),
        )?;
        let init = read()?;
        if init.get("result").is_none() {
            return Err(format!("initialize: {init}"));
        }
        send(json!({"jsonrpc":"2.0","method":"notifications/initialized"}))?;
        send(json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}))?;
        let tools = loop {
            let v = read()?;
            if v.get("id") == Some(&json!(2)) {
                break v["result"]["tools"].as_array().map_or(0, Vec::len);
            }
        };
        let ready_ms = started.elapsed().as_secs_f64() * 1000.0;
        child.stdin = Some(stdin);
        // 뒤에 오는 알림을 비워 프록시가 가득 찬 파이프에 막히지 않게 한다.
        std::thread::spawn(move || {
            let mut sink = String::new();
            while out.read_line(&mut sink).is_ok_and(|n| n > 0) {
                sink.clear();
            }
        });
        Ok(Session {
            child,
            ready_ms,
            tools,
        })
    }

    pub fn excluded(locked: bool, hidden: bool) -> Value {
        match (locked, hidden) {
            (true, _) => json!("screen locked"),
            (_, true) => json!("window hidden"),
            _ => Value::Null,
        }
    }

    pub fn mcp_idle(
        opts: &Opts,
        scratch: &Path,
        write: &dyn Fn(&[Value]),
    ) -> Result<Vec<Value>, String> {
        let mut runs = Vec::new();
        for (i, (warmup, with_mcp)) in super::run_order(opts.warmup, opts.pairs)
            .into_iter()
            .enumerate()
        {
            let run = setup(scratch, &format!("run-{i}"))?;
            let load_start = load1();
            let hid_start = hid_idle_secs();
            let app = launch(opts, &run)?;
            let mut locked = idle_until(Instant::now() + Duration::from_secs(opts.settle_secs));
            let attach = Instant::now();
            let mut samples = vec![sample(&members(app.group))];
            samples[0]["offset_s"] = json!(0.0);
            let session = if with_mcp {
                Some(open_session(opts, &run))
            } else {
                None
            };
            let mut steps = 0;
            while steps * opts.every_secs < opts.hold_secs {
                steps += 1;
                locked |= idle_until(attach + Duration::from_secs(steps * opts.every_secs));
                let mut s = sample(&members(app.group));
                s["offset_s"] = json!((steps * opts.every_secs) as f64);
                samples.push(s);
            }
            let last_epoch = samples
                .last()
                .and_then(|s| s["epoch_ms"].as_f64())
                .unwrap_or(0.0);
            let session_json = match session {
                Some(Ok(mut s)) => {
                    let proxy = sample(&[s.child.id()]);
                    let connected =
                        log_of(&run.paths, "mcp.").contains("connected to the running app");
                    drop(s.child.stdin.take());
                    let exit = s.child.wait().ok().and_then(|st| st.code());
                    json!({"ready_ms": s.ready_ms, "tools": s.tools, "proxy": proxy, "connected_to_app": connected, "exit": exit})
                }
                Some(Err(e)) => {
                    let _ = shut_down(opts, &app, &run);
                    return Err(format!("run {i}: the MCP session did not start: {e}"));
                }
                None => Value::Null,
            };
            if with_mcp && session_json["connected_to_app"] != json!(true) {
                let _ = shut_down(opts, &app, &run);
                return Err(format!(
                    "run {i}: the MCP session did not connect to the app (served headless?)"
                ));
            }
            let hidden = app.frame_hidden || hidden_before(&run, app.pid, last_epoch);
            let load_end = load1();
            let hid_end = hid_idle_secs();
            let down = shut_down(opts, &app, &run)?;
            keep(&run, &opts.out.join(format!("run-{i}")));
            let fps: Vec<String> = samples
                .iter()
                .map(|s| format!("{:.1}", s["footprint_mb"].as_f64().unwrap_or(0.0)))
                .collect();
            eprintln!(
                "pw-perf-m5: run {i} {}: cold {:.0} ms, footprint {} MB, load {load_start} → {load_end}{}",
                if warmup {
                    "warm-up"
                } else if with_mcp {
                    "session"
                } else {
                    "control"
                },
                app.frame_ms,
                fps.join(" "),
                if locked || hidden { " (excluded)" } else { "" },
            );
            runs.push(json!({
                "index": i, "warmup": warmup, "mcp": with_mcp, "cold_start_ms": app.frame_ms, "app_pid": app.pid,
                "samples": samples, "session": session_json, "excluded": excluded(locked, hidden),
                "load1_start": load_start, "load1_end": load_end, "hid_idle_start_s": hid_start, "hid_idle_end_s": hid_end,
                "shutdown": down,
            }));
            write(&runs);
        }
        Ok(runs)
    }

    /// `pitwall tool project_list` 를 준비 1 + `calls` 번. 앞 둘의 출력을 `dest` 에 남긴다.
    pub fn time_cli(opts: &Opts, run: &Run, dest: &Path) -> Result<Vec<Value>, String> {
        std::fs::create_dir_all(dest).map_err(|e| e.to_string())?;
        let project = run.project.to_string_lossy().into_owned();
        let mut calls = Vec::new();
        for i in 0..=opts.calls {
            let t = Instant::now();
            let o = Command::new(&opts.binary)
                .args(["tool", "project_list"])
                .env_clear()
                .envs(base_env(&run.paths))
                .stdin(Stdio::null())
                .output()
                .map_err(|e| format!("cannot run pitwall tool: {e}"))?;
            let ms = t.elapsed().as_secs_f64() * 1000.0;
            let stdout = String::from_utf8_lossy(&o.stdout);
            if i < 2 {
                let _ = std::fs::write(dest.join(format!("call-{i}.stdout.json")), &o.stdout);
            }
            super::cli_call_ok(o.status.code(), &stdout, &project).map_err(|e| {
                format!(
                    "call {i}: {e}; stderr: {}",
                    String::from_utf8_lossy(&o.stderr)
                        .chars()
                        .take(400)
                        .collect::<String>()
                )
            })?;
            calls.push(json!({"i": i, "warmup": i == 0, "ms": ms, "exit": o.status.code(), "stdout_bytes": o.stdout.len(), "load1": load1()}));
        }
        Ok(calls)
    }

    /// CLI 로그(`$STATE/logs/cli.*`)에서 앱에 붙은 횟수 · 헤드리스로 돈 횟수.
    pub fn cli_log_counts(run: &Run) -> (usize, usize) {
        let log = log_of(&run.paths, "cli.");
        (
            log.matches("connected to the running app").count(),
            log.matches("serving headless").count(),
        )
    }

    pub fn screen_locked_now() -> bool {
        screen_locked()
    }

    pub fn wait_secs(secs: u64) -> bool {
        idle_until(Instant::now() + Duration::from_secs(secs))
    }

    pub fn environment() -> Value {
        let git = |args: &[&str]| {
            Command::new("git")
                .args(args)
                .output()
                .ok()
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        };
        let sw = |flag: &str| {
            Command::new("sw_vers")
                .arg(flag)
                .output()
                .ok()
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        };
        json!({
            "commit": git(&["rev-parse", "HEAD"]),
            "dirty": git(&["status", "--porcelain"]).map(|s| !s.is_empty()),
            "os": format!("macOS {} ({})", sw("-productVersion").unwrap_or_default(), sw("-buildVersion").unwrap_or_default()),
            "hardware": format!(
                "{} · {} · {} CPU",
                macos::sysctl_string("hw.model").unwrap_or_default(),
                macos::sysctl_string("machdep.cpu.brand_string").unwrap_or_default(),
                std::thread::available_parallelism().map_or(0, std::num::NonZero::get),
            ),
        })
    }
}

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
        calls: 20,
        settle_secs: 10,
        pairs: 5,
        warmup: 1,
        hold_secs: 60,
        every_secs: 10,
        from_secs: 40,
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
            "--calls" => opts.calls = number("--calls")? as usize,
            "--settle-secs" => opts.settle_secs = number("--settle-secs")?,
            "--pairs" => opts.pairs = number("--pairs")? as usize,
            "--warmup" => opts.warmup = number("--warmup")? as usize,
            "--hold-secs" => opts.hold_secs = number("--hold-secs")?,
            "--every-secs" => opts.every_secs = number("--every-secs")?.max(1),
            "--from-secs" => opts.from_secs = number("--from-secs")?,
            k @ ("cli-headless" | "cli-app" | "mcp-idle") => kind = Some(k.to_owned()),
            other => return Err(format!("unknown argument {other}")),
        }
    }
    opts.app = app.ok_or("--app <Pitwall.app> is required")?;
    opts.out = out.ok_or("--out <folder> is required")?;
    opts.kind = kind.ok_or("give the kind: cli-headless, cli-app or mcp-idle")?;
    opts.binary = std::fs::canonicalize(opts.app.join("Contents/MacOS/pitwall"))
        .map_err(|e| format!("{}: {e}", opts.app.join("Contents/MacOS/pitwall").display()))?;
    if opts.from_secs > opts.hold_secs {
        return Err("--from-secs is after --hold-secs".into());
    }
    Ok(opts)
}

#[cfg(target_os = "macos")]
fn measure(opts: &drive::Opts) -> Result<u8, String> {
    std::fs::create_dir_all(&opts.out).map_err(|e| e.to_string())?;
    let scratch = std::env::temp_dir().join(format!("pw-m5-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(&scratch).map_err(|e| e.to_string())?;
    let environment = drive::environment();
    let base = json!({
        "schema": "pitwall-perf-m5/1",
        "kind": opts.kind,
        "app": opts.app.display().to_string(),
        "environment": environment,
        "started": format!("{:.0}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64())),
        "hid_idle_start_s": drive::hid_idle_secs(),
    });
    let save = |result: &Value| -> Result<(), String> {
        std::fs::write(
            opts.out.join("result.json"),
            format!(
                "{}\n",
                serde_json::to_string_pretty(result).unwrap_or_default()
            ),
        )
        .and_then(|()| std::fs::write(opts.out.join("summary.md"), markdown(result)))
        .map_err(|e| format!("cannot write the result: {e}"))
    };
    let mut code = 0;
    let outcome = match opts.kind.as_str() {
        "mcp-idle" => {
            let definition = format!(
                "per run a fresh app; at first frame + {}s attach one `pitwall mcp --project` session (initialize, initialized, tools/list, stdin held open) or do nothing (control), then sample the app instance's phys_footprint (responsible-pid group: app + WebKit) every {}s up to {}s; a run's value is the median of its samples from {}s after attach; {} warm-up (control), then {} session/control pairs alternating",
                opts.settle_secs,
                opts.every_secs,
                opts.hold_secs,
                opts.from_secs,
                opts.warmup,
                opts.pairs
            );
            let write = |runs: &[Value]| {
                let mut r = base.clone();
                r["definition"] = json!(definition);
                r["runs"] = json!(runs);
                r["summary"] = mcp_summary(runs, opts.from_secs as f64);
                let _ = save(&r);
            };
            drive::mcp_idle(opts, &scratch, &write).map(|runs| {
                let valid = runs
                    .iter()
                    .filter(|r| r["warmup"] != json!(true) && r["excluded"].is_null())
                    .count();
                if valid * 2 < opts.pairs * 2 {
                    code = 3;
                }
                let mut r = base.clone();
                r["definition"] = json!(definition);
                r["summary"] = mcp_summary(&runs, opts.from_secs as f64);
                r["runs"] = json!(runs);
                r
            })
        }
        kind => (|| {
            let run = drive::setup(&scratch, "cli")?;
            let with_app = kind == "cli-app";
            let mut app = None;
            let mut locked = false;
            if with_app {
                app = Some(drive::launch(opts, &run)?);
                locked = drive::wait_secs(opts.settle_secs);
            }
            let load_start = drive::load1();
            let calls = drive::time_cli(opts, &run, &opts.out.join("outputs"));
            let (connected, headless) = drive::cli_log_counts(&run);
            locked |= drive::screen_locked_now();
            let down = app.as_ref().map(|a| drive::shut_down(opts, a, &run));
            drive::keep(&run, &opts.out.join("run-0"));
            let calls = calls?;
            if let Some(down) = down {
                down?;
            }
            let expected = calls.len();
            let (want, got) = if with_app {
                ("connected to the running app", connected)
            } else {
                ("serving headless", headless)
            };
            if got != expected {
                return Err(format!(
                    "the CLI log has `{want}` {got} times for {expected} calls"
                ));
            }
            let mut r = base.clone();
            r["definition"] = json!(format!(
                "`<bundle>/Contents/MacOS/pitwall tool project_list` wall clock from just before spawn to process exit, 1 warm-up + {} calls back to back, {}",
                opts.calls,
                if with_app {
                    format!(
                        "with the app running (from first frame + {}s)",
                        opts.settle_secs
                    )
                } else {
                    "no app (headless host in the CLI process)".into()
                }
            ));
            r["summary"] = cli_summary(&calls);
            r["calls"] = json!(calls);
            r["log_connected"] = json!(connected);
            r["log_headless"] = json!(headless);
            r["load1_start"] = json!(load_start);
            r["app_pid"] = json!(app.as_ref().map(|a| a.pid));
            r["cold_start_ms"] = json!(app.as_ref().map(|a| a.frame_ms));
            r["excluded"] = drive::excluded(locked, false);
            if locked {
                code = 3;
            }
            Ok(r)
        })(),
    };
    let _ = std::fs::remove_dir_all(&scratch);
    let mut result = outcome?;
    result["hid_idle_end_s"] = json!(drive::hid_idle_secs());
    save(&result)?;
    eprint!("{}", markdown(&result));
    Ok(code)
}

fn main() -> std::process::ExitCode {
    #[cfg(target_os = "macos")]
    {
        let args: Vec<String> = std::env::args().skip(1).collect();
        match parse(&args).and_then(|o| measure(&o)) {
            Ok(code) => std::process::ExitCode::from(code),
            Err(e) => {
                eprintln!("pw-perf-m5: {e}");
                std::process::ExitCode::from(1)
            }
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        eprintln!("pw-perf-m5: macOS only");
        std::process::ExitCode::from(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(index: usize, warmup: bool, mcp: bool, fps: &[f64], excluded: Option<&str>) -> Value {
        let samples: Vec<Value> = fps
            .iter()
            .enumerate()
            .map(|(k, fp)| json!({"offset_s": (k * 10) as f64, "footprint_mb": fp, "processes": [{"name": "pitwall", "footprint_mb": 58.0}]}))
            .collect();
        json!({"index": index, "warmup": warmup, "mcp": mcp, "samples": samples, "excluded": excluded,
               "session": if mcp { json!({"proxy": {"footprint_mb": 6.0}}) } else { Value::Null }})
    }

    /// 준비 회 뒤 세션이 먼저, 그다음 대조 — 첫 측정과 같은 차례다.
    #[test]
    fn warm_up_first_then_session_and_control_alternate() {
        assert_eq!(
            run_order(1, 2),
            vec![
                (true, false),
                (false, true),
                (false, false),
                (false, true),
                (false, false)
            ]
        );
        assert_eq!(run_order(0, 1), vec![(false, true), (false, false)]);
    }

    /// 회의 값은 붙은 뒤 40초 이후 표본만 본다 — 10초의 일시적 증가(첫 측정 +35 MB)는 들지 않는다.
    #[test]
    fn a_run_is_valued_by_its_samples_from_40_s_after_attach() {
        let samples = run(
            0,
            false,
            true,
            &[229.0, 240.0, 213.0, 213.0, 201.0, 203.0, 202.0],
            None,
        );
        assert_eq!(
            settled_mb(samples["samples"].as_array().unwrap(), 40.0),
            Some(202.0)
        );
        assert_eq!(
            at_offset(samples["samples"].as_array().unwrap(), 10.0),
            Some(240.0)
        );
    }

    /// 요약은 준비 · 제외 회를 빼고 세션 − 대조를 회 값의 중앙값끼리 견준다.
    #[test]
    fn the_summary_compares_session_and_control_medians_without_warm_up_or_excluded_runs() {
        let runs = vec![
            run(
                0,
                true,
                false,
                &[300.0, 300.0, 300.0, 300.0, 300.0, 300.0, 300.0],
                None,
            ),
            run(
                1,
                false,
                true,
                &[229.0, 240.0, 213.0, 213.0, 201.0, 203.0, 203.0],
                None,
            ),
            run(
                2,
                false,
                false,
                &[228.0, 205.0, 204.0, 202.0, 202.0, 202.0, 202.0],
                None,
            ),
            run(
                3,
                false,
                true,
                &[226.0, 238.0, 203.0, 203.0, 203.0, 203.0, 203.0],
                None,
            ),
            run(
                4,
                false,
                false,
                &[226.0, 202.0, 202.0, 200.0, 200.0, 200.0, 200.0],
                None,
            ),
            run(
                5,
                false,
                true,
                &[226.0, 999.0, 999.0, 999.0, 999.0, 999.0, 999.0],
                Some("screen locked"),
            ),
        ];
        let s = mcp_summary(&runs, 40.0);
        assert_eq!(s["session"]["settled"]["n"], json!(2));
        assert_eq!(s["control"]["settled"]["n"], json!(2));
        assert_eq!(s["session"]["settled"]["median"], json!(203.0));
        assert_eq!(s["control"]["settled"]["median"], json!(201.0));
        assert_eq!(s["settled_delta_mb"], json!(2.0));
        assert_eq!(s["at10_delta_mb"], json!(35.5));
        assert_eq!(s["proxy_footprint_mb"]["median"], json!(6.0));
    }

    #[test]
    fn the_cli_summary_leaves_the_warm_up_call_out() {
        let calls: Vec<Value> = [15.0, 9.0, 8.0, 10.0]
            .iter()
            .enumerate()
            .map(|(i, ms)| json!({"i": i, "warmup": i == 0, "ms": ms}))
            .collect();
        let s = cli_summary(&calls);
        assert_eq!(s["wall_ms"]["median"], json!(9.0));
        assert_eq!(s["wall_ms"]["n"], json!(3));
        assert_eq!(s["warmup_ms"], json!(15.0));
    }

    /// 종료 0 이어도 등록한 프로젝트가 없는 출력(다른 `$DATA` 를 읽었다)은 회를 실패시킨다.
    #[test]
    fn a_cli_call_counts_only_when_it_lists_the_registered_project() {
        let out = r#"{"projects":[{"id":"p1","root":"/tmp/pw-m5-1/cli/project"}]}"#;
        assert_eq!(
            cli_call_ok(Some(0), out, "/tmp/pw-m5-1/cli/project"),
            Ok(())
        );
        assert!(cli_call_ok(Some(0), r#"{"projects":[]}"#, "/tmp/pw-m5-1/cli/project").is_err());
        assert!(cli_call_ok(Some(2), out, "/tmp/pw-m5-1/cli/project").is_err());
        assert!(cli_call_ok(Some(0), "not json", "/tmp/pw-m5-1/cli/project").is_err());
    }

    #[test]
    fn the_summary_names_the_settled_delta() {
        let runs = vec![
            run(1, false, true, &[1.0, 2.0, 3.0, 4.0, 5.0], None),
            run(2, false, false, &[1.0, 2.0, 3.0, 3.0, 3.0], None),
        ];
        let result = json!({"kind": "mcp-idle", "environment": {}, "runs": runs, "summary": mcp_summary(&runs, 40.0)});
        let md = markdown(&result);
        assert!(
            md.contains("session − control, settled: **2.0 MB**"),
            "{md}"
        );
    }
}
