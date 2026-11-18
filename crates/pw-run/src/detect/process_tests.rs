//! 프로세스를 띄우는 태스크 감지(17-m6-plan M6-05)의 완료 기준 ① ~ ⑤ 와 캐시 · 증폭 상한.
//!
//! 가짜 도구(`mise` · `gradlew` · `mvnw`)는 셸 스크립트다 — 불리면 표지 파일에 한 줄을 더하고, 픽스처 출력을 낸다. 「띄우지
//! 않는다」 는 그 표지 파일의 줄 수로 본다.

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use pw_core::bus::{DEFAULT_CAPACITY, EventBus, Scope};
use pw_core::config::AppPaths;
use pw_core::registry::{NewProject, Registry};
use pw_core::task::TaskSpawner;
use pw_rpc::{Caller, ConnectionScope, EventHub, Id, Outbound, Role, Router, Session};
use pw_runtime::RuntimeEnv;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use super::exec::{ExecLimits, TrustedExec};
use super::*;
use crate::config::methods::{DetectHost, HostParts, RunConfigs, register};

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures");
const MISE_VERSIONS: [&str; 2] = ["2025.1.0", "2026.4.3"];

fn fixture(rel: &str) -> String {
    fs::read_to_string(Path::new(FIXTURES).join(rel)).unwrap()
}

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), &target).unwrap();
        }
    }
}

/// 실행 파일 스크립트를 쓴다.
fn script(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, format!("#!/bin/sh\n{body}")).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

/// 표지 파일의 줄 수(불린 수).
fn calls(marker: &Path) -> usize {
    fs::read_to_string(marker).map_or(0, |t| t.lines().count())
}

struct Site {
    _tmp: tempfile::TempDir,
    base: PathBuf,
    root: PathBuf,
    /// 가짜 도구를 두는 PATH 폴더.
    bin: PathBuf,
    marker: PathBuf,
}

fn site() -> Site {
    let tmp = tempfile::tempdir().unwrap();
    let base = fs::canonicalize(tmp.path()).unwrap();
    let root = base.join("proj");
    let bin = base.join("bin");
    fs::create_dir_all(&root).unwrap();
    fs::create_dir_all(&bin).unwrap();
    let marker = base.join("calls");
    Site {
        _tmp: tmp,
        base,
        root,
        bin,
        marker,
    }
}

fn env_with(bin: &Path) -> RuntimeEnv {
    RuntimeEnv {
        path: format!("{}:/usr/bin:/bin", bin.display()).into(),
        ..RuntimeEnv::default()
    }
}

fn exec(site: &Site, trusted: bool, limits: ExecLimits) -> TrustedExec {
    let bin = site.bin.clone();
    TrustedExec::new(
        "p",
        Arc::new(move |_| trusted),
        Arc::new(move |_, _| env_with(&bin)),
        limits,
    )
    .with_cache_dir(site.base.join("data"))
}

const QUICK: ExecLimits = ExecLimits {
    timeout: Duration::from_secs(20),
    max_output: 8 * 1024 * 1024,
};

/// 가짜 `mise`: 표지에 한 줄, `<fix>.stdout`(루트 자리를 바꾼 것) · `.stderr` · `.exit`.
fn fake_mise(site: &Site, version: &str, case: &str) {
    let dir = site.base.join("fix");
    fs::create_dir_all(&dir).unwrap();
    let src = format!("mise-tasks/{version}/{case}");
    let root = site.root.display().to_string();
    fs::write(
        dir.join("out"),
        fixture(&format!("{src}.stdout")).replace("/sandbox/proj", &root),
    )
    .unwrap();
    fs::write(dir.join("err"), fixture(&format!("{src}.stderr"))).unwrap();
    let code = fixture(&format!("{src}.exit")).trim().to_owned();
    script(
        &site.bin.join("mise"),
        &format!(
            "echo \"$*\" >> '{m}'\ncat '{d}/out'\ncat '{d}/err' >&2\nexit {code}\n",
            m = site.marker.display(),
            d = dir.display()
        ),
    );
}

fn mise_project(site: &Site) {
    copy_dir(&Path::new(FIXTURES).join("mise-tasks/project"), &site.root);
}

fn only(detector: &str) -> Vec<&'static dyn TaskDetector> {
    DETECTORS
        .iter()
        .copied()
        .filter(|d| d.id() == detector)
        .collect()
}

// --- ① mise 두 판의 `tasks ls --json` 픽스처 ---------------------------------------------------------------------

/// 두 판의 신뢰 출력: 숨은 태스크를 뺀 넷, 종류 `mise-task`, 정의 본문은 정의한 파일 전체의 sha256 과 이름.
#[test]
fn mise_fixtures_of_both_versions_parse_to_the_same_tasks() {
    for version in MISE_VERSIONS {
        let s = site();
        mise_project(&s);
        fake_mise(&s, version, "trusted");
        let detection = detect_with(&only(mise::ID), &s.root, &exec(&s, true, QUICK));
        assert_eq!(detection.problems, [], "{version}");
        let keys: Vec<String> = detection.tasks.iter().map(DetectedTask::key).collect();
        assert_eq!(
            keys,
            [
                "detected:mise:build",
                "detected:mise:deploy",
                "detected:mise:lint",
                "detected:mise:test:unit"
            ],
            "{version}"
        );
        let toml = sha256_hex(&fs::read_to_string(s.root.join("mise.toml")).unwrap());
        let deploy = sha256_hex(&fs::read_to_string(s.root.join("mise-tasks/deploy")).unwrap());
        for task in &detection.tasks {
            let file = if task.name == "deploy" {
                &deploy
            } else {
                &toml
            };
            assert_eq!(
                task.definition,
                file_definition(file, &task.name),
                "{version}"
            );
            assert_eq!(
                task.config.kind,
                crate::config::RunKindConfig::MiseTask {
                    task: task.name.clone()
                }
            );
        }
        assert_eq!(calls(&s.marker), 1, "{version}");
        assert_eq!(fs::read_to_string(&s.marker).unwrap(), "tasks ls --json\n");
    }
}

/// mise 가 설정을 신뢰하지 않으면(2026 은 실패 끝, 2025 는 성공 끝 + 경고 + 파일 태스크만) 반쪽 목록을 보이지 않고 감지기를 비운다.
/// 깨진 TOML 은 실패 — 까닭에 stderr(설정 파일의 줄)를 싣지 않는다.
#[test]
fn mise_untrusted_and_broken_outputs_empty_only_that_detector() {
    for version in MISE_VERSIONS {
        for (case, reason) in [
            ("untrusted", mise::NOT_TRUSTED_REASON),
            (
                "broken",
                "`mise` failed (exit status: 1); run it in a terminal to see why",
            ),
        ] {
            let s = site();
            mise_project(&s);
            fs::write(
                s.root.join("package.json"),
                r#"{"scripts": {"dev": "vite"}}"#,
            )
            .unwrap();
            fake_mise(&s, version, case);
            let detection = detect(&s.root, &exec(&s, true, QUICK));
            let keys: Vec<String> = detection.tasks.iter().map(DetectedTask::key).collect();
            assert_eq!(keys, ["detected:npm:dev"], "{version} {case}");
            assert_eq!(
                detection.problems,
                [DetectProblem {
                    detector: "mise".into(),
                    reason: reason.into()
                }],
                "{version} {case}"
            );
            assert!(!detection.problems[0].reason.contains("[tasks"));
        }
    }
}

