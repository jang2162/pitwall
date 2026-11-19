//! 측정 앱 인스턴스 하나의 회 준비 · 띄우기 · 재기 · 끄기(macOS). `pw-perf-m5` · `pw-perf-m6`(따로 짓는 기록 · 판정 드라이버 —
//! `src/bin`)이 같이 쓴다. `pw-perf`(main.rs)의 회와 같은 규칙이다: 임시 `PITWALL_*_DIR` 넷, 미신뢰(또는 신뢰)로 등록한 프로젝트,
//! 책임 프로세스를 끊은 띄우기, `first_frame` 표시, 책임 pid 묶음의 `phys_footprint` 합, pid 로 끄기(AppKit 종료 요청 → SIGTERM →
//! SIGKILL)와 그 회의 웹뷰 저장소 지우기. 방법은 CONTRIBUTING.md 「성능 기준선」.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use pw_core::config::AppPaths;
use serde_json::{Value, json};

use crate::{macos, marks};

pub fn epoch_ms() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64() * 1000.0)
}

pub fn screen_locked() -> bool {
    Command::new("ioreg")
        .args(["-n", "Root", "-d1", "-a"])
        .output()
        .is_ok_and(|o| crate::screen_locked(&String::from_utf8_lossy(&o.stdout)))
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

/// 회 하나의 임시 폴더 · 네 폴더 · 프로젝트 · 표시 파일.
pub struct Run {
    pub root: PathBuf,
    pub paths: AppPaths,
    pub project: PathBuf,
    pub marks: PathBuf,
}

/// `scratch/name` 에 회를 준비한다: 빈 프로젝트(`README.md` 하나)를 만들고 미신뢰(`trusted` 면 신뢰)로 등록한다.
pub fn setup(scratch: &Path, name: &str, trusted: bool) -> Result<Run, String> {
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
    if trusted {
        crate::register_trusted(&paths, &project)?;
    } else {
        crate::register_untrusted(&paths, &project)?;
    }
    Ok(Run {
        marks: root.join("marks.jsonl"),
        root,
        paths,
        project,
    })
}

/// 물려받은 `PITWALL_*` 를 지우고 그 회의 네 폴더를 준 환경.
pub fn base_env(paths: &AppPaths) -> Vec<(OsString, OsString)> {
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

/// `$STATE/logs/<prefix>*` 를 이어 붙인 글.
pub fn log_of(paths: &AppPaths, prefix: &str) -> String {
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

/// 띄운 앱 하나.
pub struct App {
    pub launcher: u32,
    pub pid: u32,
    pub group: u32,
    /// 띄우기 직전 → `first_frame`.
    pub frame_ms: f64,
    pub frame_hidden: bool,
    /// 띄우기 직전의 벽시계.
    pub launched_epoch_ms: f64,
}

/// 측정 모드로 띄우고 첫 프레임을 기다린다. `extra` 는 덧붙일 환경(시나리오 이름 등).
pub fn launch(binary: &Path, run: &Run, extra: &[(OsString, OsString)]) -> Result<App, String> {
    if screen_locked() {
        return Err("the screen is locked".into());
    }
    let mut env = base_env(&run.paths);
    env.push((crate::MARK_ENV.into(), run.marks.as_os_str().into()));
    env.push((
        "PITWALL_PERF_PARENT".into(),
        std::process::id().to_string().into(),
    ));
    env.extend(extra.iter().cloned());
    let t0 = epoch_ms();
    let started = Instant::now();
    let launcher = macos::spawn_disclaimed(binary, &[run.project.as_os_str()], &run.root, &env)
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
    if macos::exe_path(pid).as_deref() != Some(binary) {
        return Err(format!(
            "the first frame came from pid {pid}, not this bundle"
        ));
    }
    let key = crate::log_field(
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
        launched_epoch_ms: t0,
    })
}

/// 책임 pid 가 `group` 인 프로세스(앱 + WebKit).
pub fn members(group: u32) -> Vec<u32> {
    macos::all_pids()
        .into_iter()
        .filter(|&p| macos::responsible_pid(p) == Some(group))
        .collect()
}

/// 실행 파일 이름(끝났으면 `?`).
pub fn name_of(pid: u32) -> String {
    macos::exe_path(pid)
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "?".into())
}

/// pid 들의 footprint · RSS 합과 프로세스별 값.
pub fn sample(pids: &[u32]) -> Value {
    let mut procs: Vec<(u32, String, u64, u64)> = pids
        .iter()
        .filter_map(|&pid| {
            let u = macos::usage(pid)?;
            Some((pid, name_of(pid), u.footprint, u.resident))
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

/// `root` 의 자손 pid(부모 pid 를 따라 — 손자 포함). WebKit 은 launchd 의 자식이라 들지 않는다.
pub fn descendants(root: u32) -> Vec<u32> {
    let parents: Vec<(u32, u32)> = macos::all_pids()
        .into_iter()
        .filter_map(|p| Some((p, macos::parent_pid(p)?)))
        .collect();
    crate::lsp::descendants(root, &parents)
        .into_iter()
        .collect()
}

/// `until` 까지 0.5초마다 화면 잠금을 본다. 한 번이라도 잠겼으면 true.
pub fn idle_until(until: Instant) -> bool {
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
pub fn shut_down(binary: &Path, app: &App, run: &Run) -> Result<Value, String> {
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
        .filter(|&p| macos::exe_path(p).as_deref() == Some(binary))
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
    let socket_left = crate::log_field(&log, "single-instance key", "identifier")
        .is_some_and(|key| crate::single_instance_socket(&key).exists());
    // 기본이 아닌 $DATA 의 그 회만의 웹뷰 저장소(사람의 기본 저장소는 건드리지 않는다).
    if let (Some(home), Some(store)) = (
        std::env::var_os("HOME"),
        crate::log_field(&log, "webview data store", "store"),
    ) {
        let root = PathBuf::from(home).join("Library/WebKit/dev.pitwall.desktop/WebsiteDataStore");
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

/// 첫 프레임을 기다리다 실패한 회가 남긴 이 번들의 앱(실행 파일이 `binary` 인 프로세스)을 끈다 — 측정 번들은 측정만 띄운다
/// (사람이 띄운 Pitwall 은 다른 번들이다). 끈 pid 들.
pub fn kill_strays(binary: &Path) -> Vec<u32> {
    let strays: Vec<u32> = macos::all_pids()
        .into_iter()
        .filter(|&p| macos::exe_path(p).as_deref() == Some(binary))
        .collect();
    for &p in &strays {
        request_quit(p);
    }
    let until = Instant::now() + Duration::from_secs(10);
    while strays.iter().any(|&p| macos::alive(p)) && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(100));
    }
    for &p in strays.iter().filter(|&&p| macos::alive(p)) {
        macos::signal(p, libc::SIGKILL);
    }
    strays
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

/// `epoch` 까지 그 pid 의 창이 한 번이라도 가려졌는가.
pub fn hidden_before(run: &Run, pid: u32, epoch: f64) -> bool {
    marks::parse(&std::fs::read_to_string(&run.marks).unwrap_or_default())
        .iter()
        .any(|m| m.event == "hidden" && m.pid == pid && m.epoch_ms <= epoch)
}

pub fn excluded(locked: bool, hidden: bool) -> Value {
    match (locked, hidden) {
        (true, _) => json!("screen locked"),
        (_, true) => json!("window hidden"),
        _ => Value::Null,
    }
}

/// 결과 머리의 커밋 · OS · 하드웨어.
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
