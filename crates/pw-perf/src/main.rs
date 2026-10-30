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

    use pw_core::config::AppPaths;
    use pw_perf::git::{
        self, Acted, Action, GitReport, GitRun, GitSettings, Progress, RepoResult, RunFailure,
    };
    use pw_perf::macos;
    use pw_perf::marks::{self, Mark};
    use pw_perf::report::{
        Build, Commit, DEFINITIONS, Hardware, Memory, Os, Process, Report, Run, Settings, markdown,
        summarize_runs, targets, too_few_valid, valid_runs,
    };

    const USAGE: &str = "usage: pw-perf --app <Pitwall.app | binary> [--runs N] [--warmup N] [--idle-secs S] [--timeout-secs S] [--out DIR] [--project DIR]\n       pw-perf --app <Pitwall.app | binary> --git-repo <synth repo> [--git-repo <synth repo> ...] [--checkout BRANCH] [--scenario-timeout-secs S] [--runs N] [--warmup N] [--out DIR]";

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
        /// 임시 프로젝트 대신 등록할 폴더(기준선을 git 저장소 프로젝트로 잰다 — M2-22).
        project: Option<PathBuf>,
        /// 있으면 git 시나리오를 잰다(저장소마다 준비 + 측정 회).
        git_repos: Vec<PathBuf>,
        checkout: String,
        scenario_timeout: Duration,
    }

    fn parse(args: Vec<String>) -> Result<Options, String> {
        let mut app = None;
        let mut runs = 5;
        let mut warmup = 1;
        let mut idle = 10;
        let mut timeout = 60;
        let mut out = None;
        let mut project = None;
        let mut git_repos = Vec::new();
        let mut checkout = git::DEFAULT_CHECKOUT.to_owned();
        let mut scenario_timeout = 420;
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
                "--project" => project = Some(PathBuf::from(value()?)),
                "--git-repo" => git_repos.push(PathBuf::from(value()?)),
                "--checkout" => checkout = value()?,
                "--scenario-timeout-secs" => scenario_timeout = number(value()?)?,
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
        if project.is_some() && !git_repos.is_empty() {
            return Err(format!("--project and --git-repo do not mix\n{USAGE}"));
        }
        let resolve = |dir: PathBuf| {
            std::fs::canonicalize(&dir).map_err(|e| format!("{}: {e}", dir.display()))
        };
        let project = project.map(resolve).transpose()?;
        let git_repos = git_repos
            .into_iter()
            .map(resolve)
            .collect::<Result<Vec<_>, _>>()?;
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
            project,
            git_repos,
            checkout,
            scenario_timeout: Duration::from_secs(scenario_timeout),
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

    /// 측정하는 동안 잠자기 · 디스플레이 잠들기를 막는 `caffeinate`. `-w` 로 이 하네스가 어떻게 끝나든 함께 끝나고, 정상으로
    /// 끝날 때는 띄운 그 pid 만 끈다. 띄우지 못하면 알리고 그대로 잰다.
    struct Awake(Option<std::process::Child>);

    impl Awake {
        fn start() -> Self {
            let child = std::process::Command::new("/usr/bin/caffeinate")
                .args(["-d", "-i", "-m", "-w", &std::process::id().to_string()])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn();
            match child {
                Ok(child) => {
                    say(format!(
                        "pw-perf: caffeinate {} keeps the display and the system awake while measuring",
                        child.id()
                    ));
                    Self(Some(child))
                }
                Err(e) => {
                    say(format!(
                        "pw-perf: warning: cannot start caffeinate ({e}) — the display may sleep and lock during the runs"
                    ));
                    Self(None)
                }
            }
        }
    }

    impl Drop for Awake {
        fn drop(&mut self) {
            if let Some(child) = &mut self.0 {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }

    /// 결과 파일을 썼으면 `Ok`. 값은 유효한 회가 충분한가(아니면 종료 코드 3).
    pub fn run(args: Vec<String>) -> Result<bool, String> {
        let opts = parse(args)?;
        macos::check_responsibility()?;
        let _awake = Awake::start();
        std::fs::create_dir_all(&opts.out).map_err(|e| format!("{}: {e}", opts.out.display()))?;
        let out = std::fs::canonicalize(&opts.out).map_err(|e| e.to_string())?;
        let scratch = Scratch(std::env::temp_dir().join(format!("pw-perf-{}", std::process::id())));
        if !opts.git_repos.is_empty() {
            return run_git(&opts, &scratch.0, &out);
        }
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
            let warmup = index < opts.warmup;
            let run = run_once(
                &opts,
                &scratch.0,
                &out,
                store_root.as_deref(),
                index,
                opts.project.as_deref(),
                None,
                |launch, dir| {
                    let started = first_frame(&opts, launch, dir, index)?;
                    observe_idle(&opts, started, launch, dir, index, warmup)
                },
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
            build: build(&opts),
            commit: commit(),
            os: os(),
            hardware: hardware(),
            settings: Settings {
                runs: opts.runs,
                warmup: opts.warmup,
                idle_secs: opts.idle.as_secs(),
                project: opts.project.as_ref().map(|p| p.display().to_string()),
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

    fn os() -> Os {
        Os {
            name: "macOS".into(),
            version: macos::sysctl_string("kern.osproductversion"),
            build: macos::sysctl_string("kern.osversion"),
        }
    }

    fn hardware() -> Hardware {
        Hardware {
            model: macos::sysctl_string("hw.model"),
            cpu: macos::sysctl_string("machdep.cpu.brand_string"),
            cpus: macos::sysctl_u64("hw.ncpu").map(|n| n as u32),
            memory_bytes: macos::sysctl_u64("hw.memsize"),
        }
    }

    fn build(opts: &Options) -> Build {
        Build {
            profile: profile_of(&opts.binary).into(),
            app: opts.app.display().to_string(),
            binary: opts.binary.display().to_string(),
        }
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
        /// `project` 가 있으면 그 폴더를 등록한다(지우지 않는다). 없으면 회 폴더 안에 빈 프로젝트를 만든다.
        fn create(scratch: &Path, index: usize, project: Option<&Path>) -> Result<Self, String> {
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
            std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
            let project = match project {
                Some(project) => project.to_path_buf(),
                None => {
                    let project = root.join("project");
                    std::fs::create_dir_all(&project).map_err(|e| e.to_string())?;
                    std::fs::write(project.join("README.md"), "perf baseline project\n")
                        .map_err(|e| e.to_string())?;
                    project
                }
            };
            pw_perf::register_untrusted(&paths, &project)?;
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

    /// 한 회가 끝나지 못한 까닭. `fatal` 이면 다음 회로 가지 않는다 — 중단 신호, 회 폴더 · 띄우기 실패, 끈 뒤 남은 프로세스 ·
    /// 소켓(다음 회와 섞일 수 있다).
    struct RunError {
        message: String,
        fatal: bool,
    }

    impl RunError {
        fn run(message: String) -> Self {
            Self {
                message,
                fatal: interrupted().is_some(),
            }
        }
        fn fatal(message: String) -> Self {
            Self {
                message,
                fatal: true,
            }
        }
    }

    impl From<RunError> for String {
        fn from(e: RunError) -> Self {
            e.message
        }
    }

    /// 한 회에 잰 것. 끈 뒤 [`run_once`] 가 묶음 키로 남은 프로세스를 보고 SIGKILL 여부를 적는다.
    trait Observed {
        fn group(&self) -> u32;
        fn killed(self, killed: bool) -> Self;
    }

    impl Observed for Run {
        fn group(&self) -> u32 {
            self.group
        }
        fn killed(self, killed: bool) -> Self {
            Run {
                killed,
                leftovers: Vec::new(),
                ..self
            }
        }
    }

    impl Observed for GitRun {
        fn group(&self) -> u32 {
            self.group
        }
        fn killed(self, killed: bool) -> Self {
            GitRun { killed, ..self }
        }
    }

    /// 띄운 프로세스와 그 시각.
    struct Launch {
        pid: u32,
        started: Instant,
        /// 띄우기 직전의 벽시계.
        t0: f64,
    }

    /// 앱 하나: 임시 `PITWALL_*_DIR` 에 프로젝트를 등록하고 CLI 로 띄워 `observe` 로 잰 뒤 pid 로 끈다. `scenario` 가 있으면
    /// 앱에 그 시나리오를 돌리게 한다. 실패 · 신호 · 패닉 어느 쪽으로 끝나도 띄운 앱을 끄고 임시 폴더를 지운다.
    #[allow(clippy::too_many_arguments)] // 회 하나의 설정 — 묶을 구조체를 따로 두면 부르는 두 곳이 모두 그것을 채워야 한다.
    fn run_once<T: Observed>(
        opts: &Options,
        scratch: &Path,
        out: &Path,
        store_root: Option<&Path>,
        index: usize,
        project: Option<&Path>,
        scenario: Option<&str>,
        observe: impl FnOnce(&Launch, &RunDir) -> Result<T, String>,
    ) -> Result<T, RunError> {
        preflight().map_err(RunError::run)?;
        let dir = RunDir::create(scratch, index, project).map_err(RunError::fatal)?;

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
        if let Some(scenario) = scenario {
            env.push((pw_perf::SCENARIO_ENV.into(), scenario.into()));
        }

        let started = Instant::now();
        let t0 = epoch_ms();
        let launcher =
            macos::spawn_disclaimed(&opts.binary, &[dir.project.as_os_str()], &dir.root, &env)
                .map_err(|e| {
                    RunError::fatal(format!("cannot launch {}: {e}", opts.binary.display()))
                })?;

        let launch = Launch {
            pid: launcher,
            started,
            t0,
        };
        let observed = catch_unwind(AssertUnwindSafe(|| observe(&launch, &dir)));
        // 이 회의 것: 띄운 pid, 표시 파일에 `main` 을 남긴 pid(떼어 띄운 앱), 그 앱의 묶음 키. 표시 파일 경로는 이 측정이
        // 준 환경을 받은 프로세스만 안다.
        let marked = marks::parse(&std::fs::read_to_string(&dir.marks).unwrap_or_default());
        let mut groups = vec![launcher];
        groups.extend(marked_pids(&marked, launcher));
        if let Ok(Ok(run)) = &observed {
            groups.push(run.group());
        }
        let (killed, leftovers) = shut_down(opts, launcher, &groups);
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
            Ok(Err(e)) => {
                let message = format!("{e}{}", leftover_note(&leftovers, &sockets));
                return Err(if leftovers.is_empty() && sockets.is_empty() {
                    RunError::run(message)
                } else {
                    RunError::fatal(message)
                });
            }
            Ok(Ok(run)) => run,
        };
        if !leftovers.is_empty() || !sockets.is_empty() {
            return Err(RunError::fatal(format!(
                "run {index} did not clean up after quitting{}",
                leftover_note(&leftovers, &sockets)
            )));
        }
        Ok(run.killed(killed))
    }

    /// 첫 프레임을 낸 앱.
    struct Started {
        frame: Mark,
        app_pid: u32,
        /// 메모리를 합칠 묶음 키(책임 프로세스).
        group: u32,
        cold_start_ms: f64,
    }

    /// 첫 프레임 표시를 기다리고, 그 표시를 낸 앱이 이 측정이 띄운 것인지 본다.
    fn first_frame(
        opts: &Options,
        launch: &Launch,
        dir: &RunDir,
        index: usize,
    ) -> Result<Started, String> {
        let (launcher, started, t0) = (launch.pid, launch.started, launch.t0);
        let read_marks = || marks::parse(&std::fs::read_to_string(&dir.marks).unwrap_or_default());
        let mut launcher_exit = None;
        let mut locked = false;
        let mut next_lock_check = Instant::now();
        let frame: Mark = loop {
            check_interrupted()?;
            if Instant::now() >= next_lock_check {
                locked |= screen_locked()?;
                next_lock_check = Instant::now() + LOCK_CHECK;
            }
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
                    "run {index}: no first-frame mark within {}s ({})",
                    opts.timeout.as_secs(),
                    marks::stall_note(&all, epoch_ms(), locked)
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
        Ok(Started {
            frame,
            app_pid,
            group,
            cold_start_ms,
        })
    }

    /// 첫 프레임 뒤 `--idle-secs` 만큼 기다려 유휴 메모리를 잰다(콜드 스타트 · 유휴 메모리 기준선).
    fn observe_idle(
        opts: &Options,
        started: Started,
        launch: &Launch,
        dir: &RunDir,
        index: usize,
        warmup: bool,
    ) -> Result<Run, String> {
        let Started {
            frame,
            app_pid,
            group,
            cold_start_ms,
        } = started;
        let (launcher, t0) = (launch.pid, launch.t0);
        let read_marks = || marks::parse(&std::fs::read_to_string(&dir.marks).unwrap_or_default());

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

    /// git 시나리오: 저장소마다 준비 + 측정 회를 돌려 `result.json` · `summary.md` 를 쓴다. 회마다 저장소를 `main` 으로 돌려놓고
    /// 띄우며, 앱이 기다린다고 알리는 일(`git checkout <--checkout>`, 픽스처 파일 1만 개 고치기, `git add <파일>`)을 그 저장소에서 한다.
    /// 한 회가 멈추거나(시나리오 시간 제한) 실패하면 그 회를 실패로 적고 다음 회로 간다 — 앱은 그 회의 pid 로만 끈다. 회가 끝날
    /// 때마다 결과 파일을 다시 써서, 중단 신호나 되돌릴 수 없는 실패로 멈춰도 그때까지의 요약이 남는다.
    fn run_git(opts: &Options, scratch: &Path, out: &Path) -> Result<bool, String> {
        let mut expected = Vec::new();
        for repo in &opts.git_repos {
            check_fixture(repo, &opts.checkout)?;
            let commits: u64 = git_in(repo, &["rev-list", "--count", "--all"])?
                .trim()
                .parse()
                .map_err(|e| format!("{}: cannot count the commits: {e}", repo.display()))?;
            expected.push(commits);
        }
        let total = opts.warmup + opts.runs;
        say(format!(
            "pw-perf: git scenario on {} repositories × ({} warm-up + {} measured); each run opens one project window → {}",
            opts.git_repos.len(),
            opts.warmup,
            opts.runs,
            out.display()
        ));
        let store_root = webkit_store_root(&opts.app);
        let mut repos: Vec<RepoResult> = Vec::new();
        let mut stopped = None;
        'repos: for (r, repo) in opts.git_repos.iter().enumerate() {
            let repo_out = out.join(format!("repo-{r}"));
            let graph_before = has_commit_graph(repo);
            repos.push(repo_result(
                repo,
                graph_before,
                expected[r],
                Vec::new(),
                Vec::new(),
            ));
            let mut runs = Vec::new();
            let mut failures = Vec::new();
            for index in 0..total {
                if let Err(e) = check_interrupted() {
                    stopped = Some(e);
                    break 'repos;
                }
                let warmup = index < opts.warmup;
                if let Err(e) = git_in(repo, &["checkout", "-q", "main"]) {
                    stopped = Some(e);
                    break 'repos;
                }
                let run = run_once(
                    opts,
                    scratch,
                    &repo_out,
                    store_root.as_deref(),
                    index,
                    Some(repo),
                    Some("git"),
                    |launch, dir| {
                        let started = first_frame(opts, launch, dir, index)?;
                        observe_git(opts, started, dir, index, warmup, repo, expected[r])
                    },
                );
                // 실패한 회도 저장소를 돌려놓는다 — 다음 회 · 다음 측정이 고치지 않은 `main` 에서 시작한다.
                let back = git_in(repo, &["checkout", "-q", "HEAD", "--", git::CHANGES_DIR])
                    .and_then(|_| git_in(repo, &["checkout", "-q", "main"]));
                match run {
                    Ok(run) => {
                        let m = &run.measure;
                        say(format!(
                            "pw-perf: {} run {index}{}{}: open {:.0} ms, fling ≤16.7 ms {:.2}, diff {:.0} ms, checkout → status bar {:.0} ms, gutter {:.0} ms, typing block max {:.0} ms, commit fling ≤16.7 ms {:.2}, git add → staged {:.0} ms",
                            repo.display(),
                            if run.warmup { " (warm-up)" } else { "" },
                            run.excluded
                                .as_deref()
                                .map_or(String::new(), |why| format!(" (excluded: {why})")),
                            m.open_ms,
                            m.fling.interval.within16_7,
                            m.diff_ms,
                            m.branch_ms,
                            m.gutter_ms,
                            m.typing.blocks.max_ms,
                            m.commit_fling.interval.within16_7,
                            m.stage_ms
                        ));
                        runs.push(run);
                    }
                    Err(e) => {
                        say(format!(
                            "pw-perf: {} run {index}{} failed: {}",
                            repo.display(),
                            if warmup { " (warm-up)" } else { "" },
                            e.message
                        ));
                        failures.push(RunFailure {
                            index,
                            warmup,
                            reason: e.message.clone(),
                        });
                        if e.fatal {
                            stopped = Some(e.message);
                        }
                    }
                }
                if stopped.is_none()
                    && let Err(e) = back
                {
                    stopped = Some(format!("cannot restore the repository: {e}"));
                }
                *repos.last_mut().expect("pushed above") = repo_result(
                    repo,
                    graph_before,
                    expected[r],
                    runs.clone(),
                    failures.clone(),
                );
                write_git_report(opts, out, &repos, stopped.as_deref(), false)?;
                if stopped.is_some() {
                    break 'repos;
                }
                std::thread::sleep(Duration::from_secs(1));
            }
            let graph_after = has_commit_graph(repo);
            if graph_before != graph_after {
                stopped = Some(format!(
                    "{}: commit-graph went from {graph_before} to {graph_after} during the runs — something wrote to the repository; recreate it",
                    repo.display()
                ));
                break;
            }
        }
        let report = write_git_report(opts, out, &repos, stopped.as_deref(), true)?;
        if let Some(stopped) = stopped {
            return Err(format!(
                "{stopped} (the runs before it are in {})",
                out.join("summary.md").display()
            ));
        }
        Ok(report.warning.is_none())
    }

    fn repo_result(
        repo: &Path,
        commit_graph: bool,
        commits: u64,
        runs: Vec<GitRun>,
        failures: Vec<RunFailure>,
    ) -> RepoResult {
        let summary = git::summarize_git(&runs);
        RepoResult {
            path: repo.display().to_string(),
            commit_graph,
            commits: Some(commits),
            targets: git::git_targets(&summary),
            summary,
            valid_runs: runs
                .iter()
                .filter(|r| !r.warmup && r.excluded.is_none())
                .count(),
            runs,
            failures,
        }
    }

    /// 지금까지의 회로 `result.json` · `summary.md` 를 (다시) 쓴다. `last` 면 요약을 화면에도 낸다.
    fn write_git_report(
        opts: &Options,
        out: &Path,
        repos: &[RepoResult],
        stopped: Option<&str>,
        last: bool,
    ) -> Result<GitReport, String> {
        let warning = repos.iter().find_map(|r| {
            too_few_valid(r.valid_runs, opts.runs).map(|w| format!("{}: {w}", r.path))
        });
        let report = GitReport {
            schema: 1,
            scenario: "git",
            definitions: git::DEFINITIONS,
            build: build(opts),
            commit: commit(),
            os: os(),
            hardware: hardware(),
            settings: GitSettings {
                runs: opts.runs,
                warmup: opts.warmup,
                checkout: opts.checkout.clone(),
            },
            repos: repos.to_vec(),
            warning,
            stopped: stopped.map(str::to_owned),
        };
        let json = serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?;
        std::fs::write(out.join("result.json"), json + "\n").map_err(|e| e.to_string())?;
        let md = git::git_report_markdown(&report);
        std::fs::write(out.join("summary.md"), &md).map_err(|e| e.to_string())?;
        if last {
            let _ = writeln!(std::io::stdout(), "{md}");
            say(format!(
                "pw-perf: wrote {} and {}",
                out.join("result.json").display(),
                out.join("summary.md").display()
            ));
            if let Some(warning) = &report.warning {
                say(format!("pw-perf: warning: {warning}"));
            }
        }
        Ok(report)
    }

    /// 시나리오를 따라가며 표시 파일을 다시 읽는 간격. 측정하는 쪽의 반응 지연은 수치에 들지 않는다(시작점은 git 을 띄우기 직전의
    /// 벽시계) — 프레임을 재는 동안 같은 기기에서 도는 일을 줄인다.
    const POLL: Duration = Duration::from_millis(50);
    /// 화면 잠금을 보는 간격(`ioreg` 를 띄운다).
    const LOCK_CHECK: Duration = Duration::from_secs(2);

    /// 앱의 시나리오를 따라간다: 앱이 기다리는 일(체크아웃 · 파일 고치기 · `git add`)을 저장소에서 하고, 끝 표시가 오면 메모리를 잰다.
    fn observe_git(
        opts: &Options,
        started: Started,
        dir: &RunDir,
        index: usize,
        warmup: bool,
        repo: &Path,
        commits: u64,
    ) -> Result<GitRun, String> {
        let Started {
            frame,
            app_pid,
            group,
            cold_start_ms,
        } = started;
        let deadline = Instant::now() + opts.scenario_timeout;
        let mut acted = Acted::default();
        let mut locked = false;
        let mut next_lock_check = Instant::now();
        let measure = loop {
            check_interrupted()?;
            if Instant::now() >= next_lock_check {
                locked |= screen_locked()?;
                next_lock_check = Instant::now() + LOCK_CHECK;
            }
            let all = marks::parse(&std::fs::read_to_string(&dir.marks).unwrap_or_default());
            match git::progress(&all, app_pid, acted) {
                Progress::Waiting => {}
                Progress::Act(Action::Checkout) => {
                    // 「터미널에서 git checkout」 의 시작점: 띄우기 직전의 벽시계.
                    acted.checked_out_at = Some(epoch_ms());
                    git_in(repo, &["checkout", "-q", &opts.checkout])?;
                    continue;
                }
                Progress::Act(Action::Change) => {
                    let changed = git::write_changes(repo).map_err(|e| {
                        format!(
                            "run {index}: cannot change the files in {}: {e}",
                            git::CHANGES_DIR
                        )
                    })?;
                    if changed == 0 {
                        return Err(format!(
                            "run {index}: {} has no files to change",
                            repo.join(git::CHANGES_DIR).display()
                        ));
                    }
                    acted.changed = true;
                    continue;
                }
                Progress::Act(Action::Stage(path)) => {
                    // 「터미널에서 git add」 의 시작점: 띄우기 직전의 벽시계.
                    acted.staged_at = Some(epoch_ms());
                    git_in(repo, &["add", "--", &path])?;
                    continue;
                }
                Progress::Done(m) => {
                    git::check_rows(&m, commits).map_err(|e| format!("run {index}: {e}"))?;
                    break *m;
                }
                Progress::Failed(e) => return Err(format!("run {index}: {e}")),
            }
            if !macos::alive(app_pid) {
                return Err(format!(
                    "run {index}: the app {app_pid} exited during the scenario"
                ));
            }
            if Instant::now() > deadline {
                return Err(format!(
                    "run {index}: the scenario did not finish within {}s ({})",
                    opts.scenario_timeout.as_secs(),
                    marks::stall_note(&all, epoch_ms(), locked)
                ));
            }
            std::thread::sleep(POLL);
        };
        locked |= screen_locked()?;
        let ended = epoch_ms();
        let memory = sample(group);
        let all = marks::parse(&std::fs::read_to_string(&dir.marks).unwrap_or_default());
        let hidden = all
            .iter()
            .any(|m| m.event == "hidden" && m.pid == app_pid && m.epoch_ms <= ended);
        // 가려진 창은 WebKit 이 rAF 를 미뤄 프레임 · 첫 그림 수치가 실제 화면의 것이 아니다.
        let excluded = if frame.hidden == Some(true) {
            Some("window hidden before its first frame".to_owned())
        } else if hidden {
            Some("window hidden during the scenario".to_owned())
        } else if locked {
            Some("screen locked during the run".to_owned())
        } else {
            None
        };
        Ok(GitRun {
            index,
            warmup,
            excluded,
            app_pid,
            group,
            cold_start_ms,
            measure,
            memory,
            killed: false,
        })
    }

    /// 저장소에서 git 을 돌린다(사람의 전역 · 시스템 설정과 훅은 읽지 않는다). 실패하면 stderr 를 붙여 돌려준다.
    fn git_in(repo: &Path, args: &[&str]) -> Result<String, String> {
        let mut command = std::process::Command::new("git");
        // 물려받은 `GIT_DIR` · `GIT_WORK_TREE` · `GIT_INDEX_FILE` 류(git 훅 안에서 돌 때)가 다른 저장소를 가리키지 않게 지우고,
        // 저장소가 아닌 폴더를 받았을 때 위의 저장소(이 워크트리 등)를 집지 않게 위로 찾기를 저장소의 부모에서 멈춘다.
        for (key, _) in std::env::vars_os() {
            if key.to_string_lossy().starts_with("GIT_") {
                command.env_remove(key);
            }
        }
        if let Some(parent) = std::fs::canonicalize(repo)
            .ok()
            .and_then(|r| r.parent().map(Path::to_path_buf))
        {
            command.env("GIT_CEILING_DIRECTORIES", parent);
        }
        let out = command
            .arg("-C")
            .arg(repo)
            // 커밋 뒤 자동 유지보수(git 2.54 의 `maintenance run --auto --detach`)가 「commit-graph 없음」 저장소에 그래프를 쓰지 않게.
            .args([
                "-c",
                "core.hooksPath=/dev/null",
                "-c",
                "maintenance.auto=false",
                "-c",
                "gc.auto=0",
            ])
            .args(args)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_TERMINAL_PROMPT", "0")
            .output()
            .map_err(|e| format!("cannot run git: {e}"))?;
        if out.status.success() {
            Ok(String::from_utf8_lossy(&out.stdout).into_owned())
        } else {
            Err(format!(
                "git {args:?} in {} failed: {}",
                repo.display(),
                String::from_utf8_lossy(&out.stderr).trim()
            ))
        }
    }

    /// 픽스처(`synth_history --perf-fixture`)가 든 저장소인가: `main` 과 체크아웃 대상 브랜치, 고칠 파일들과 타이핑 대상 파일이 있고
    /// 작업 트리가 깨끗하다.
    fn check_fixture(repo: &Path, checkout: &str) -> Result<(), String> {
        let hint = "make it with `mise run perf-git-repos` (synth_history --perf-fixture)";
        for branch in ["main", checkout] {
            git_in(
                repo,
                &[
                    "rev-parse",
                    "--verify",
                    "-q",
                    &format!("refs/heads/{branch}"),
                ],
            )
            .map_err(|_| format!("{} has no branch `{branch}`; {hint}", repo.display()))?;
        }
        for path in [git::CHANGES_DIR, git::TYPING_PATH] {
            git_in(repo, &["cat-file", "-e", &format!("main:{path}")]).map_err(|_| {
                format!(
                    "{} has no `{path}` on main (an older fixture); remove it and {hint}",
                    repo.display()
                )
            })?;
        }
        let status = git_in(repo, &["status", "--porcelain"])?;
        if !status.is_empty() {
            return Err(format!(
                "{} has uncommitted changes; the scenario checks out branches in it",
                repo.display()
            ));
        }
        Ok(())
    }

    fn has_commit_graph(repo: &Path) -> bool {
        let info = repo.join(".git/objects/info");
        info.join("commit-graph").is_file() || info.join("commit-graphs").is_dir()
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
