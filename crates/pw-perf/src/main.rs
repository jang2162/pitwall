//! `pw-perf --app <Pitwall.app>`: 콜드 스타트 · 유휴 메모리를 여러 번 재 `result.json` · `summary.md` 를 쓴다. 무엇을
//! 어떻게 재는지와 종료 코드는 CONTRIBUTING.md 「성능 기준선」 절. macOS 만 잰다.

use std::process::ExitCode;

#[cfg(not(target_os = "macos"))]
fn main() -> ExitCode {
    eprintln!("pw-perf: only macOS is measured");
    ExitCode::from(2)
}

#[cfg(target_os = "macos")]
fn main() -> ExitCode {
    measure::install_signal_handlers();
    let result = measure::run(std::env::args().skip(1).collect());
    // 신호로 멈췄으면 진행 중인 회는 이미 껐다(run_once). 셸 관례대로 128 + 신호 번호로 끝난다.
    if let Some(sig) = measure::interrupted() {
        measure::say(format!(
            "pw-perf: stopped by signal {sig}; the running app was shut down"
        ));
        return ExitCode::from(128 + sig as u8);
    }
    match result {
        Ok(true) => ExitCode::SUCCESS,
        // 유효한 회가 너무 적다(report::too_few_valid). 결과 파일은 썼다.
        Ok(false) => ExitCode::from(3),
        Err(e) => {
            measure::say(format!("pw-perf: {e}"));
            ExitCode::FAILURE
        }
    }
}