/// 정의 본문은 정의한 파일 전체다 — `depends` · `env` · `dir` 만 고쳐도, 다른 태스크를 고쳐도 바뀐다(M6-11 F2).
#[test]
fn a_mise_definition_follows_its_whole_file() {
    let s = site();
    mise_project(&s);
    fake_mise(&s, "2026.4.3", "trusted");
    let definition = |s: &Site, name: &str| {
        detect_with(&only(mise::ID), &s.root, &exec(s, true, QUICK))
            .get(&key("mise", name))
            .unwrap()
            .definition
            .clone()
    };
    let before = definition(&s, "build");
    let deploy = definition(&s, "deploy");
    let toml = fs::read_to_string(s.root.join("mise.toml")).unwrap();
    fs::write(
        s.root.join("mise.toml"),
        toml.replace("depends = [\"lint\"]", "depends = [\"lint\", \"secret\"]"),
    )
    .unwrap();
    assert_ne!(definition(&s, "build"), before);
    assert_eq!(
        definition(&s, "deploy"),
        deploy,
        "another file's task did not change"
    );
    fs::write(s.root.join("mise-tasks/deploy"), "#!/bin/sh\necho other\n").unwrap();
    assert_ne!(definition(&s, "deploy"), deploy);
}

/// mise 설정도 파일 태스크 폴더도 없으면 mise 를 띄우지 않는다.
#[test]
fn mise_is_not_started_without_a_mise_config() {
    let s = site();
    fake_mise(&s, "2026.4.3", "trusted");
    let detection = detect(&s.root, &exec(&s, true, QUICK));
    assert_eq!(detection.tasks, []);
    assert_eq!(calls(&s.marker), 0);
}

// --- ② 미신뢰 프로젝트에서는 mise · gradle · maven 을 띄우지 않는다 ------------------------------------------------

fn gradle_project(site: &Site, gradlew_body: &str) {
    copy_dir(&Path::new(FIXTURES).join("gradle/project"), &site.root);
    script(
        &site.root.join("gradlew"),
        &format!("echo \"$*\" >> '{}'\n{gradlew_body}", site.marker.display()),
    );
}

fn gradle_prints_fixture(site: &Site) -> String {
    let out = site.base.join("gradle.out");
    fs::write(&out, fixture("gradle/9.5.0-tasks-all.stdout")).unwrap();
    format!("cat '{}'\n", out.display())
}

fn maven_project(site: &Site) {
    copy_dir(&Path::new(FIXTURES).join("maven/project"), &site.root);
    let pom = site.base.join("effective.xml");
    fs::write(&pom, fixture("maven/3.9.9-effective-pom.xml")).unwrap();
    // `-Doutput=<경로>` 의 경로로 픽스처를 쓴다.
    script(
        &site.root.join("mvnw"),
        &format!(
            "echo \"$*\" >> '{m}'\nfor a in \"$@\"; do case \"$a\" in -Doutput=*) cp '{p}' \"${{a#-Doutput=}}\";; esac; done\n",
            m = site.marker.display(),
            p = pom.display()
        ),
    );
}

/// 미신뢰면 감지(mise)도 읽기(gradle · maven)도 프로세스를 띄우지 않고 까닭을 남긴다. 신뢰 판정을 걷으면 표지가 늘어 실패한다.
#[test]
fn an_untrusted_project_starts_no_detection_process() {
    let s = site();
    mise_project(&s);
    fake_mise(&s, "2026.4.3", "trusted");
    gradle_project(&s, &gradle_prints_fixture(&s));
    let untrusted = exec(&s, false, QUICK);
    let detection = detect(&s.root, &untrusted);
    assert_eq!(calls(&s.marker), 0, "{detection:#?}");
    assert!(
        detection
            .problems
            .iter()
            .any(|p| p.detector == "mise" && p.reason == exec::untrusted_reason("mise")),
        "{detection:#?}"
    );
    assert!(gradle::read(&s.root, &untrusted).is_err());
    let m = site();
    maven_project(&m);
    assert!(maven::read(&m.root, &exec(&m, false, QUICK)).is_err());
    assert_eq!(calls(&s.marker) + calls(&m.marker), 0);
}

// --- ③ gradle · maven 은 「태스크 읽기」 전에 프로세스를 띄우지 않는다 ---------------------------------------------

/// 읽기 전: 래퍼가 있어도 띄우지 않고 흔한 태스크만(상태 notRead). 읽은 뒤: 픽스처의 태스크(상태 read), 다시 감지해도 띄우지 않는다.
/// 빌드 파일이 바뀌면 흔한 태스크로 돌아가고 상태 stale.
#[test]
fn gradle_starts_nothing_until_tasks_are_read() {
    let s = site();
    gradle_project(&s, &gradle_prints_fixture(&s));
    let e = exec(&s, true, QUICK);
    let names = |d: &Detection| d.tasks.iter().map(|t| t.name.clone()).collect::<Vec<_>>();
    let before = detect(&s.root, &e);
    assert_eq!(calls(&s.marker), 0);
    assert_eq!(names(&before), ["build", "test"]);
    assert_eq!(
        before.lists,
        [TaskList {
            detector: "gradle".into(),
            state: TaskListState::NotRead
        }]
    );

    let read = gradle::read(&s.root, &e).unwrap();
    assert_eq!(calls(&s.marker), 1);
    assert_eq!(
        fs::read_to_string(&s.marker).unwrap(),
        "tasks --all --quiet --console=plain --no-daemon -Dorg.gradle.daemon=false\n"
    );
    let key = cache::key(&s.root, cache::BuildFiles::Gradle).unwrap();
    cache::store(e.cache_dir().unwrap(), "gradle", &key, &read).unwrap();

    let after = detect(&s.root, &e);
    assert_eq!(
        calls(&s.marker),
        1,
        "listing after reading started gradle again"
    );
    assert_eq!(after.lists[0].state, TaskListState::Read);
    assert_eq!(after.tasks.len(), read.len());
    let run = after.get("detected:gradle:app:run").unwrap();
    assert_eq!(
        run.config.kind,
        crate::config::RunKindConfig::Gradle {
            tasks: vec!["app:run".into()]
        }
    );
    assert_eq!(run.definition, "app:run");

    fs::write(
        s.root.join("gradle.properties"),
        "org.gradle.jvmargs=-Xmx1g\n",
    )
    .unwrap();
    let stale = detect(&s.root, &e);
    assert_eq!(stale.lists[0].state, TaskListState::Stale);
    assert_eq!(names(&stale), ["build", "test"]);
    assert_eq!(calls(&s.marker), 1);
}

/// Spring Boot 플러그인이 보이면 읽기 전에도 `bootRun`, Maven 은 라이프사이클과 그 goal.
#[test]
fn common_tasks_follow_the_plugins_seen_in_the_root_build_file() {
    let s = site();
    fs::write(
        s.root.join("build.gradle.kts"),
        "plugins { id(\"org.springframework.boot\") version \"3.4.0\" }\n",
    )
    .unwrap();
    fs::write(
        s.root.join("pom.xml"),
        "<project><build><plugins><plugin><artifactId>spring-boot-maven-plugin</artifactId></plugin></plugins></build></project>",
    )
    .unwrap();
    let detection = detect(&s.root, &exec(&s, true, QUICK));
    let keys: Vec<String> = detection.tasks.iter().map(DetectedTask::key).collect();
    assert_eq!(
        keys,
        [
            "detected:gradle:bootRun",
            "detected:gradle:build",
            "detected:gradle:test",
            "detected:maven:clean",
            "detected:maven:compile",
            "detected:maven:install",
            "detected:maven:package",
            "detected:maven:spring-boot:run",
            "detected:maven:test",
            "detected:maven:verify",
        ]
    );
    assert_eq!(calls(&s.marker), 0);
}

