//! 결과: 기계가 읽는 JSON(`result.json`)과 사람이 읽는 요약(`summary.md`).

use serde::Serialize;

use crate::stats::{Summary, summarize};

/// 목표(docs/design/06-roadmap.md 「성능 목표 (검증 기준)」). 넘어도 측정은 실패하지 않는다 — 기록만 한다.
pub const TARGET_COLD_START_MS: f64 = 1500.0;
/// 06 의 「300MB」 는 300 × 10^6 바이트로 본다(MB 보다 엄격한 쪽). 요약도 MB(10^6) 로 적는다.
pub const TARGET_IDLE_FOOTPRINT_BYTES: u64 = 300 * MB;
const MB: u64 = 1_000_000;
const GIB: u64 = 1 << 30;

/// 무엇에서 무엇까지 쟀는가. 결과 파일만 받은 사람도 읽을 수 있게 결과에 함께 싣는다(정본은 CONTRIBUTING.md
/// 「성능 기준선」 절).
pub const DEFINITIONS: Definitions = Definitions {
    start: "wall clock just before the harness spawns `<bundle>/Contents/MacOS/pitwall <project>` (CLI cold start of a registered project)",
    end: "the project window's `perf_first_frame` mark: second requestAnimationFrame after the first React commit, timestamped by the app on receipt",
    idle: "memory sampled idle_secs after the first-frame mark; a run whose window was hidden at any time before the sample, or whose screen was locked, is excluded",
    memory: "sum of phys_footprint (Activity Monitor's Memory column) over the processes whose responsible pid is the app instance (the app and the WebKit WebContent/GPU/Networking it requested); the launch is spawned with responsibility disclaimed so that this group is the instance alone",
    cache: "warm disk cache, new processes: no purge between runs; warm-up runs and excluded runs (window hidden before its first frame, screen locked) are recorded but left out of the medians",
};

#[derive(Debug, Clone, Serialize)]
pub struct Definitions {
    pub start: &'static str,
    pub end: &'static str,
    pub idle: &'static str,
    pub memory: &'static str,
    pub cache: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub schema: u32,
    pub definitions: Definitions,
    pub build: Build,
    pub commit: Commit,
    pub os: Os,
    pub hardware: Hardware,
    pub settings: Settings,
    pub runs: Vec<Run>,
    /// 중앙값에 든 회 수(준비 회와 제외한 회를 뺀 것).
    pub valid_runs: usize,
    /// 유효한 회가 `--runs` 의 절반보다 적을 때의 경고([`too_few_valid`]). 있으면 pw-perf 는 3 으로 끝난다.
    pub warning: Option<String>,
    pub summary: Summaries,
    pub targets: Targets,
}

