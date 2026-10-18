//! `pw-perf --app <Pitwall.app>`: 콜드 스타트 · 유휴 메모리를 여러 번 재 `result.json` · `summary.md` 를 쓴다. 무엇을
//! 어떻게 재는지는 CONTRIBUTING.md 「성능 기준선」 절. macOS 만 잰다.

use std::process::ExitCode;

#[cfg(not(target_os = "macos"))]
fn main() -> ExitCode {
    eprintln!("pw-perf: only macOS is measured");
    ExitCode::from(2)
}

#[cfg(target_os = "macos")]
fn main() -> ExitCode {
    match measure::run(std::env::args().skip(1).collect()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("pw-perf: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(target_os = "macos")]
mod measure {
    use std::ffi::{OsStr, OsString};
    use std::path::{Path, PathBuf};
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    use pw_core::EventBus;
    use pw_core::config::AppPaths;
    use pw_core::registry::{NewProject, Registry};
    use pw_perf::macos;
    use pw_perf::marks::{self, Mark};
    use pw_perf::report::{
        Build, Commit, DEFINITIONS, Hardware, Memory, Os, Process, Report, Run, Settings, markdown,
        summarize_runs, targets,
    };

    const USAGE: &str = "usage: pw-perf --app <Pitwall.app | binary> [--runs N] [--warmup N] [--idle-secs S] [--timeout-secs S] [--out DIR]";

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

    pub fn run(args: Vec<String>) -> Result<(), String> {
        let opts = parse(args)?;
        std::fs::create_dir_all(&opts.out).map_err(|e| format!("{}: {e}", opts.out.display()))?;
        let out = std::fs::canonicalize(&opts.out).map_err(|e| e.to_string())?;
        let total = opts.warmup + opts.runs;
        eprintln!(
            "pw-perf: {} ({} warm-up + {} measured; each opens one project window) → {}",
            opts.binary.display(),
            opts.warmup,
            opts.runs,
            out.display()
        );
        let mut runs = Vec::new();
        for index in 0..total {
            let run = run_once(&opts, &out, index, index < opts.warmup)?;
            eprintln!(
                "pw-perf: run {index}{}{}: cold start {:.0} ms, idle footprint {:.1} MiB ({} processes)",
                if run.warmup { " (warm-up)" } else { "" },
                run.excluded
                    .as_deref()
                    .map_or(String::new(), |why| format!(" (excluded: {why})")),
                run.cold_start_ms,
                run.memory.footprint_bytes as f64 / 1048576.0,
                run.memory.processes.len()
            );
            runs.push(run);
            // 앞 회의 프로세스 정리가 다음 회의 시작과 겹치지 않게 잠깐 쉰다.
            std::thread::sleep(Duration::from_secs(1));
        }
        let summary = summarize_runs(&runs);
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
            summary,
            runs,
        };
        let json = serde_json::to_string_pretty(&report).map_err(|e| e.to_string())?;
        std::fs::write(out.join("result.json"), json + "\n").map_err(|e| e.to_string())?;
        let md = markdown(&report);
        std::fs::write(out.join("summary.md"), &md).map_err(|e| e.to_string())?;
        println!("{md}");
        eprintln!(
            "pw-perf: wrote {} and {}",
            out.join("result.json").display(),
            out.join("summary.md").display()
        );
        Ok(())
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

    /// 이 실행 파일로 떠 있는 프로세스.
    fn instances_of(binary: &Path) -> Vec<u32> {
        macos::all_pids()
            .into_iter()
            .filter(|&pid| macos::exe_path(pid).is_some_and(|p| p == binary))
            .collect()
    }

    /// release 빌드는 단일 인스턴스 키가 하나라(apps/desktop/src-tauri/src/instance.rs) 어디서 띄운 release Pitwall 이든
    /// 이 측정의 인자를 받아 갈 수 있다 — 그러면 띄운 프로세스가 곧바로 끝나 엉뚱한 수치가 나온다. debug 빌드는 키가
    /// 빌드 폴더마다 달라 겹치지 않는다.
    fn other_release_instances() -> Vec<(u32, PathBuf)> {
        macos::all_pids()
            .into_iter()
            .filter_map(|pid| macos::exe_path(pid).map(|p| (pid, p)))
            .filter(|(_, p)| {
                p.file_name().is_some_and(|n| n == "pitwall")
                    && !p.to_string_lossy().contains("/target/debug/")
            })
            .collect()
    }

    fn preflight(opts: &Options) -> Result<(), String> {
        if screen_locked()? {
            return Err("the screen is locked — WebKit defers frames while locked, so this would not be a baseline. Unlock and run again".into());
        }
        let others = other_release_instances();
        if !others.is_empty() {
            let list: Vec<String> = others
                .iter()
                .map(|(pid, p)| format!("{pid} {}", p.display()))
                .collect();
            return Err(format!(
                "a non-debug Pitwall is running and would take the launch's arguments (single instance); quit it first:\n  {}",
                list.join("\n  ")
            ));
        }
        if !instances_of(&opts.binary).is_empty() {
            return Err(format!("{} is already running", opts.binary.display()));
        }
        Ok(())
    }

    /// 앱 하나: 임시 `PITWALL_*_DIR` 에 프로젝트를 등록하고 CLI 로 띄워 첫 프레임 · 유휴 메모리를 잰 뒤 pid 로 끈다.
    fn run_once(opts: &Options, out: &Path, index: usize, warmup: bool) -> Result<Run, String> {
        preflight(opts)?;
        let dir = out.join(format!("run-{index}"));
        let paths = AppPaths::under(&dir.join("app"));
        let project = dir.join("project");
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
        drop(registry);
        let marks_file = dir.join("marks.jsonl");

        let mut env: Vec<(OsString, OsString)> = std::env::vars_os()
            .filter(|(k, _)| {
                // 앱의 동작을 바꾸는 물려받은 변수를 지운다 — 떼어 띄우기 표식 · 개발 서버 · 시험 키 · 다른 폴더.
                !k.to_string_lossy().starts_with("PITWALL_")
            })
            .collect();
        for (key, value) in [
            ("PITWALL_CONFIG_DIR", &paths.config),
            ("PITWALL_DATA_DIR", &paths.data),
            ("PITWALL_STATE_DIR", &paths.state),
            ("PITWALL_CACHE_DIR", &paths.cache),
            (pw_perf::MARK_ENV, &marks_file),
        ] {
            env.push((key.into(), value.into()));
        }

        let started = Instant::now();
        let t0 = epoch_ms();
        let launcher = macos::spawn_disclaimed(&opts.binary, &[project.as_os_str()], &dir, &env)
            .map_err(|e| format!("cannot launch {}: {e}", opts.binary.display()))?;

        let result = observe(opts, launcher, started, t0, &marks_file, index, warmup);
        // 이 회의 것: 띄운 pid, 표시 파일에 `main` 을 남긴 pid(떼어 띄운 앱), 그 앱의 묶음 키. 표시 파일 경로는 이 측정이
        // 준 환경을 받은 프로세스만 안다.
        let marked = marks::parse(&std::fs::read_to_string(&marks_file).unwrap_or_default());
        let mut groups = vec![launcher];
        groups.extend(marked_pids(&marked, launcher));
        groups.extend(result.as_ref().ok().map(|r| r.group));
        let quit = shut_down(opts, launcher, &groups);
        let mut run = result?;
        let (killed, leftovers) = quit;
        run.killed = killed;
        run.leftovers = leftovers;
        if !run.leftovers.is_empty() {
            return Err(format!(
                "processes of run {index} are still alive after quitting: {:?}",
                run.leftovers
            ));
        }
        Ok(run)
    }

    fn observe(
        opts: &Options,
        launcher: u32,
        started: Instant,
        t0: f64,
        marks_file: &Path,
        index: usize,
        warmup: bool,
    ) -> Result<Run, String> {
        let mut launcher_exit = None;
        let frame: Mark = loop {
            if launcher_exit.is_none() {
                launcher_exit = macos::try_wait(launcher).map_err(|e| e.to_string())?;
            }
            let text = std::fs::read_to_string(marks_file).unwrap_or_default();
            let all = marks::parse(&text);
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
        // 묶음 키: 앱의 책임 프로세스. 끊어 띄운 launcher 의 자식은 자기 pid 를 책임 프로세스로 갖는다(launcher 일 수도
        // 있다). 다른 값(터미널 호스트 등)이면 그 아래의 다른 프로세스가 섞이므로 재지 않는다.
        let group = macos::responsible_pid(app_pid);
        if group != app_pid && group != launcher {
            return Err(format!(
                "run {index}: the app {app_pid} is responsible to {group}, not to itself or the launch {launcher} — its WebKit processes cannot be told apart"
            ));
        }
        let all = marks::parse(&std::fs::read_to_string(marks_file).unwrap_or_default());
        let since = |m: Option<&Mark>| m.map(|m| m.epoch_ms - t0);
        let cold_start_ms = frame.epoch_ms - t0;

        std::thread::sleep(opts.idle);
        let memory = sample(group);
        if !memory.processes.iter().any(|p| p.pid == app_pid) {
            return Err(format!(
                "run {index}: the app {app_pid} exited before the idle sample"
            ));
        }
        // 첫 프레임 전에 창이 가려졌거나 측정 중에 화면이 잠겼으면 WebKit 이 프레임 · GPU 메모리를 줄여 그 회는 기준선이 아니다.
        let excluded = if frame.hidden == Some(true) {
            Some("window hidden before its first frame".to_owned())
        } else if screen_locked()? {
            Some("screen locked during the run".to_owned())
        } else {
            None
        };
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
            .filter(|&pid| macos::responsible_pid(pid) == group)
            .collect()
    }

    fn members_of(groups: &[u32]) -> Vec<u32> {
        let mut pids: Vec<u32> = groups.iter().flat_map(|&g| members(g)).collect();
        pids.sort_unstable();
        pids.dedup();
        pids
    }

    /// 앱을 pid 로 끈다: AppKit 의 종료 요청(앱이 정상 종료하며 단일 인스턴스 소켓을 지운다) → SIGTERM → SIGKILL. 끝난 뒤
    /// 남은 프로세스를 돌려준다. 실패한 회에서도 부른다 — 이 측정이 띄운 것은 남기지 않고, 그 밖의 것(사람이 띄운
    /// Pitwall 등)은 건드리지 않는다.
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