/// 실제 `gradle tasks --all`(9.5.0) 출력: 무리 머리 · `Rules` 를 빼고 하위 프로젝트 경로까지.
#[test]
fn the_gradle_fixture_parses_every_task() {
    let names = gradle::parse_tasks(&fixture("gradle/9.5.0-tasks-all.stdout"));
    assert_eq!(names.len(), 77, "{names:?}");
    for expected in [
        "app:run",
        "hello",
        "prepareKotlinBuildScriptModel",
        "lib:test",
        "wrapper",
        "tasks",
    ] {
        assert!(
            names.iter().any(|n| n == expected),
            "{expected} in {names:?}"
        );
    }
    assert!(
        !names
            .iter()
            .any(|n| n.starts_with("Pattern") || n.starts_with("Tasks"))
    );
    assert!(!names.iter().any(|n| n.contains(' ') || n.starts_with('-')));
    let noisy = format!(
        "> Task :tasks\n\n{}\nRules\n-----\nPattern: clean<TaskName>: Cleans the output files of a task.\n\nBUILD SUCCESSFUL in 1s\n",
        fixture("gradle/9.5.0-tasks-all.stdout")
    );
    assert_eq!(gradle::parse_tasks(&noisy), names);
}

/// 실제 effective POM(Maven 3.9.9, 두 모듈): `build/plugins` 의 execution goal 만 접두사 관례로. `pluginManagement` · 보고 플러그인은
/// 빠진다.
#[test]
fn the_effective_pom_fixture_gives_plugin_goals() {
    let goals = maven::plugin_goals(&fixture("maven/3.9.9-effective-pom.xml")).unwrap();
    assert_eq!(
        goals,
        [
            "exec:exec",
            "clean:clean",
            "install:install",
            "deploy:deploy",
            "site:site",
            "site:deploy",
            "surefire:test",
            "resources:testResources",
            "resources:resources",
            "jar:jar",
            "compiler:compile",
            "compiler:testCompile",
        ]
    );
    assert_eq!(maven::prefix("maven-surefire-plugin"), Some("surefire"));
    assert_eq!(
        maven::prefix("spring-boot-maven-plugin"),
        Some("spring-boot")
    );
    assert_eq!(maven::prefix("frontend-plugin"), None);
}

/// DTD 가 있으면 읽지 않고, 엔티티 참조가 든 값은 버린다(펼치지 않는다).
#[test]
fn the_effective_pom_reader_expands_no_entity() {
    let dtd = "<?xml version=\"1.0\"?><!DOCTYPE p [<!ENTITY a \"aaaaaaaaaa\"><!ENTITY b \"&a;&a;&a;&a;\">]><project/>";
    assert!(maven::plugin_goals(dtd).unwrap_err().contains("DOCTYPE"));
    let refs = "<project><build><plugins><plugin><artifactId>maven-x-plugin</artifactId><executions><execution><goals><goal>&b;</goal><goal>ok</goal></goals></execution></executions></plugin></plugins></build></project>";
    assert_eq!(maven::plugin_goals(refs).unwrap(), ["x:ok"]);
}

/// `pluginManagement` · `reporting` · `profiles` 아래의 플러그인은 그 execution 이 있어도 goal 로 나오지 않는다(빌드에 묶이지 않았다).
#[test]
fn only_build_plugins_give_goals() {
    let plugin = |a: &str, g: &str| {
        format!(
            "<plugin><artifactId>{a}</artifactId><executions><execution><goals><goal>{g}</goal></goals></execution></executions></plugin>"
        )
    };
    let xml = format!(
        "<projects><project><build><pluginManagement><plugins>{}</plugins></pluginManagement><plugins>{}</plugins></build><reporting><plugins>{}</plugins></reporting><profiles><profile><build><plugins>{}</plugins></build></profile></profiles></project></projects>",
        plugin("maven-managed-plugin", "m"),
        plugin("maven-used-plugin", "u"),
        plugin("maven-report-plugin", "r"),
        plugin("maven-profile-plugin", "p"),
    );
    assert_eq!(maven::plugin_goals(&xml).unwrap(), ["used:u"]);
}

/// Maven 읽기: 0700 임시 폴더의 파일로 받고 끝나면 지운다. 라이프사이클 + goal.
#[test]
fn maven_reads_the_effective_pom_through_a_private_temporary_file() {
    let s = site();
    maven_project(&s);
    let names = maven::read(&s.root, &exec(&s, true, QUICK)).unwrap();
    assert_eq!(&names[..6], maven::LIFECYCLE);
    assert!(names.contains(&"exec:exec".to_owned()), "{names:?}");
    let args = fs::read_to_string(&s.marker).unwrap();
    let output = args
        .split_whitespace()
        .find_map(|a| a.strip_prefix("-Doutput="))
        .unwrap();
    assert!(args.starts_with("--batch-mode --quiet help:effective-pom -Doutput="));
    let folder = Path::new(output).parent().unwrap();
    assert!(
        !folder.exists(),
        "the temporary folder {} was left",
        folder.display()
    );
}

// --- ④ 시한을 넘긴 감지기는 끝내고 그 감지기만 비운다 · 남기지 않는다 --------------------------------------------------

/// mise 가 멈추면 시한에 세션째 끝내고 mise 만 빈다(npm 은 그대로). 손자도 남지 않는다.
#[test]
fn a_detector_past_its_time_limit_is_ended_and_only_it_is_emptied() {
    let s = site();
    mise_project(&s);
    fs::write(
        s.root.join("package.json"),
        r#"{"scripts": {"dev": "vite"}}"#,
    )
    .unwrap();
    let pid = s.base.join("pid");
    script(
        &s.bin.join("mise"),
        &format!(
            "(trap '' HUP; exec sleep 60) & echo $! > '{}'\nsleep 60\n",
            pid.display()
        ),
    );
    let short = ExecLimits {
        timeout: Duration::from_millis(500),
        max_output: 1024,
    };
    let started = Instant::now();
    let detection = detect(&s.root, &exec(&s, true, short));
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );
    let keys: Vec<String> = detection.tasks.iter().map(DetectedTask::key).collect();
    assert_eq!(keys, ["detected:npm:dev"]);
    assert_eq!(
        detection.problems,
        [DetectProblem {
            detector: "mise".into(),
            reason: "`mise` did not finish within 500ms; Pitwall stopped it".into()
        }]
    );
    assert_gone(&pid);
}

/// gradle 이 끝나며 남긴 것(1회용 데몬 · 빌드 스크립트가 띄운 것)은 함께 끝난다.
#[test]
fn reading_gradle_tasks_leaves_no_process_behind() {
    let s = site();
    let pid = s.base.join("pid");
    let body = format!(
        "(trap '' HUP; exec sleep 60) > /dev/null 2>&1 & echo $! > '{}'\n{}",
        pid.display(),
        gradle_prints_fixture(&s)
    );
    gradle_project(&s, &body);
    let names = gradle::read(&s.root, &exec(&s, true, QUICK)).unwrap();
    assert_eq!(names.len(), 77);
    assert_gone(&pid);
}

/// 출력이 상한을 넘으면 끝내고 까닭이다.
#[test]
fn output_over_the_limit_is_refused() {
    let s = site();
    gradle_project(&s, "head -c 100000 /dev/zero\n");
    let small = ExecLimits {
        timeout: Duration::from_secs(10),
        max_output: 4096,
    };
    let err = gradle::read(&s.root, &exec(&s, true, small)).unwrap_err();
    assert!(err.contains("printed more than"), "{err}");
}