#[derive(Debug, Clone, Serialize)]
pub struct Build {
    /// `release` · `debug` · `unknown`. 바이너리가 놓인 `target/<프로필>/` 로 판정한다. 기준선은 release 번들만이다.
    pub profile: String,
    pub app: String,
    pub binary: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Commit {
    pub sha: Option<String>,
    /// 측정할 때 작업 트리에 커밋하지 않은 변경이 있었는가.
    pub dirty: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Os {
    pub name: String,
    pub version: Option<String>,
    pub build: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Hardware {
    pub model: Option<String>,
    pub cpu: Option<String>,
    pub cpus: Option<u32>,
    pub memory_bytes: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Settings {
    pub runs: usize,
    pub warmup: usize,
    pub idle_secs: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Run {
    pub index: usize,
    pub warmup: bool,
    /// 기준선에서 뺀 이유(창이 첫 프레임 전에 가려졌다, 화면이 잠겼다). `None` 이면 유효한 회다.
    pub excluded: Option<String>,
    /// 시작 → 첫 프레임.
    pub cold_start_ms: f64,
    /// 시작 → 띄운 프로세스(CLI)가 `main` 에 들어온 때.
    pub launcher_main_ms: Option<f64>,
    /// 시작 → 떼어 띄운 앱 프로세스가 `main` 에 들어온 때.
    pub app_main_ms: Option<f64>,
    pub launcher_pid: u32,
    pub app_pid: u32,
    /// 메모리를 합친 묶음의 키: 책임 프로세스가 이 pid 인 프로세스를 모았다(앱 pid 이거나 띄운 pid).
    pub group: u32,
    /// 띄운 프로세스와 첫 프레임을 낸 프로세스가 다른가(CLI 콜드 스타트는 앱을 떼어 다시 띄운다).
    pub detached: bool,
    pub memory: Memory,
    /// 끄고 나서도 남은 프로세스(pid). 비어야 한다.
    pub leftovers: Vec<u32>,
    /// SIGTERM 으로 끝나지 않아 SIGKILL 을 보냈는가.
    pub killed: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Memory {
    pub footprint_bytes: u64,
    pub rss_bytes: u64,
    pub processes: Vec<Process>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Process {
    pub pid: u32,
    pub name: String,
    pub footprint_bytes: u64,
    pub rss_bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Summaries {
    pub cold_start_ms: Option<Summary>,
    pub app_main_ms: Option<Summary>,
    pub footprint_bytes: Option<Summary>,
    pub rss_bytes: Option<Summary>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Targets {
    pub cold_start_ms: f64,
    pub idle_footprint_bytes: u64,
    /// 중앙값이 목표 안인가. 값이 없으면 `None`.
    pub cold_start_met: Option<bool>,
    pub idle_footprint_met: Option<bool>,
}

pub fn valid_runs(runs: &[Run]) -> usize {
    runs.iter()
        .filter(|r| !r.warmup && r.excluded.is_none())
        .count()
}

/// 유효한 회가 요청한 측정 회 수의 절반보다 적은가 — 중앙값이 몇 회의 값일 뿐이라 기준선으로 읽으면 안 된다.
pub fn too_few_valid(valid: usize, requested: usize) -> Option<String> {
    (valid * 2 < requested).then(|| {
        format!(
            "only {valid} of {requested} measured runs are valid (the rest were excluded); the medians are not a baseline — rerun with the project window unobscured"
        )
    })
}

/// 준비 운전(warm-up)과 제외한 회를 뺀 회만 모은다.
pub fn summarize_runs(runs: &[Run]) -> Summaries {
    let measured: Vec<&Run> = runs
        .iter()
        .filter(|r| !r.warmup && r.excluded.is_none())
        .collect();
    let values = |f: &dyn Fn(&Run) -> Option<f64>| -> Option<Summary> {
        summarize(&measured.iter().filter_map(|r| f(r)).collect::<Vec<_>>())
    };
    Summaries {
        cold_start_ms: values(&|r| Some(r.cold_start_ms)),
        app_main_ms: values(&|r| r.app_main_ms),
        footprint_bytes: values(&|r| Some(r.memory.footprint_bytes as f64)),
        rss_bytes: values(&|r| Some(r.memory.rss_bytes as f64)),
    }
}

pub fn targets(summary: &Summaries) -> Targets {
    Targets {
        cold_start_ms: TARGET_COLD_START_MS,
        idle_footprint_bytes: TARGET_IDLE_FOOTPRINT_BYTES,
        cold_start_met: summary
            .cold_start_ms
            .map(|s| s.median < TARGET_COLD_START_MS),
        idle_footprint_met: summary
            .footprint_bytes
            .map(|s| s.median < TARGET_IDLE_FOOTPRINT_BYTES as f64),
    }
}

fn mb(bytes: f64) -> String {
    format!("{:.1}", bytes / MB as f64)
}

fn met(value: Option<bool>) -> &'static str {
    match value {
        Some(true) => "within target",
        Some(false) => "over target",
        None => "no value",
    }
}

/// 사람이 읽는 요약(GitHub job summary 에도 그대로 붙인다).
pub fn markdown(report: &Report) -> String {
    let mut out = String::new();
    let push = |out: &mut String, line: String| {
        out.push_str(&line);
        out.push('\n');
    };
    push(&mut out, "## Pitwall performance baseline".into());
    push(&mut out, String::new());
    if let Some(warning) = &report.warning {
        push(&mut out, format!("> **Warning:** {warning}."));
        push(&mut out, String::new());
    }
    if report.build.profile != "release" {
        push(
            &mut out,
            format!(
                "> **Not a baseline:** measured a `{}` build. Baselines are release bundles.",
                report.build.profile
            ),
        );
        push(&mut out, String::new());
    }
    let commit = match (&report.commit.sha, report.commit.dirty) {
        (Some(sha), Some(true)) => format!("`{sha}` (uncommitted changes)"),
        (Some(sha), _) => format!("`{sha}`"),
        (None, _) => "unknown".into(),
    };
    let hw = &report.hardware;
    push(
        &mut out,
        format!(
            "- build: {} (`{}`)\n- commit: {commit}\n- os: {} {} ({})\n- hardware: {} · {} · {} CPUs · {} GiB",
            report.build.profile,
            report.build.app,
            report.os.name,
            report.os.version.as_deref().unwrap_or("?"),
            report.os.build.as_deref().unwrap_or("?"),
            hw.model.as_deref().unwrap_or("?"),
            hw.cpu.as_deref().unwrap_or("?"),
            hw.cpus.map_or("?".into(), |n| n.to_string()),
            hw.memory_bytes
                .map_or("?".into(), |b| (b / GIB).to_string()),
        ),
    );
    push(
        &mut out,
        format!(
            "- runs: {} measured ({} valid, {} excluded) + {} warm-up · idle = {}s after the first frame · MB = 10^6 bytes",
            report.settings.runs,
            report.valid_runs,
            report
                .runs
                .iter()
                .filter(|r| !r.warmup && r.excluded.is_some())
                .count(),
            report.settings.warmup,
            report.settings.idle_secs
        ),
    );
    push(&mut out, String::new());
    push(&mut out, "| metric | median | min – max | target |".into());
    push(&mut out, "| --- | --- | --- | --- |".into());
    let s = &report.summary;
    let t = &report.targets;
    if let Some(c) = s.cold_start_ms {
        push(
            &mut out,
            format!(
                "| cold start → project window first frame | {:.0} ms | {:.0} – {:.0} ms | < {:.0} ms ({}) |",
                c.median,
                c.min,
                c.max,
                t.cold_start_ms,
                met(t.cold_start_met)
            ),
        );
    }
    if let Some(m) = s.footprint_bytes {
        push(
            &mut out,
            format!(
                "| idle memory (phys_footprint, app + WebKit) | {} MB | {} – {} MB | < {} MB ({}) |",
                mb(m.median),
                mb(m.min),
                mb(m.max),
                t.idle_footprint_bytes / MB,
                met(t.idle_footprint_met)
            ),
        );
    }
    if let Some(r) = s.rss_bytes {
        push(
            &mut out,
            format!(
                "| idle memory (RSS, same processes; reference) | {} MB | {} – {} MB | |",
                mb(r.median),
                mb(r.min),
                mb(r.max)
            ),
        );
    }
    if let Some(a) = s.app_main_ms {
        push(
            &mut out,
            format!(
                "| start → detached app process enters `main` | {:.0} ms | {:.0} – {:.0} ms | |",
                a.median, a.min, a.max
            ),
        );
    }
    push(&mut out, String::new());
    push(
        &mut out,
        "| run | cold start (ms) | app main (ms) | footprint (MB) | RSS (MB) | processes |".into(),
    );
    push(&mut out, "| --- | --- | --- | --- | --- | --- |".into());
    for run in &report.runs {
        let names: Vec<String> = run
            .memory
            .processes
            .iter()
            .map(|p| format!("{} {}", p.name, mb(p.footprint_bytes as f64)))
            .collect();
        push(
            &mut out,
            format!(
                "| {}{}{} | {:.0} | {} | {} | {} | {} |",
                run.index,
                if run.warmup { " (warm-up)" } else { "" },
                run.excluded
                    .as_deref()
                    .map_or(String::new(), |why| format!(" (excluded: {why})")),
                run.cold_start_ms,
                run.app_main_ms.map_or("?".into(), |v| format!("{v:.0}")),
                mb(run.memory.footprint_bytes as f64),
                mb(run.memory.rss_bytes as f64),
                names.join(" · ")
            ),
        );
    }
    push(&mut out, String::new());
    let d = &report.definitions;
    push(
        &mut out,
        format!(
            "Start: {}. End: {}. Memory: {}; {}. Cache: {}.",
            d.start, d.end, d.memory, d.idle, d.cache
        ),
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(index: usize, warmup: bool, cold: f64, footprint: u64) -> Run {
        Run {
            index,
            warmup,
            excluded: None,
            cold_start_ms: cold,
            launcher_main_ms: Some(5.0),
            app_main_ms: Some(40.0),
            launcher_pid: 1,
            app_pid: 2,
            group: 2,
            detached: true,
            memory: Memory {
                footprint_bytes: footprint,
                rss_bytes: footprint * 2,
                processes: Vec::new(),
            },
            leftovers: Vec::new(),
            killed: false,
        }
    }

    #[test]
    fn warm_up_runs_stay_out_of_the_medians() {
        let runs = [
            run(0, true, 5000.0, 900 * MB),
            run(1, false, 700.0, 200 * MB),
            run(2, false, 900.0, 250 * MB),
            run(3, false, 800.0, 220 * MB),
        ];
        let s = summarize_runs(&runs);
        let cold = s.cold_start_ms.unwrap();
        assert_eq!((cold.median, cold.max, cold.n), (800.0, 900.0, 3));
        assert_eq!(s.footprint_bytes.unwrap().median, (220 * MB) as f64);
        let t = targets(&s);
        assert_eq!(t.cold_start_met, Some(true));
        assert_eq!(t.idle_footprint_met, Some(true));
    }

    #[test]
    fn excluded_runs_stay_out_of_the_medians() {
        let mut hidden = run(2, false, 47000.0, 100 * MB);
        hidden.excluded = Some("window hidden before its first frame".into());
        let runs = [run(1, false, 500.0, 116 * MB), hidden];
        let cold = summarize_runs(&runs).cold_start_ms.unwrap();
        assert_eq!((cold.median, cold.max, cold.n), (500.0, 500.0, 1));
    }

    #[test]
    fn fewer_valid_runs_than_half_the_request_warn() {
        assert_eq!(too_few_valid(3, 5), None);
        assert_eq!(too_few_valid(2, 4), None);
        assert!(too_few_valid(2, 5).unwrap().contains("only 2 of 5"));
        assert!(too_few_valid(0, 1).is_some());
        let mut hidden = run(2, false, 47000.0, 100 * MB);
        hidden.excluded = Some("hidden".into());
        let runs = [run(0, true, 900.0, 1), run(1, false, 500.0, 1), hidden];
        assert_eq!(valid_runs(&runs), 1);
    }

    /// 06 의 300MB 는 10^6 바이트 단위다 — 300 MiB(314.6 × 10^6)는 넘은 것으로 본다.
    #[test]
    fn the_memory_target_is_in_decimal_megabytes() {
        let runs = [run(0, false, 500.0, 300 * 1024 * 1024)];
        assert_eq!(
            targets(&summarize_runs(&runs)).idle_footprint_met,
            Some(false)
        );
        let runs = [run(0, false, 500.0, 299_000_000)];
        assert_eq!(
            targets(&summarize_runs(&runs)).idle_footprint_met,
            Some(true)
        );
    }

    #[test]
    fn a_median_over_the_target_is_reported_not_failed() {
        let runs = [run(0, false, 1600.0, 310 * MB)];
        let t = targets(&summarize_runs(&runs));
        assert_eq!(t.cold_start_met, Some(false));
        assert_eq!(t.idle_footprint_met, Some(false));
    }
}