#[cfg(target_os = "macos")]
mod measure {
    use std::ffi::{OsStr, OsString};
    use std::fmt::Display;
    use std::io::Write as _;
    use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicI32, Ordering};
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    use pw_core::EventBus;
    use pw_core::config::AppPaths;
    use pw_core::registry::{NewProject, Registry};
    use pw_perf::macos;
    use pw_perf::marks::{self, Mark};
    use pw_perf::report::{
        Build, Commit, DEFINITIONS, Hardware, Memory, Os, Process, Report, Run, Settings, markdown,
        summarize_runs, targets, too_few_valid, valid_runs,
    };

    const USAGE: &str = "usage: pw-perf --app <Pitwall.app | binary> [--runs N] [--warmup N] [--idle-secs S] [--timeout-secs S] [--out DIR]";

    /// 앱이 측정하는 프로세스(이 하네스)의 pid 를 받는 변수(`apps/desktop/src-tauri/src/perf.rs` 의 `PARENT_ENV`).
    const PARENT_ENV: &str = "PITWALL_PERF_PARENT";

    static SIGNAL: AtomicI32 = AtomicI32::new(0);

    extern "C" fn on_signal(sig: libc::c_int) {
        SIGNAL.store(sig, Ordering::SeqCst);
    }

    /// Ctrl+C · 터미널 닫기 · `kill` 로 멈춰도 진행 중인 회를 끄고 끝나게 한다. 앱은 떼어 띄워져(detach.rs) 이 신호가 닿지
    /// 않는다. 처리기는 표식만 남기고, 측정 루프가 그것을 보고 멈춘다. SIGKILL 처럼 처리할 수 없는 끝은 앱 쪽 감시
    /// (`PITWALL_PERF_PARENT`)가 맡는다.
    pub fn install_signal_handlers() {
        for sig in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
            // SAFETY: 처리기는 원자 변수에 쓰기만 한다(async-signal-safe).
            unsafe {
                libc::signal(sig, on_signal as *const () as libc::sighandler_t);
            }
        }
    }

    pub fn interrupted() -> Option<i32> {
        Some(SIGNAL.load(Ordering::SeqCst)).filter(|&s| s != 0)
    }

    fn check_interrupted() -> Result<(), String> {
        match interrupted() {
            Some(sig) => Err(format!("stopped by signal {sig}")),
            None => Ok(()),
        }
    }

    /// stderr 에 한 줄. 터미널이 닫혀(SIGHUP) 쓰지 못해도 패닉하지 않는다 — 정리가 끝까지 돌아야 한다.
    pub fn say(line: impl Display) {
        let _ = writeln!(std::io::stderr(), "{line}");
    }

    struct Options {
        app: PathBuf,
        binary: PathBuf,
        runs: usize,
        warmup: usize,
        idle: Duration,
        timeout: Duration,
        out: PathBuf,
    }

    fn parse(args: Vec<String>) -> Result<Options, String> {
        let mut app = None;
        let mut runs = 5;
        let mut warmup = 1;
        let mut idle = 10;
        let mut timeout = 60;
        let mut out = None;
        let mut it = args.into_iter();
        while let Some(flag) = it.next() {
            let mut value = || {
                it.next()
                    .ok_or_else(|| format!("{flag} needs a value\n{USAGE}"))
            };
            let number = |v: String| v.parse::<u64>().map_err(|e| format!("{flag}: {e}"));
            match flag.as_str() {
                "--app" => app = Some(PathBuf::from(value()?)),
                "--runs" => runs = number(value()?)? as usize,
                "--warmup" => warmup = number(value()?)? as usize,
                "--idle-secs" => idle = number(value()?)?,
                "--timeout-secs" => timeout = number(value()?)?,
                "--out" => out = Some(PathBuf::from(value()?)),
                "-h" | "--help" => return Err(USAGE.into()),
                other => return Err(format!("unknown argument {other}\n{USAGE}")),
            }
        }
        let app = app.ok_or(USAGE)?;
        let app = std::fs::canonicalize(&app).map_err(|e| format!("{}: {e}", app.display()))?;
        // 번들이면 그 안의 실행 파일을 띄운다 — CLI(`pitwall <path>`)가 실행하는 것과 같은 파일이다.
        let binary = if app.extension().is_some_and(|e| e == "app") {
            app.join("Contents/MacOS/pitwall")
        } else {
            app.clone()
        };
        if !binary.is_file() {
            return Err(format!("no app binary at {}", binary.display()));
        }
        if runs == 0 {
            return Err("--runs must be at least 1".into());
        }
        let out = match out {
            Some(dir) => dir,
            None => PathBuf::from("target/perf").join(format!("{}", epoch_ms() as u64)),
        };
        Ok(Options {
            app,
            binary,
            runs,
            warmup,
            idle: Duration::from_secs(idle),
            timeout: Duration::from_secs(timeout),
            out,
        })
    }

    fn epoch_ms() -> f64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0.0, |d| d.as_secs_f64() * 1000.0)
    }

    /// 회 폴더들의 짧은 뿌리(`$TMPDIR/pw-perf-<pid>`). 끝나면(오류 · 신호 · 패닉으로 풀릴 때도) 지운다.
    ///
    /// 왜 `target/perf` 가 아닌가: 앱의 MCP 소켓은 `$STATE/run/<해시>.sock` 이고 103바이트를 넘으면 공용 temp 폴더로 넘어가
    /// 끝난 뒤에도 남는다(pw_mcp::socket::endpoint). 워크트리 아래 `target/perf/…` 는 그 길이를 넘는다. 결과(표시 파일 ·
    /// 앱 로그)만 `--out` 으로 옮긴다.
    struct Scratch(PathBuf);

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// 결과 파일을 썼으면 `Ok`. 값은 유효한 회가 충분한가(아니면 종료 코드 3).
    pub fn run(args: Vec<String>) -> Result<bool, String> {
        let opts = parse(args)?;
        macos::check_responsibility()?;
        std::fs::create_dir_all(&opts.out).map_err(|e| format!("{}: {e}", opts.out.display()))?;
        let out = std::fs::canonicalize(&opts.out).map_err(|e| e.to_string())?;
        let scratch = Scratch(std::env::temp_dir().join(format!("pw-perf-{}", std::process::id())));
        let total = opts.warmup + opts.runs;
        say(format!(
            "pw-perf: {} ({} warm-up + {} measured; each opens one project window) → {}",
            opts.binary.display(),
            opts.warmup,
            opts.runs,
            out.display()
        ));
        let store_root = webkit_store_root(&opts.app);
        let mut runs = Vec::new();
        for index in 0..total {
            check_interrupted()?;
            let run = run_once(
                &opts,
                &scratch.0,
                &out,
                store_root.as_deref(),
                index,
                index < opts.warmup,
            )?;
            say(format!(
                "pw-perf: run {index}{}{}: cold start {:.0} ms, idle footprint {:.1} MB ({} processes)",
                if run.warmup { " (warm-up)" } else { "" },
                run.excluded
                    .as_deref()
                    .map_or(String::new(), |why| format!(" (excluded: {why})")),
                run.cold_start_ms,
                run.memory.footprint_bytes as f64 / 1e6,
                run.memory.processes.len()
            ));
            runs.push(run);
            // 앞 회의 프로세스 정리가 다음 회의 시작과 겹치지 않게 잠깐 쉰다.
            std::thread::sleep(Duration::from_secs(1));
        }
        let summary = summarize_runs(&runs);
        let valid = valid_runs(&runs);
        let warning = too_few_valid(valid, opts.runs);
        let report = Report {
            schema: 1,
            definitions: DEFINITIONS,
            build: Build {
                profile: profile_of(&opts.binary).into(),
                app: opts.app.display().to_string(),
                binary: opts.binary.display().to_string(),
            },
            commit: commit(),
            os: Os {
                name: "macOS".into(),
                version: macos::sysctl_string("kern.osproductversion"),
                build: macos::sysctl_string("kern.osversion"),
            },
            hardware: Hardware {
                model: macos::sysctl_string("hw.model"),
                cpu: macos::sysctl_string("machdep.cpu.brand_string"),
                cpus: macos::sysctl_u64("hw.ncpu").map(|n| n as u32),
                memory_bytes: macos::sysctl_u64("hw.memsize"),
            },
            settings: Settings {
                runs: opts.runs,
                warmup: opts.warmup,
                idle_secs: opts.idle.as_secs(),
            },
            targets: targets(&summary),
            valid_runs: valid,
            warning: warning.clone(),
            summary,
            runs,
        };
        let json = serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?;
        std::fs::write(out.join("result.json"), json + "\n").map_err(|e| e.to_string())?;
        let md = markdown(&report);
        std::fs::write(out.join("summary.md"), &md).map_err(|e| e.to_string())?;
        let _ = writeln!(std::io::stdout(), "{md}");
        say(format!(
            "pw-perf: wrote {} and {}",
            out.join("result.json").display(),
            out.join("summary.md").display()
        ));
        if let Some(warning) = warning {
            say(format!("pw-perf: warning: {warning}"));
        }
        Ok(report.warning.is_none())
    }

    /// 바이너리가 놓인 `target/<프로필>/` 로 빌드 종류를 판정한다.
    fn profile_of(binary: &Path) -> &'static str {
        let path = binary.to_string_lossy();
        if path.contains("/target/release/") {
            "release"
        } else if path.contains("/target/debug/") {
            "debug"
        } else {
            "unknown"
        }
    }

    fn commit() -> Commit {
        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .args(args)
                .output()
                .ok()
                .filter(|o| o.status.success())
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        };
        Commit {
            sha: git(&["rev-parse", "HEAD"]),
            dirty: git(&["status", "--porcelain"]).map(|s| !s.is_empty()),
        }
    }

    fn screen_locked() -> Result<bool, String> {
        let out = std::process::Command::new("ioreg")
            .args(["-n", "Root", "-d1", "-a"])
            .output()
            .map_err(|e| format!("cannot run ioreg: {e}"))?;
        Ok(pw_perf::screen_locked(&String::from_utf8_lossy(
            &out.stdout,
        )))
    }

    /// 기본이 아닌 `$DATA` 로 뜬 앱의 웹뷰 저장소가 생기는 폴더(`~/Library/WebKit/<번들 id>/WebsiteDataStore`). 번들이
    /// 아니면 실행 파일 이름이 그 자리를 정한다.
    fn webkit_store_root(app: &Path) -> Option<PathBuf> {
        let id = if app.extension().is_some_and(|e| e == "app") {
            let out = std::process::Command::new("plutil")
                .args(["-extract", "CFBundleIdentifier", "raw", "-o", "-"])
                .arg(app.join("Contents/Info.plist"))
                .output()
                .ok()?;
            String::from_utf8_lossy(&out.stdout).trim().to_owned()
        } else {
            app.file_name()?.to_string_lossy().into_owned()
        };
        let home = std::env::var_os("HOME")?;
        (!id.is_empty()).then(|| {
            PathBuf::from(home)
                .join("Library/WebKit")
                .join(id)
                .join("WebsiteDataStore")
        })
    }

    fn preflight() -> Result<(), String> {
        if screen_locked()? {
            return Err("the screen is locked — WebKit defers frames while locked, so this would not be a baseline. Unlock and run again".into());
        }
        Ok(())
    }

    /// 한 회의 임시 폴더. `PITWALL_*_DIR` 넷과 프로젝트가 그 아래에 있다.
    struct RunDir {
        root: PathBuf,
        paths: AppPaths,
        project: PathBuf,
        marks: PathBuf,
    }

    impl RunDir {
        fn create(scratch: &Path, index: usize) -> Result<Self, String> {
            let root = scratch.join(format!("run-{index}"));
            let paths = AppPaths::under(&root);
            // MCP 소켓이 `$STATE/run` 에 들어가는지 앱과 같은 규칙으로 본다 — 넘으면 공용 temp 로 가서 남는다.
            let socket = pw_mcp::socket::endpoint("key", &paths.state, &std::env::temp_dir(), 0)
                .map_err(|e| e.to_string())?;
            if !socket.starts_with(&paths.state) {
                return Err(format!(
                    "the run folder {} is too long for the app's MCP socket; set a shorter TMPDIR",
                    root.display()
                ));
            }
            let project = root.join("project");
            std::fs::create_dir_all(&project).map_err(|e| e.to_string())?;
            std::fs::write(project.join("README.md"), "perf baseline project\n")
                .map_err(|e| e.to_string())?;
            let registry = Registry::new(paths.clone(), None, EventBus::new(16));
            registry
                .create(NewProject {
                    root: project.clone(),
                    ..NewProject::default()
                })
                .map_err(|e| format!("cannot register the project: {e}"))?;
            Ok(Self {
                marks: root.join("marks.jsonl"),
                root,
                paths,
                project,
            })
        }

        /// 앱 로그(launcher · 앱이 같은 파일에 쓴다).
        fn log(&self) -> String {
            let dir = self.paths.state.join("logs");
            let mut text = String::new();
            if let Ok(entries) = std::fs::read_dir(&dir) {
                for entry in entries.flatten() {
                    if entry.file_name().to_string_lossy().starts_with("core.") {
                        text.push_str(&std::fs::read_to_string(entry.path()).unwrap_or_default());
                    }
                }
            }
            text
        }

        /// 표시 파일과 앱 로그를 결과 폴더로 옮기고 임시 폴더를 지운다(그 안의 MCP 소켓도 함께 — 앱은 끝날 때 지우지
        /// 않는다).
        fn keep_and_remove(&self, out: &Path) {
            let _ = std::fs::create_dir_all(out.join("logs"));
            let _ = std::fs::copy(&self.marks, out.join("marks.jsonl"));
            if let Ok(entries) = std::fs::read_dir(self.paths.state.join("logs")) {
                for entry in entries.flatten() {
                    let _ = std::fs::copy(entry.path(), out.join("logs").join(entry.file_name()));
                }
            }
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    /// 앱 하나: 임시 `PITWALL_*_DIR` 에 프로젝트를 등록하고 CLI 로 띄워 첫 프레임 · 유휴 메모리를 잰 뒤 pid 로 끈다.
    /// 실패 · 신호 · 패닉 어느 쪽으로 끝나도 띄운 앱을 끄고 임시 폴더를 지운다.
    fn run_once(
        opts: &Options,
        scratch: &Path,
        out: &Path,
        store_root: Option<&Path>,
        index: usize,
        warmup: bool,
    ) -> Result<Run, String> {
        preflight()?;
        let dir = RunDir::create(scratch, index)?;

        let mut env: Vec<(OsString, OsString)> = std::env::vars_os()
            .filter(|(k, _)| {
                // 앱의 동작을 바꾸는 물려받은 변수를 지운다 — 떼어 띄우기 표식 · 개발 서버 · 시험 키 · 다른 폴더.
                !k.to_string_lossy().starts_with("PITWALL_")
            })
            .collect();
        let p = &dir.paths;
        for (key, value) in [
            ("PITWALL_CONFIG_DIR", p.config.as_os_str()),
            ("PITWALL_DATA_DIR", p.data.as_os_str()),
            ("PITWALL_STATE_DIR", p.state.as_os_str()),
            ("PITWALL_CACHE_DIR", p.cache.as_os_str()),
            (pw_perf::MARK_ENV, dir.marks.as_os_str()),
        ] {
            env.push((key.into(), value.into()));
        }
        env.push((PARENT_ENV.into(), std::process::id().to_string().into()));

        let started = Instant::now();
        let t0 = epoch_ms();
        let launcher =
            macos::spawn_disclaimed(&opts.binary, &[dir.project.as_os_str()], &dir.root, &env)
                .map_err(|e| format!("cannot launch {}: {e}", opts.binary.display()))?;

        let observed = catch_unwind(AssertUnwindSafe(|| {
            observe(opts, launcher, started, t0, &dir, index, warmup)
        }));
        // 이 회의 것: 띄운 pid, 표시 파일에 `main` 을 남긴 pid(떼어 띄운 앱), 그 앱의 묶음 키. 표시 파일 경로는 이 측정이
        // 준 환경을 받은 프로세스만 안다.
        let marked = marks::parse(&std::fs::read_to_string(&dir.marks).unwrap_or_default());
        let mut groups = vec![launcher];
        groups.extend(marked_pids(&marked, launcher));
        if let Ok(Ok(run)) = &observed {
            groups.push(run.group);
        }
        let (killed, mut leftovers) = shut_down(opts, launcher, &groups);
        let log = dir.log();
        let mut sockets = Vec::new();
        if let Some(key) = pw_perf::log_field(&log, "single-instance key", "identifier") {
            let socket = pw_perf::single_instance_socket(&key);
            if socket.exists() {
                sockets.push(socket);
            }
        }
        // 웹뷰 저장소는 이 회의 `$DATA` 로 정해져 다시 쓰이지 않는다 — 지운다(사람의 기본 저장소는 건드리지 않는다).
        if let (Some(root), Some(store)) = (
            store_root,
            pw_perf::log_field(&log, "webview data store", "store"),
        ) {
            let _ = std::fs::remove_dir_all(root.join(store.to_lowercase()));
            // 이 측정이 처음 만든 폴더면 비어 있다 — 비었을 때만 지운다(다른 앱의 저장소가 있으면 남는다).
            let _ = std::fs::remove_dir(root);
        }
        dir.keep_and_remove(&out.join(format!("run-{index}")));
        let leftover_note = |pids: &[u32], sockets: &[PathBuf]| {
            if pids.is_empty() && sockets.is_empty() {
                String::new()
            } else {
                format!(" (left behind: processes {pids:?}, sockets {sockets:?})")
            }
        };
        let run = match observed {
            Err(panic) => resume_unwind(panic),
            Ok(Err(e)) => return Err(format!("{e}{}", leftover_note(&leftovers, &sockets))),
            Ok(Ok(run)) => run,
        };
        if !leftovers.is_empty() || !sockets.is_empty() {
            return Err(format!(
                "run {index} did not clean up after quitting{}",
                leftover_note(&leftovers, &sockets)
            ));
        }
        leftovers.clear();
        Ok(Run {
            killed,
            leftovers,
            ..run
        })
    }

    fn observe(
        opts: &Options,
        launcher: u32,
        started: Instant,
        t0: f64,
        dir: &RunDir,
        index: usize,
        warmup: bool,
    ) -> Result<Run, String> {
        let read_marks = || marks::parse(&std::fs::read_to_string(&dir.marks).unwrap_or_default());
        let mut launcher_exit = None;
        let frame: Mark = loop {
            check_interrupted()?;
            if launcher_exit.is_none() {
                launcher_exit = macos::try_wait(launcher).map_err(|e| e.to_string())?;
            }
            let all = read_marks();
            if let Some(frame) = marks::first(&all, "first_frame", None) {
                break frame.clone();
            }
            // 떼어 띄운 앱도 같은 표시 파일에 `main` 을 남긴다. 띄운 프로세스가 끝났는데 그런 프로세스가 없거나 모두
            // 끝났으면 인자가 다른 인스턴스로 넘어갔거나(단일 인스턴스) 앱이 죽은 것이다.
            if launcher_exit.is_some() && started.elapsed() > Duration::from_secs(2) {
                let apps = marked_pids(&all, launcher);
                if apps.is_empty() {
                    return Err(format!(
                        "run {index}: the launch exited ({launcher_exit:?}) without starting an app — another Pitwall with the same single-instance key took the arguments"
                    ));
                }
                if !apps.iter().any(|&p| macos::alive(p)) {
                    return Err(format!(
                        "run {index}: the app {apps:?} exited before drawing its first frame"
                    ));
                }
            }
            if started.elapsed() > opts.timeout {
                return Err(format!(
                    "run {index}: no first-frame mark within {}s (is the window hidden or the screen locked?)",
                    opts.timeout.as_secs()
                ));
            }
            std::thread::sleep(Duration::from_millis(2));
        };
        let app_pid = frame.pid;
        // 표시를 낸 프로세스가 이 측정이 띄운 실행 파일인지 본다.
        if macos::exe_path(app_pid).as_deref() != Some(opts.binary.as_path()) {
            return Err(format!(
                "run {index}: the first-frame mark came from pid {app_pid}, which is not {}",
                opts.binary.display()
            ));
        }
        // 이 회의 앱이 정식 앱과 키를 나눠 쓰지 않는지 본다(기본이 아닌 `$DATA` 는 키에 해시가 붙는다, instance.rs).
        // 그 규칙이 없는 빌드면 측정 중에 사람이 띄운 release Pitwall 의 인자가 이 앱으로 올 수 있다.
        let key = pw_perf::log_field(&dir.log(), "single-instance key", "identifier");
        if !key
            .as_deref()
            .is_some_and(|k| k.contains(".data-") || k.contains(".dev-"))
        {
            return Err(format!(
                "run {index}: the app's single-instance key {key:?} is not split by its data dir; rebuild the bundle from this tree"
            ));
        }
        // 묶음 키: 앱의 책임 프로세스. 끊어 띄운 launcher 의 자식은 자기 pid 를 책임 프로세스로 갖는다(launcher 일 수도
        // 있다). 다른 값(터미널 호스트 등)이면 그 아래의 다른 프로세스가 섞이므로 재지 않는다.
        let group = macos::responsible_pid(app_pid).unwrap_or(0);
        if group != app_pid && group != launcher {
            return Err(format!(
                "run {index}: the app {app_pid} is responsible to {group}, not to itself or the launch {launcher} — its WebKit processes cannot be told apart"
            ));
        }
        let cold_start_ms = frame.epoch_ms - t0;

        // 유휴: 표본 시각까지 기다리며 신호와 화면 잠금을 본다(중간에 잠겼다 풀린 것도 잡는다).
        let sample_at = Instant::now() + opts.idle;
        let mut locked = false;
        let mut next_lock_check = Instant::now();
        while Instant::now() < sample_at {
            check_interrupted()?;
            if Instant::now() >= next_lock_check {
                locked |= screen_locked()?;
                next_lock_check = Instant::now() + Duration::from_millis(500);
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        locked |= screen_locked()?;
        let sampled_epoch = epoch_ms();
        let memory = sample(group);
        if !memory.processes.iter().any(|p| p.pid == app_pid) {
            return Err(format!(
                "run {index}: the app {app_pid} exited before the idle sample"
            ));
        }
        let all = read_marks();
        let hidden_before_sample = all
            .iter()
            .any(|m| m.event == "hidden" && m.pid == app_pid && m.epoch_ms <= sampled_epoch);
        // 가려진 창은 WebKit 이 프레임 · GPU · WebContent 메모리를 줄여 그 회의 두 수치가 기준선이 아니다.
        let excluded = if frame.hidden == Some(true) {
            Some("window hidden before its first frame".to_owned())
        } else if hidden_before_sample {
            Some("window hidden before the idle sample".to_owned())
        } else if locked {
            Some("screen locked during the run".to_owned())
        } else {
            None
        };
        let since = |m: Option<&Mark>| m.map(|m| m.epoch_ms - t0);
        Ok(Run {
            index,
            warmup,
            excluded,
            cold_start_ms,
            launcher_main_ms: since(marks::first(&all, "main", Some(launcher))),
            app_main_ms: since(marks::first(&all, "main", Some(app_pid))),
            launcher_pid: launcher,
            app_pid,
            group,
            detached: app_pid != launcher,
            memory,
            leftovers: Vec::new(),
            killed: false,
        })
    }

    /// 책임 프로세스가 `group` 인 프로세스(앱 + 그 앱이 요청한 WebKit 등)의 메모리.
    fn sample(group: u32) -> Memory {
        let mut processes: Vec<Process> = members(group)
            .into_iter()
            .filter_map(|pid| {
                let usage = macos::usage(pid)?;
                let name = macos::exe_path(pid)
                    .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
                    .unwrap_or_else(|| "?".into());
                Some(Process {
                    pid,
                    name,
                    footprint_bytes: usage.footprint,
                    rss_bytes: usage.resident,
                })
            })
            .collect();
        processes.sort_by_key(|p| std::cmp::Reverse(p.footprint_bytes));
        Memory {
            footprint_bytes: processes.iter().map(|p| p.footprint_bytes).sum(),
            rss_bytes: processes.iter().map(|p| p.rss_bytes).sum(),
            processes,
        }
    }

    /// 표시 파일에 `main` 을 남긴 프로세스 중 `launcher` 가 아닌 것(떼어 띄운 앱).
    fn marked_pids(all: &[Mark], launcher: u32) -> Vec<u32> {
        let mut pids: Vec<u32> = all
            .iter()
            .filter(|m| m.event == "main" && m.pid != launcher)
            .map(|m| m.pid)
            .collect();
        pids.dedup();
        pids
    }

    /// 책임 프로세스가 `group` 인 살아 있는 프로세스(그 pid 자신 포함).
    fn members(group: u32) -> Vec<u32> {
        macos::all_pids()
            .into_iter()
            .filter(|&pid| macos::responsible_pid(pid) == Some(group))
            .collect()
    }

    fn members_of(groups: &[u32]) -> Vec<u32> {
        let mut pids: Vec<u32> = groups.iter().flat_map(|&g| members(g)).collect();
        pids.sort_unstable();
        pids.dedup();
        pids
    }

    /// 앱을 pid 로 끈다: AppKit 의 종료 요청(앱이 정상 종료하며 단일 인스턴스 소켓을 지운다) → SIGTERM → SIGKILL. 끝난 뒤
    /// 남은 프로세스를 돌려준다. 실패한 회 · 신호로 멈춘 회에서도 부른다 — 이 측정이 띄운 것은 남기지 않고, 그 밖의 것
    /// (사람이 띄운 Pitwall 등)은 건드리지 않는다.
    fn shut_down(opts: &Options, launcher: u32, groups: &[u32]) -> (bool, Vec<u32>) {
        let deadline = |secs| Instant::now() + Duration::from_secs(secs);
        let gone = |pids: &[u32]| pids.iter().all(|&p| !macos::alive(p));
        // 끝난 launcher 를 거둬 좀비로 목록에 남지 않게 한다.
        let _ = macos::try_wait(launcher);
        let ours = members_of(groups);
        let apps: Vec<u32> = ours
            .iter()
            .copied()
            .filter(|&p| macos::exe_path(p).as_deref() == Some(opts.binary.as_path()))
            .collect();
        for &app in &apps {
            request_quit(app);
        }
        let wait = |pids: &[u32], until: Instant| {
            while !gone(pids) && Instant::now() < until {
                std::thread::sleep(Duration::from_millis(50));
            }
        };
        wait(&apps, deadline(10));
        let mut killed = false;
        for (sig, secs) in [(libc::SIGTERM, 5), (libc::SIGKILL, 5)] {
            let alive: Vec<u32> = apps.iter().copied().filter(|&p| macos::alive(p)).collect();
            if alive.is_empty() {
                break;
            }
            killed |= sig == libc::SIGKILL;
            for &p in &alive {
                macos::signal(p, sig);
            }
            wait(&alive, deadline(secs));
        }
        // 띄운 프로세스가 아직 안 끝났으면 거둔다(좀비를 남기지 않는다).
        if macos::try_wait(launcher).ok().flatten().is_none() {
            macos::signal(launcher, libc::SIGKILL);
            let until = deadline(5);
            while macos::try_wait(launcher).ok().flatten().is_none() && Instant::now() < until {
                std::thread::sleep(Duration::from_millis(20));
            }
        }
        // WebKit 프로세스는 앱과의 연결이 끊기면 스스로 끝난다.
        let until = deadline(10);
        while !members_of(groups).is_empty() && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(100));
        }
        let leftovers = members_of(groups);
        (killed, leftovers)
    }

    /// 그 pid 의 앱에 AppKit 종료(`NSRunningApplication.terminate`)를 보낸다. 이름이 아니라 pid 로 고른다 — 같은 이름의
    /// 다른 Pitwall 이 떠 있어도 이 앱만 받는다(AGENTS.md 「데스크톱 앱은 이름이 아니라 pid 로 고르고 끈다」).
    fn request_quit(pid: u32) {
        let script = format!(
            "ObjC.import('AppKit'); const a = $.NSRunningApplication.runningApplicationWithProcessIdentifier({pid}); if (a) a.terminate;"
        );
        let _ = std::process::Command::new("osascript")
            .args([OsStr::new("-l"), OsStr::new("JavaScript"), OsStr::new("-e")])
            .arg(script)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
}