fn assert_gone(pidfile: &Path) {
    let deadline = Instant::now() + Duration::from_secs(5);
    let pid: i32 = loop {
        if let Ok(text) = fs::read_to_string(pidfile)
            && let Ok(pid) = text.trim().parse()
        {
            break pid;
        }
        assert!(Instant::now() < deadline, "no pid written");
        std::thread::sleep(Duration::from_millis(10));
    };
    // SAFETY: 신호 0 은 프로세스가 있는지만 본다.
    while unsafe { libc::kill(pid, 0) } == 0 {
        if Instant::now() > deadline {
            // SAFETY: 시험이 띄운 손자만 끝낸다.
            unsafe { libc::kill(pid, libc::SIGKILL) };
            panic!("the process {pid} was left behind");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

// --- 증폭 상한 ----------------------------------------------------------------------------------------------------

/// 파일 태스크 수백 개 × 큰 파일: 정의 본문을 위해 읽는 합계가 상한을 넘으면 그 감지기를 비운다(다 읽지 않는다).
#[test]
fn mise_sources_are_read_within_a_total_budget() {
    let s = site();
    mise_project(&s);
    let big = "x".repeat(900 * 1024);
    let mut listed = Vec::new();
    for i in 0..40 {
        let path = s.root.join(format!("mise-tasks/t{i:02}"));
        fs::write(&path, &big).unwrap();
        listed.push(json!({"name": format!("t{i:02}"), "source": path.display().to_string()}));
    }
    let out = s.base.join("out.json");
    fs::write(&out, serde_json::to_string(&listed).unwrap()).unwrap();
    script(&s.bin.join("mise"), &format!("cat '{}'\n", out.display()));
    let started = Instant::now();
    let detection = detect_with(&only(mise::ID), &s.root, &exec(&s, true, QUICK));
    assert!(started.elapsed() < Duration::from_secs(5));
    assert_eq!(detection.tasks, []);
    assert!(
        detection.problems[0]
            .reason
            .contains("add up to more than 16 MiB"),
        "{:?}",
        detection.problems
    );
}

/// 태스크 수천 개를 내는 출력은 이름순 앞 500 개만 — 까닭을 남긴다.
#[test]
fn many_gradle_tasks_are_capped() {
    let mut out = String::from("Other tasks\n-----------\n");
    for i in 0..3000 {
        out.push_str(&format!("t{i:04} - task\n"));
    }
    let s = site();
    let file = s.base.join("many.out");
    fs::write(&file, &out).unwrap();
    gradle_project(&s, &format!("cat '{}'\n", file.display()));
    let e = exec(&s, true, QUICK);
    let names = gradle::read(&s.root, &e).unwrap();
    let key = cache::key(&s.root, cache::BuildFiles::Gradle).unwrap();
    cache::store(e.cache_dir().unwrap(), "gradle", &key, &names).unwrap();
    let detection = detect_with(&only(gradle::ID), &s.root, &e);
    assert_eq!(detection.tasks.len(), MAX_TASKS);
    assert!(
        detection.problems[0].reason.contains("found 3000 tasks"),
        "{:?}",
        detection.problems
    );
}

// --- 캐시 --------------------------------------------------------------------------------------------------------

/// 열쇠는 빌드 파일 · 런타임 원천 파일의 내용이다(수정 시각이 아니라). 경로는 루트 기준이다 — 같은 내용의 다른 폴더는 같은 열쇠.
#[test]
fn the_cache_key_follows_build_and_runtime_files() {
    let s = site();
    gradle_project(&s, "true\n");
    let key = || cache::key(&s.root, cache::BuildFiles::Gradle).unwrap();
    let k0 = key();
    assert_eq!(key(), k0);
    let edits: [(&str, &str); 4] = [
        ("app/build.gradle.kts", "plugins { application }\n// x\n"),
        (
            "gradle/wrapper/gradle-wrapper.properties",
            "distributionUrl=x\n",
        ),
        ("gradle/libs.versions.toml", "[versions]\n"),
        (".tool-versions", "java 21\n"),
    ];
    let mut last = k0.clone();
    for (rel, text) in edits {
        let path = s.root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, text).unwrap();
        let next = key();
        assert_ne!(next, last, "{rel}");
        last = next;
    }
    // 산출물 폴더 · 숨은 폴더의 같은 이름은 보지 않는다.
    for rel in [
        "build/build.gradle",
        ".gradle/build.gradle",
        "node_modules/x/build.gradle",
    ] {
        let path = s.root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "x").unwrap();
    }
    assert_eq!(key(), last);
    let other = s.base.join("copy");
    copy_dir(&s.root, &other);
    assert_eq!(cache::key(&other, cache::BuildFiles::Gradle).unwrap(), last);
}

/// 깨진 캐시 · 다른 판은 버리고 읽기 전 상태다.
#[test]
fn a_broken_or_older_cache_is_discarded() {
    let s = site();
    gradle_project(&s, "true\n");
    let e = exec(&s, true, QUICK);
    let dir = e.cache_dir().unwrap().to_path_buf();
    fs::create_dir_all(&dir).unwrap();
    let key = cache::key(&s.root, cache::BuildFiles::Gradle).unwrap();
    for text in [
        "{ not json".to_owned(),
        json!({"version": cache::VERSION + 1, "lists": {"gradle": [{"key": key, "tasks": ["x"]}]}})
            .to_string(),
    ] {
        fs::write(dir.join(cache::FILE), text).unwrap();
        let detection = detect_with(&only(gradle::ID), &s.root, &e);
        assert_eq!(detection.lists[0].state, TaskListState::NotRead);
        let names: Vec<&str> = detection.tasks.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["build", "test"]);
    }
    cache::store(&dir, "gradle", &key, &["x".to_owned()]).unwrap();
    assert_eq!(
        cache::lookup(&dir, "gradle", || Ok(key.clone())).unwrap(),
        cache::Lookup::Hit(vec!["x".to_owned()])
    );
}

// --- ⑤ run.tasks.refresh 의 범위 · RPC -------------------------------------------------------------------------------

struct Host {
    _site: Site,
    configs: Arc<RunConfigs>,
    project: String,
    tasks: TaskSpawner,
    root: PathBuf,
    marker: PathBuf,
    /// `$DATA/projects/<id>/` — 캐시 자리.
    cache_dir: PathBuf,
    projects: Registry,
}

fn host(trusted: bool, setup: impl FnOnce(&Site)) -> Host {
    let s = site();
    setup(&s);
    let paths = AppPaths::under(&s.base.join("app"));
    let bus = EventBus::new(DEFAULT_CAPACITY);
    let projects = Registry::new(paths.clone(), None, bus.clone());
    let project = projects
        .create(NewProject {
            root: s.root.clone(),
            ..NewProject::default()
        })
        .unwrap()
        .id;
    if trusted {
        projects.set_trust(&project, true).unwrap();
    }
    let cache_dir = paths.data.join("projects").join(&project);
    let configs = Arc::new(RunConfigs::new());
    configs.attach(HostParts {
        paths,
        projects: projects.clone(),
    });
    let tasks = TaskSpawner::new(bus, Scope::Host, CancellationToken::new());
    let bin = s.bin.clone();
    configs.attach_detect(DetectHost {
        env: Arc::new(move |_, _| env_with(&bin)),
        tasks: tasks.clone(),
    });
    Host {
        root: s.root.clone(),
        marker: s.marker.clone(),
        _site: s,
        configs,
        project,
        tasks,
        cache_dir,
        projects,
    }
}

async fn call(
    host: &Host,
    scope: ConnectionScope,
    method: &str,
    params: Value,
) -> Result<Value, (i32, Value)> {
    let mut router = Router::new();
    register(&mut router, Arc::clone(&host.configs));
    let (session, mut rx) = Session::new(
        Arc::new(router),
        EventHub::new(),
        Caller {
            role: Role::Owner,
            scope,
            window: None,
        },
    );
    session.handle_value(json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}));
    loop {
        let message = tokio::time::timeout(Duration::from_secs(30), rx.recv())
            .await
            .expect("timed out")
            .expect("session closed");
        if let Outbound::Response(r) = message {
            assert_eq!(r.id, Id::Number(1));
            return r
                .outcome
                .map_err(|e| (e.code, e.data.unwrap_or(Value::Null)));
        }
    }
}

fn refresh_params(h: &Host, detector: &str) -> Value {
    json!({"projectId": h.project, "detector": detector})
}

/// Host · 다른 프로젝트 창 · 파일 창 · 에이전트 범위는 `PERMISSION_DENIED` 이고 아무것도 띄우지 않는다. 미신뢰면 그 프로젝트
/// 창이어도 `untrusted`.
#[tokio::test(flavor = "multi_thread")]
async fn refresh_is_for_the_trusted_projects_own_window_only() {
    let h = host(true, |s| gradle_project(s, &gradle_prints_fixture(s)));
    let outsiders = [
        ConnectionScope::Host,
        ConnectionScope::Project("someone-else".into()),
        ConnectionScope::Agent,
        Caller::file(Role::Owner, h.root.join("build.gradle.kts")).scope,
    ];
    for scope in outsiders {
        let err = call(
            &h,
            scope.clone(),
            "run.tasks.refresh",
            refresh_params(&h, "gradle"),
        )
        .await
        .unwrap_err();
        assert_eq!(err.0, pw_rpc::code::PERMISSION_DENIED, "{scope:?}");
    }
    assert_eq!(calls(&h.marker), 0);

    let u = host(false, |s| gradle_project(s, &gradle_prints_fixture(s)));
    let err = call(
        &u,
        ConnectionScope::Project(u.project.clone()),
        "run.tasks.refresh",
        refresh_params(&u, "gradle"),
    )
    .await
    .unwrap_err();
    assert_eq!(err.0, pw_rpc::code::PERMISSION_DENIED);
    assert_eq!(err.1["reason"], "untrusted");
    assert_eq!(calls(&u.marker), 0);

    let err = call(
        &h,
        ConnectionScope::Project(h.project.clone()),
        "run.tasks.refresh",
        refresh_params(&h, "npm"),
    )
    .await
    .unwrap_err();
    assert_eq!(err.0, pw_rpc::code::INVALID_PARAMS);
}

/// 그 프로젝트 창의 누름: gradle 을 한 번 돌려 캐시하고, 그 뒤 `run.config.list` 는 읽은 목록을 띄우지 않고 보인다.
#[tokio::test(flavor = "multi_thread")]
async fn refreshing_gradle_reads_once_and_the_list_keeps_it() {
    let h = host(true, |s| gradle_project(s, &gradle_prints_fixture(s)));
    let scope = ConnectionScope::Project(h.project.clone());
    let list = call(
        &h,
        scope.clone(),
        "run.config.list",
        json!({"projectId": h.project}),
    )
    .await
    .unwrap();
    assert_eq!(
        list["taskLists"],
        json!([{"detector": "gradle", "state": "notRead"}])
    );
    assert_eq!(calls(&h.marker), 0);
    let refreshed = call(
        &h,
        scope.clone(),
        "run.tasks.refresh",
        refresh_params(&h, "gradle"),
    )
    .await
    .unwrap();
    assert_eq!(refreshed["error"], Value::Null);
    assert_eq!(
        refreshed["taskLists"],
        json!([{"detector": "gradle", "state": "read"}])
    );
    assert_eq!(refreshed["detected"].as_array().unwrap().len(), 77);
    assert_eq!(calls(&h.marker), 1);
    let list = call(
        &h,
        scope,
        "run.config.list",
        json!({"projectId": h.project}),
    )
    .await
    .unwrap();
    assert_eq!(
        list["taskLists"],
        json!([{"detector": "gradle", "state": "read"}])
    );
    assert_eq!(list["detected"].as_array().unwrap().len(), 77);
    assert_eq!(calls(&h.marker), 1);
    assert!(h.tasks.running(Some(&h.project)).is_empty());
}

/// 실패한 읽기는 까닭을 돌려주고 캐시를 건드리지 않는다.
#[tokio::test(flavor = "multi_thread")]
async fn a_failed_refresh_reports_why_and_keeps_the_static_list() {
    let h = host(true, |s| {
        gradle_project(s, "echo 'secret=hunter2' >&2\nexit 3\n")
    });
    let refreshed = call(
        &h,
        ConnectionScope::Project(h.project.clone()),
        "run.tasks.refresh",
        refresh_params(&h, "gradle"),
    )
    .await
    .unwrap();
    let error = refreshed["error"].as_str().unwrap();
    assert_eq!(
        error,
        "`gradlew` failed (exit status: 3); run it in a terminal to see why"
    );
    assert!(!error.contains("hunter2"));
    assert_eq!(
        refreshed["taskLists"],
        json!([{"detector": "gradle", "state": "notRead"}])
    );
}

/// 읽기는 프로젝트 범위 Host 작업이다 — `task.cancel`(여기서는 spawner 의 `cancel`)로 멈추면 gradle 을 세션째 끝내고 `CANCELLED`.
#[tokio::test(flavor = "multi_thread")]
async fn a_running_refresh_is_a_cancellable_host_task() {
    let pid_holder = Arc::new(std::sync::Mutex::new(PathBuf::new()));
    let holder = Arc::clone(&pid_holder);
    let h = host(true, move |s| {
        let pid = s.base.join("pid");
        *holder.lock().unwrap() = pid.clone();
        gradle_project(
            s,
            &format!(
                "(trap '' HUP; exec sleep 60) & echo $! > '{}'\nsleep 60\n",
                pid.display()
            ),
        );
    });
    let pid = pid_holder.lock().unwrap().clone();
    let configs = Arc::clone(&h.configs);
    let scope = ConnectionScope::Project(h.project.clone());
    let params: crate::detect::refresh::RunTasksRefreshParams =
        serde_json::from_value(refresh_params(&h, "gradle")).unwrap();
    let call = tokio::spawn(async move { configs.refresh(&scope, params).await });
    let deadline = Instant::now() + Duration::from_secs(10);
    let task = loop {
        if let Some(task) = h.tasks.running(Some(&h.project)).into_iter().next() {
            break task;
        }
        assert!(Instant::now() < deadline, "no host task appeared");
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    assert_eq!(task.title, "Reading Gradle tasks");
    assert_eq!(task.project_id.as_deref(), Some(h.project.as_str()));
    while !pid.exists() {
        assert!(Instant::now() < deadline, "gradlew did not start");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(h.tasks.cancel(task.task_id));
    let err = tokio::time::timeout(Duration::from_secs(10), call)
        .await
        .expect("the refresh did not end after cancel")
        .unwrap()
        .unwrap_err();
    assert_eq!(err.code, pw_rpc::code::CANCELLED);
    tokio::task::spawn_blocking(move || assert_gone(&pid))
        .await
        .unwrap();
}

// --- 로컬 live: 실제 mise ------------------------------------------------------------------------------------------

/// `PITWALL_LIVE_MISE=1` 일 때만: 로그인 셸 PATH 의 실제 mise 로 `mise.toml` 의 태스크 둘(격리한 mise 폴더 — 사람의 신뢰 목록을
/// 건드리지 않는다).
#[test]
fn live_mise_lists_two_tasks() {
    if std::env::var_os("PITWALL_LIVE_MISE").is_none() {
        eprintln!("skipped: set PITWALL_LIVE_MISE=1 to run against the real mise");
        return;
    }
    let s = site();
    fs::write(
        s.root.join("mise.toml"),
        "[tasks.hello]\nrun = \"echo hello\"\n\n[tasks.\"build:prod\"]\nrun = \"echo build\"\n",
    )
    .unwrap();
    let real =
        pw_runtime::mise::find_mise(&pw_runtime::RuntimeEnv::process()).expect("mise on PATH");
    let mut vars = std::collections::BTreeMap::new();
    for (name, dir) in [
        ("MISE_DATA_DIR", "data"),
        ("MISE_CONFIG_DIR", "config"),
        ("MISE_STATE_DIR", "state"),
        ("MISE_CACHE_DIR", "cache"),
        ("HOME", "home"),
    ] {
        let path = s.base.join("mise").join(dir);
        fs::create_dir_all(&path).unwrap();
        vars.insert(name.into(), path.into_os_string());
    }
    let env = RuntimeEnv {
        path: format!("{}:/usr/bin:/bin", real.parent().unwrap().display()).into(),
        vars,
        ..RuntimeEnv::default()
    };
    let status = std::process::Command::new(&real)
        .arg("trust")
        .current_dir(&s.root)
        .env_clear()
        .envs(&env.vars)
        .env("PATH", &env.path)
        .status()
        .unwrap();
    assert!(status.success());
    let exec = TrustedExec::new(
        "p",
        Arc::new(|_| true),
        Arc::new(move |_, _| env.clone()),
        QUICK,
    );
    let detection = detect_with(&only(mise::ID), &s.root, &exec);
    let keys: Vec<String> = detection.tasks.iter().map(DetectedTask::key).collect();
    assert_eq!(
        keys,
        ["detected:mise:build:prod", "detected:mise:hello"],
        "{detection:#?}"
    );
}

// --- 파일 감시: 버전 파일이 바뀌면 mise 감지를 다시 한다 ----------------------------------------------------------------

/// 감시는 목록과 같은 실행기로 다시 감지한다 — 내용이 같은 `mise.toml` 이벤트는 알리지 않고(실행기가 달라 지문이 갈리면 실패),
/// 파일 태스크 폴더 안의 파일이 바뀌면 알린다.
#[tokio::test(flavor = "multi_thread")]
async fn a_changed_mise_file_announces_the_project_with_the_same_executor() {
    let h = host(true, |s| {
        mise_project(s);
        fake_mise(s, "2026.4.3", "trusted");
    });
    let bus = EventBus::new(DEFAULT_CAPACITY);
    let mut announced =
        bus.subscribe(pw_core::bus::Filter::all().topics([pw_core::config::TOPIC_CHANGED]));
    tokio::spawn(follow_files(
        subscribe_files(&bus),
        h.configs.seen(),
        bus.clone(),
    ));
    let list = call(
        &h,
        ConnectionScope::Project(h.project.clone()),
        "run.config.list",
        json!({"projectId": h.project}),
    )
    .await
    .unwrap();
    assert_eq!(list["detected"].as_array().unwrap().len(), 4, "{list:#}");
    let publish = |path: PathBuf| {
        bus.publish(
            Scope::Project(h.project.clone()),
            "fs.changed",
            &json!({"root": h.root.display().to_string(), "worktree": null, "paths": [path.display().to_string()], "kinds": ["modified"], "rescan": false}),
        )
        .unwrap();
    };
    publish(h.root.join("mise.toml"));
    assert!(
        tokio::time::timeout(Duration::from_millis(700), announced.recv())
            .await
            .is_err(),
        "an unchanged mise.toml was announced"
    );
    let before = calls(&h.marker);
    assert!(before >= 2, "the watch did not run mise again ({before})");
    fs::write(
        h.root.join("mise-tasks/deploy"),
        "#!/bin/sh\necho changed\n",
    )
    .unwrap();
    publish(h.root.join("mise-tasks/deploy"));
    let received = tokio::time::timeout(Duration::from_secs(10), announced.recv())
        .await
        .expect("no config.changed")
        .unwrap();
    let pw_core::bus::Received::Event(event) = received else {
        panic!("{received:?}")
    };
    assert_eq!(event.payload["changed"], json!([watch::CHANGED_KEY]));
}

// --- 리뷰 반영(M6-05 리뷰 고쳐야 1 · 2, 권고) ------------------------------------------------------------------------------

/// gradle 의 1회용 데몬 꼴: 스스로 `setsid` 하고 SIGTERM 을 무시하는(인터럽트를 삼키는 빌드 스크립트) 자손. 실행기는 그것을 도는
/// 동안 부모로 따라가 기록했다가 — 클라이언트가 먼저 끝나 init 으로 넘어가도 — 정상 끝 · 시한 · 취소에서 SIGKILL 까지 끝낸다.
fn escaping_daemon(pidfile: &Path) -> String {
    format!(
        "perl -e 'use POSIX; POSIX::setsid(); $SIG{{TERM}}=\"IGNORE\"; open(F, \">\", \"{p}.tmp\"); print F $$; close F; rename(\"{p}.tmp\", \"{p}\"); sleep 60' > /dev/null 2>&1 &\nwhile [ ! -f '{p}' ]; do sleep 0.05; done\nsleep 0.4\n",
        p = pidfile.display()
    )
}

#[test]
fn a_setsid_descendant_ignoring_sigterm_is_ended_in_every_way_a_read_ends() {
    // 정상 끝: 데몬을 남기고 클라이언트는 목록을 내고 끝난다.
    let s = site();
    let pid = s.base.join("daemon.pid");
    let body = format!("{}{}", escaping_daemon(&pid), gradle_prints_fixture(&s));
    gradle_project(&s, &body);
    let names = gradle::read(&s.root, &exec(&s, true, QUICK)).unwrap();
    assert_eq!(names.len(), 77);
    assert_gone(&pid);

    // 시한: 클라이언트도 멈춰 있다.
    let s = site();
    let pid = s.base.join("daemon.pid");
    gradle_project(&s, &format!("{}sleep 60\n", escaping_daemon(&pid)));
    let short = ExecLimits {
        timeout: Duration::from_secs(3),
        max_output: 1024,
    };
    let err = gradle::read(&s.root, &exec(&s, true, short)).unwrap_err();
    assert!(err.contains("did not finish within"), "{err}");
    assert_gone(&pid);

    // 취소.
    let s = site();
    let pid = s.base.join("daemon.pid");
    gradle_project(&s, &format!("{}sleep 60\n", escaping_daemon(&pid)));
    let cancel = pw_runtime::Cancel::default();
    let e = exec(&s, true, QUICK).with_cancel(cancel.clone());
    let root = s.root.clone();
    let reading = std::thread::spawn(move || gradle::read(&root, &e));
    let deadline = Instant::now() + Duration::from_secs(10);
    while !pid.exists() {
        assert!(Instant::now() < deadline, "the daemon did not start");
        std::thread::sleep(Duration::from_millis(20));
    }
    std::thread::sleep(Duration::from_millis(300));
    cancel.cancel();
    let err = reading.join().unwrap().unwrap_err();
    assert!(err.contains("was cancelled"), "{err}");
    assert_gone(&pid);
}

/// 도는 동안 사람이 신뢰를 거두면 읽기를 멈추고(세션 · 자손째) 미신뢰 까닭이다.
#[test]
fn withdrawing_trust_stops_a_running_read() {
    let s = site();
    let pid = s.base.join("daemon.pid");
    gradle_project(&s, &format!("{}sleep 60\n", escaping_daemon(&pid)));
    let trusted = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let flag = Arc::clone(&trusted);
    let bin = s.bin.clone();
    let e = TrustedExec::new(
        "p",
        Arc::new(move |_| flag.load(std::sync::atomic::Ordering::SeqCst)),
        Arc::new(move |_, _| env_with(&bin)),
        QUICK,
    );
    let root = s.root.clone();
    let started = Instant::now();
    let reading = std::thread::spawn(move || gradle::read(&root, &e));
    while !pid.exists() {
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "the daemon did not start"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    trusted.store(false, std::sync::atomic::Ordering::SeqCst);
    let err = reading.join().unwrap().unwrap_err();
    assert_eq!(err, exec::untrusted_reason("gradlew"));
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "{:?}",
        started.elapsed()
    );
    assert_gone(&pid);
}

/// 수만 개 · 이상한 이름을 내는 gradle: 캐시에는 꼴을 지난 이름순 500 개만 쓰고, 다른 감지기(maven)의 기록은 남는다.
#[tokio::test(flavor = "multi_thread")]
async fn a_huge_task_list_is_trimmed_before_it_is_cached() {
    let h = host(true, |s| {
        let mut out = String::from("Other tasks\n-----------\n");
        for i in 0..50_000 {
            out.push_str(&format!("task{i:05}{} - x\n", "y".repeat(150)));
        }
        out.push_str("a;rm - x\n-flag - x\n");
        let file = s.base.join("huge.out");
        fs::write(&file, out).unwrap();
        gradle_project(s, &format!("cat '{}'\n", file.display()));
    });
    cache::store(&h.cache_dir, "maven", "sha256:k", &["package".to_owned()]).unwrap();
    let refreshed = call(
        &h,
        ConnectionScope::Project(h.project.clone()),
        "run.tasks.refresh",
        refresh_params(&h, "gradle"),
    )
    .await
    .unwrap();
    assert_eq!(refreshed["error"], Value::Null, "{refreshed:#}");
    assert_eq!(refreshed["detected"].as_array().unwrap().len(), MAX_TASKS);
    let text = fs::read_to_string(h.cache_dir.join(cache::FILE)).unwrap();
    assert!(text.len() < 1024 * 1024, "{} bytes", text.len());
    assert!(!text.contains("a;rm") && !text.contains("-flag"));
    // 거르지 않은 목록을 그대로 쓰려 하면 캐시는 쓰기를 거절한다 — 읽는 쪽이 파일째 버려 maven 기록까지 사라지지 않게.
    let huge: Vec<String> = (0..50_000)
        .map(|i| format!("task{i:05}{}", "y".repeat(150)))
        .collect();
    let err = cache::store(&h.cache_dir, "gradle", "sha256:other", &huge).unwrap_err();
    assert!(err.contains("would be larger than"), "{err}");
    assert_eq!(
        cache::lookup(&h.cache_dir, "maven", || Ok("sha256:k".to_owned())).unwrap(),
        cache::Lookup::Hit(vec!["package".to_owned()])
    );
}

/// 손으로 고친 캐시의 이상한 이름(셸 글자 · 제어 문자 · `-` 머리 · 너무 긴 것)은 감지에서 다시 빠진다.
#[test]
fn odd_names_in_a_cache_are_left_out() {
    let s = site();
    gradle_project(&s, "true\n");
    let e = exec(&s, true, QUICK);
    let key = cache::key(&s.root, cache::BuildFiles::Gradle).unwrap();
    let names: Vec<String> = [
        "ok",
        "a;rm -rf x",
        "line\nbreak",
        "-flag",
        &"x".repeat(MAX_NAME_LEN + 1),
    ]
    .iter()
    .map(|n| (*n).to_owned())
    .collect();
    cache::store(e.cache_dir().unwrap(), "gradle", &key, &names).unwrap();
    let detection = detect_with(&only(gradle::ID), &s.root, &e);
    let got: Vec<&str> = detection.tasks.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(got, ["ok"]);
    assert!(
        detection.problems[0].reason.contains("skipped 4 task(s)"),
        "{:?}",
        detection.problems
    );
}

/// mise 가 숨은 태스크(`hide`)나 제어 문자가 든 이름을 내도 목록에 넣지 않는다.
#[test]
fn hidden_and_control_character_mise_tasks_are_left_out() {
    let s = site();
    mise_project(&s);
    let source = s.root.join("mise.toml").display().to_string();
    let listed = json!([
        {"name": "shown", "source": source},
        {"name": "secret", "source": source, "hide": true},
        {"name": "evil\u{1b}[2Jname", "source": source},
    ]);
    let out = s.base.join("out.json");
    fs::write(&out, listed.to_string()).unwrap();
    script(&s.bin.join("mise"), &format!("cat '{}'\n", out.display()));
    let detection = detect_with(&only(mise::ID), &s.root, &exec(&s, true, QUICK));
    let got: Vec<&str> = detection.tasks.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(got, ["shown"]);
    assert!(detection.problems[0].reason.contains("control characters"));
}

/// 깊게 중첩된 XML 은 읽지 않는다(메모리).
#[test]
fn a_deeply_nested_effective_pom_is_refused() {
    let xml = format!("{}{}", "<a>".repeat(10_000), "</a>".repeat(10_000));
    assert!(
        maven::plugin_goals(&xml)
            .unwrap_err()
            .contains("deeper than")
    );
}

/// 그 프로젝트 창이 아닌 연결의 `run.config.list` 는 mise 를 띄우지 않고 캐시 · 정적 결과만 준다. 그 창의 목록은 띄운다.
#[tokio::test(flavor = "multi_thread")]
async fn only_the_projects_own_window_lists_start_mise() {
    let h = host(true, |s| {
        mise_project(s);
        fake_mise(s, "2026.4.3", "trusted");
        gradle_project(s, "true\n");
    });
    let key = cache::key(&h.root, cache::BuildFiles::Gradle).unwrap();
    cache::store(&h.cache_dir, "gradle", &key, &["fromCache".to_owned()]).unwrap();
    let params = json!({"projectId": h.project});
    for scope in [
        ConnectionScope::Host,
        ConnectionScope::Agent,
        ConnectionScope::Project("someone-else".into()),
    ] {
        let list = call(&h, scope.clone(), "run.config.list", params.clone())
            .await
            .unwrap();
        assert_eq!(calls(&h.marker), 0, "{scope:?}");
        assert!(
            list["detectProblems"]
                .as_array()
                .unwrap()
                .iter()
                .any(|p| p["detector"] == "mise" && p["reason"] == exec::NOT_THIS_WINDOW),
            "{list:#}"
        );
        assert_eq!(
            list["taskLists"],
            json!([{"detector": "gradle", "state": "read"}])
        );
    }
    let list = call(
        &h,
        ConnectionScope::Project(h.project.clone()),
        "run.config.list",
        params,
    )
    .await
    .unwrap();
    assert_eq!(calls(&h.marker), 1);
    assert!(
        list["detected"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["key"] == "detected:mise:build")
    );
}

/// 버려진 요청(창이 닫힘)은 읽기 작업을 멈추고 gradle 을 끝낸다.
#[tokio::test(flavor = "multi_thread")]
async fn a_dropped_refresh_request_stops_the_read() {
    let pid_holder = Arc::new(std::sync::Mutex::new(PathBuf::new()));
    let holder = Arc::clone(&pid_holder);
    let h = host(true, move |s| {
        let pid = s.base.join("daemon.pid");
        *holder.lock().unwrap() = pid.clone();
        gradle_project(s, &format!("{}sleep 60\n", escaping_daemon(&pid)));
    });
    let pid = pid_holder.lock().unwrap().clone();
    let configs = Arc::clone(&h.configs);
    let scope = ConnectionScope::Project(h.project.clone());
    let params: crate::detect::refresh::RunTasksRefreshParams =
        serde_json::from_value(refresh_params(&h, "gradle")).unwrap();
    let request = tokio::spawn(async move { configs.refresh(&scope, params).await });
    let deadline = Instant::now() + Duration::from_secs(10);
    while !pid.exists() {
        assert!(Instant::now() < deadline, "gradlew did not start");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    request.abort();
    while !h.tasks.running(Some(&h.project)).is_empty() {
        assert!(
            Instant::now() < deadline,
            "the host task kept running after the request was dropped"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    tokio::task::spawn_blocking(move || assert_gone(&pid))
        .await
        .unwrap();
}

/// 감지 결과의 태스크가 같아도 목록 상태(`notRead` → `read`)만 바뀌면 창에 알린다(지문에 상태가 든다).
#[tokio::test(flavor = "multi_thread")]
async fn a_task_list_state_change_alone_is_announced() {
    let h = host(true, |s| gradle_project(s, "true\n"));
    let bus = EventBus::new(DEFAULT_CAPACITY);
    let mut announced =
        bus.subscribe(pw_core::bus::Filter::all().topics([pw_core::config::TOPIC_CHANGED]));
    tokio::spawn(follow_files(
        subscribe_files(&bus),
        h.configs.seen(),
        bus.clone(),
    ));
    let list = call(
        &h,
        ConnectionScope::Project(h.project.clone()),
        "run.config.list",
        json!({"projectId": h.project}),
    )
    .await
    .unwrap();
    assert_eq!(
        list["taskLists"],
        json!([{"detector": "gradle", "state": "notRead"}])
    );
    // 같은 이름(`build` · `test`)을 읽은 것으로 캐시에 둔다 — 태스크는 같고 상태만 `read` 가 된다.
    let key = cache::key(&h.root, cache::BuildFiles::Gradle).unwrap();
    cache::store(
        &h.cache_dir,
        "gradle",
        &key,
        &["build".to_owned(), "test".to_owned()],
    )
    .unwrap();
    bus.publish(
        Scope::Project(h.project.clone()),
        "fs.changed",
        &json!({"root": h.root.display().to_string(), "worktree": null, "paths": [h.root.join("build.gradle.kts").display().to_string()], "kinds": ["modified"], "rescan": false}),
    )
    .unwrap();
    let received = tokio::time::timeout(Duration::from_secs(10), announced.recv())
        .await
        .expect("a state-only change was not announced")
        .unwrap();
    let pw_core::bus::Received::Event(event) = received else {
        panic!("{received:?}")
    };
    assert_eq!(event.payload["changed"], json!([watch::CHANGED_KEY]));
    let _ = &h.projects;
}

// --- 로컬 live: 실제 gradle 의 1회용 데몬 ------------------------------------------------------------------------------

/// `pgrep -f <pattern>` 의 pid 들.
fn procs(pattern: &str) -> std::collections::BTreeSet<String> {
    let out = std::process::Command::new("pgrep")
        .args(["-f", pattern])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::to_owned)
        .collect()
}

/// `PITWALL_LIVE_GRADLE=<gradle 실행 파일>`(와 `JAVA_HOME`)일 때만: `org.gradle.jvmargs` 가 있어 1회용 데몬(스스로 새 세션)이 뜨고, 빌드
/// 스크립트가 프로세스 하나(`sleep`)를 띄운 뒤 인터럽트를 삼키며 120 초를 돈다(M6-05 리뷰의 실측 꼴). 시한(60 초)에 끝낸 뒤 데몬도 그
/// 자식도 남지 않는다 — 데몬은 그 자식을 남긴 채 스스로 끝나므로, 도는 동안 자손으로 기록해 둔 것을 끝내야 한다.
#[test]
fn live_gradle_leaves_no_single_use_daemon() {
    let Some(gradle) = std::env::var_os("PITWALL_LIVE_GRADLE") else {
        eprintln!(
            "skipped: set PITWALL_LIVE_GRADLE=<path to gradle> (and JAVA_HOME) to run against the real gradle"
        );
        return;
    };
    let s = site();
    fs::write(
        s.root.join("settings.gradle.kts"),
        "rootProject.name = \"live\"\n",
    )
    .unwrap();
    fs::write(
        s.root.join("gradle.properties"),
        "org.gradle.jvmargs=-Xmx383m\n",
    )
    .unwrap();
    fs::write(
        s.root.join("build.gradle.kts"),
        "ProcessBuilder(\"/bin/sleep\", \"61.383\").start()\nval end = System.currentTimeMillis() + 120_000\nwhile (System.currentTimeMillis() < end) { try { Thread.sleep(100) } catch (e: Throwable) {} }\n",
    )
    .unwrap();
    script(
        &s.root.join("gradlew"),
        &format!("exec '{}' \"$@\"\n", Path::new(&gradle).display()),
    );
    // 데몬(이 시험만 쓰는 `-Xmx383m`)과 빌드 스크립트가 띄운 것(`sleep 61.383`). 데몬은 클라이언트가 죽으면 스스로 끝나지만(Gradle
    // 9.5 실측 — 8 초 안) 그 자식은 init 으로 넘어가 남는다.
    let daemons = || -> std::collections::BTreeSet<String> {
        let mut all = procs("Xmx383m");
        all.extend(procs("sleep 61[.]383"));
        all
    };
    assert!(
        daemons().is_empty(),
        "a live daemon is already running: {:?}",
        daemons()
            .iter()
            .map(|p| String::from_utf8_lossy(
                &std::process::Command::new("ps")
                    .args(["-o", "args=", "-p", p])
                    .output()
                    .unwrap()
                    .stdout
            )
            .into_owned())
            .collect::<Vec<_>>()
    );
    let mut vars = std::collections::BTreeMap::new();
    vars.insert(
        "GRADLE_USER_HOME".into(),
        s.base.join("guh").into_os_string(),
    );
    if let Some(java) = std::env::var_os("JAVA_HOME") {
        vars.insert("JAVA_HOME".into(), java);
    }
    vars.insert("HOME".into(), s.base.join("home").into_os_string());
    let env = RuntimeEnv {
        path: "/usr/bin:/bin".into(),
        vars,
        ..RuntimeEnv::default()
    };
    let limits = ExecLimits {
        timeout: Duration::from_secs(60),
        max_output: 8 * 1024 * 1024,
    };
    let e = TrustedExec::new(
        "p",
        Arc::new(|_| true),
        Arc::new(move |_, _| env.clone()),
        limits,
    );
    let root = s.root.clone();
    let watcher = std::thread::spawn(move || {
        let (mut daemon, mut child) = (false, false);
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(58) {
            daemon |= !procs("Xmx383m").is_empty();
            child |= !procs("sleep 61[.]383").is_empty();
            std::thread::sleep(Duration::from_millis(200));
        }
        (daemon, child)
    });
    let err = gradle::read(&root, &e).unwrap_err();
    let (daemon, child) = watcher.join().unwrap();
    assert!(err.contains("did not finish within"), "{err}");
    assert!(
        daemon,
        "no single-use daemon appeared (did org.gradle.jvmargs take effect?)"
    );
    assert!(
        child,
        "the build script did not start its process within the time limit"
    );
    let deadline = Instant::now() + Duration::from_secs(12);
    while !daemons().is_empty() {
        assert!(Instant::now() < deadline, "left behind: {:?}", daemons());
        std::thread::sleep(Duration::from_millis(100));
    }
}
