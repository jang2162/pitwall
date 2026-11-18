//! 실행 툴 ① 조회 시험(17-m6-plan M6-12 완료 기준 ② ③ ④ ⑥ 과 관문 · 가림 · 띄운 자리). 실행은 프로세스 없이 시험용 종류
//! ([`Scripted`])로 만들고 출력을 직접 쓴다. 목록 · 등급 · 헤드리스(① ⑤)는 데스크톱의 `mcp_tests::run_read` 가 본다.

use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use pw_core::bus::{DEFAULT_CAPACITY, EventBus};
use pw_core::config::AppPaths;
use pw_core::registry::{NewProject, Registry};
use pw_rpc::{ConnectionScope, RpcError, code};
use pw_runtime::{LoginShell, Runtime};

use super::*;
use crate::runs::{
    HostParts, NewRun, ProcessLauncher, ProcessSpec, Relaunch, RunKind, RunLimits, RunOutput,
};

/// 띄우지 않는다(이 시험은 [`Runs::start_kind`] 로만 실행을 만든다).
struct NoLaunch;

impl ProcessLauncher for NoLaunch {
    fn launch(&self, _: &ProcessSpec, _: RunOutput) -> Result<Arc<dyn RunKind>, RpcError> {
        Err(RpcError::internal("not in this test"))
    }
}

/// 출력을 시험이 쓰는 종류.
struct Scripted {
    output: Mutex<Option<RunOutput>>,
}

impl RunKind for Scripted {
    fn kind(&self) -> &'static str {
        "process"
    }

    fn stop(&self) {
        if let Some(output) = self.output.lock().unwrap().as_ref() {
            output.exited(RunExit {
                code: None,
                signal: Some("test".into()),
            });
        }
    }
}

struct Fixture {
    _tmp: tempfile::TempDir,
    runs: Runs,
    project: String,
    other: String,
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        Self::with_limits(RunLimits::default())
    }

    fn with_limits(limits: RunLimits) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::under(&tmp.path().join("app"));
        let bus = EventBus::new(DEFAULT_CAPACITY);
        let projects = Registry::new(paths.clone(), None, bus.clone());
        let runtime = Arc::new(Runtime::new(LoginShell::new(None, Duration::from_secs(1))));
        let runs = Runs::with(runtime, Arc::new(NoLaunch), limits);
        runs.attach(HostParts {
            paths,
            projects: projects.clone(),
            bus,
        });
        let make = |name: &str| {
            let root = tmp.path().join(name);
            fs::create_dir_all(&root).unwrap();
            let id = projects
                .create(NewProject {
                    root: root.clone(),
                    ..NewProject::default()
                })
                .unwrap()
                .id;
            projects.set_trust(&id, true).unwrap();
            (id, pw_core::registry::path::normalize(&root).unwrap())
        };
        let (project, root) = make("project");
        let (other, _) = make("other");
        Self {
            _tmp: tmp,
            runs,
            project,
            other,
            root,
        }
    }

    /// 실행 하나를 만들고 출력 손잡이를 돌려준다.
    fn run(&self, project: &str, launched_by: LaunchedBy) -> (RunId, RunOutput) {
        let slot: Arc<Mutex<Option<RunOutput>>> = Arc::default();
        let keep = Arc::clone(&slot);
        let info = self
            .runs
            .start_kind(
                NewRun {
                    project_id: project.to_owned(),
                    config: "dev".into(),
                    name: "Dev".into(),
                    launched_by,
                    worktree: None,
                    cwd: None,
                    profile: None,
                    relaunch: Relaunch::None,
                    size: (80, 24),
                    restart_of: None,
                },
                move |output| {
                    *keep.lock().unwrap() = Some(output.clone());
                    Ok(Arc::new(Scripted {
                        output: Mutex::new(Some(output)),
                    }))
                },
            )
            .unwrap();
        let output = slot.lock().unwrap().clone().unwrap();
        (info.run_id, output)
    }

    fn person_run(&self) -> (RunId, RunOutput) {
        self.run(
            &self.project,
            LaunchedBy::Window {
                window: Some("main".into()),
            },
        )
    }

    fn logs(&self, params: RunToolLogsParams) -> Result<RunToolLogs, RpcError> {
        self.runs.tool_logs(&ConnectionScope::Agent, &params)
    }

    fn params(&self, run_id: &str) -> RunToolLogsParams {
        RunToolLogsParams {
            project_id: self.project.clone(),
            run_id: run_id.to_owned(),
            ..RunToolLogsParams::default()
        }
    }
}

fn numbered(n: usize) -> Vec<u8> {
    (0..n)
        .map(|i| format!("line {i}\n"))
        .collect::<String>()
        .into_bytes()
}

/// 기본은 마지막 200줄, `tail` 이 수를 정한다. `nextSince` 는 끝이고 그 뒤 새 출력만 `since` 로 읽힌다.
#[test]
fn the_default_is_the_last_two_hundred_lines_and_since_reads_on() {
    let f = Fixture::new();
    let (run, output) = f.person_run();
    output.write(&numbered(500));
    let logs = f.logs(f.params(&run)).unwrap();
    assert_eq!(logs.lines, 200);
    assert!(logs.text.starts_with("line 300\n"), "{}", &logs.text[..20]);
    assert!(logs.text.ends_with("line 499"));
    assert!(!logs.truncated);
    assert_eq!(logs.next_since, logs.log_end);

    let three = f
        .logs(RunToolLogsParams {
            tail: Some(3),
            ..f.params(&run)
        })
        .unwrap();
    assert_eq!(three.text, "line 497\nline 498\nline 499");

    output.write(b"new 1\nnew 2\n");
    let more = f
        .logs(RunToolLogsParams {
            since: Some(logs.next_since),
            ..f.params(&run)
        })
        .unwrap();
    assert_eq!(more.text, "new 1\nnew 2");
    assert_eq!(more.from, logs.next_since);
    assert!(!more.truncated);
    assert_eq!(more.dropped, 0);
}

/// 완료 기준 ②: `since` 가 링 버퍼에서 버려진 오프셋이면 `truncated` 와 남은 처음부터 — 조용히 비우지 않는다.
#[test]
fn a_since_that_was_dropped_is_truncated_from_the_first_kept_byte() {
    let f = Fixture::with_limits(RunLimits {
        running: 4096,
        ..RunLimits::default()
    });
    let (run, output) = f.person_run();
    output.write(&numbered(2000));
    let logs = f
        .logs(RunToolLogsParams {
            since: Some(0),
            ..f.params(&run)
        })
        .unwrap();
    assert!(logs.log_start > 0, "the ring dropped its front");
    assert!(logs.truncated);
    assert_eq!(logs.dropped, logs.log_start);
    assert_eq!(logs.from, logs.log_start);
    assert!(logs.lines > 0, "the kept output is returned, not nothing");
    assert!(logs.text.ends_with("line 1999"));
}

/// 완료 기준 ③: 응답은 256 KiB 를 넘지 않는다 — `tail` 은 앞(오래된 줄)을 덜고, `since` 는 뒤를 덜고 다음 `since` 로 잇는다.
#[test]
fn the_response_stays_under_the_session_limit() {
    let f = Fixture::new();
    let (run, output) = f.person_run();
    let long = "x".repeat(4000);
    let mut bytes = Vec::new();
    for i in 0..1000 {
        bytes.extend(format!("{i:04} {long}\n").into_bytes());
    }
    output.write(&bytes);

    let tail = f
        .logs(RunToolLogsParams {
            tail: Some(1000),
            ..f.params(&run)
        })
        .unwrap();
    let size = serde_json::to_vec(&tail).unwrap().len();
    assert!(size <= 256 * 1024, "{size}");
    assert!(tail.truncated);
    assert!(tail.lines < 1000);
    assert!(
        tail.text.ends_with(&format!("0999 {long}")),
        "the latest lines are kept"
    );

    let mut since = 0;
    let mut seen = 0;
    loop {
        let page = f
            .logs(RunToolLogsParams {
                since: Some(since),
                ..f.params(&run)
            })
            .unwrap();
        let size = serde_json::to_vec(&page).unwrap().len();
        assert!(size <= 256 * 1024, "{size}");
        for line in page.text.lines() {
            assert_eq!(
                &line[..4],
                format!("{seen:04}"),
                "pages neither skip nor repeat lines"
            );
            seen += 1;
        }
        if page.next_since == page.log_end {
            assert!(!page.truncated);
            break;
        }
        assert!(page.truncated);
        since = page.next_since;
    }
    assert_eq!(seen, 1000);
}

/// 한 줄이 상한보다 크면 그 줄을 잘라 싣고 다음 줄로 넘어간다 — 같은 자리를 다시 읽지 않는다.
#[test]
fn a_line_larger_than_the_budget_is_cut_and_paging_moves_on() {
    let f = Fixture::new();
    let (run, output) = f.person_run();
    output.write(format!("{}\nafter\n", "y".repeat(400 * 1024)).as_bytes());
    let first = f
        .logs(RunToolLogsParams {
            since: Some(0),
            ..f.params(&run)
        })
        .unwrap();
    assert!(serde_json::to_vec(&first).unwrap().len() <= 256 * 1024);
    assert!(first.truncated);
    assert_eq!(first.lines, 1);
    let second = f
        .logs(RunToolLogsParams {
            since: Some(first.next_since),
            ..f.params(&run)
        })
        .unwrap();
    assert_eq!(second.text, "after");
}

/// 완료 기준 ④: ANSI 이스케이프 · 제어 문자를 걷는다. `\r` 은 줄 앞으로 돌아간다.
#[test]
fn ansi_escapes_are_removed() {
    let f = Fixture::new();
    let (run, output) = f.person_run();
    output.write(
        b"\x1b[1;32mready\x1b[0m in \x1b[33m120\x1b[39m ms\r\n\
          \x1b]0;title\x07\x1b]8;;http://x\x1b\\link\x1b]8;;\x1b\\\n\
          10%\r50%\r100% \x1b[K\n\
          \x1b(Bplain\x07\x00\ttab\n",
    );
    let logs = f.logs(f.params(&run)).unwrap();
    assert_eq!(logs.text, "ready in 120 ms\nlink\n100% \nplain\ttab");
    assert!(!logs.text.contains('\u{1b}'));
}

#[test]
fn clean_line_removes_c1_and_partial_sequences() {
    assert_eq!(clean_line(b"\xc2\x9b31mred"), "red");
    assert_eq!(clean_line(b"cut \x1b["), "cut ");
    assert_eq!(clean_line(b"bad \xff byte"), "bad \u{fffd} byte");
}

/// `grep` 은 걷은 글에 맞춘 정규식이다. `tail` 은 맞은 줄의 마지막 몇 줄이다. 틀린 정규식 · 너무 긴 글은 invalid params.
#[test]
fn grep_filters_lines_by_a_regular_expression() {
    let f = Fixture::new();
    let (run, output) = f.person_run();
    output.write(b"INFO a\n\x1b[31mERROR\x1b[0m b\nINFO c\nERROR d\nERROR e\n");
    let logs = f
        .logs(RunToolLogsParams {
            grep: Some("^ERROR".into()),
            tail: Some(2),
            ..f.params(&run)
        })
        .unwrap();
    assert_eq!(logs.text, "ERROR d\nERROR e");
    let all = f
        .logs(RunToolLogsParams {
            grep: Some("^ERROR".into()),
            since: Some(0),
            ..f.params(&run)
        })
        .unwrap();
    assert_eq!(all.text, "ERROR b\nERROR d\nERROR e");

    for bad in [
        "(".to_owned(),
        "a".repeat(GREP_MAX_CHARS + 1),
        "x{1000}{1000}".to_owned(),
    ] {
        let e = f
            .logs(RunToolLogsParams {
                grep: Some(bad),
                ..f.params(&run)
            })
            .unwrap_err();
        assert_eq!(e.code, code::INVALID_PARAMS, "{e:?}");
    }
    let e = f
        .logs(RunToolLogsParams {
            tail: Some(MAX_TAIL + 1),
            ..f.params(&run)
        })
        .unwrap_err();
    assert_eq!(e.code, code::INVALID_PARAMS);
}

/// 완료 기준 ⑥ · ④: 다른 프로젝트의 `runId` 는 없는 실행과 같은 오류다(있다는 것을 알리지 않는다).
#[test]
fn another_projects_run_id_is_not_found() {
    let f = Fixture::new();
    let (theirs, output) = f.run(&f.other, LaunchedBy::Window { window: None });
    output.write(b"their secret output\n");
    let call = |run_id: &str| {
        f.logs(RunToolLogsParams {
            project_id: f.project.clone(),
            run_id: run_id.to_owned(),
            ..RunToolLogsParams::default()
        })
        .unwrap_err()
    };
    let other = call(&theirs);
    let missing = call("nope-1");
    assert_eq!(other.code, code::NOT_FOUND);
    assert_eq!(other.code, missing.code);
    assert_eq!(
        other.message.replace(&theirs, "<id>"),
        missing.message.replace("nope-1", "<id>")
    );
    let status = f
        .runs
        .tool_status(
            &ConnectionScope::Agent,
            &RunToolStatusParams {
                project_id: f.project.clone(),
                run_id: Some(theirs.clone()),
            },
        )
        .unwrap_err();
    assert_eq!(status.code, code::NOT_FOUND);
    // 그 프로젝트로 부르면 읽힌다.
    let own = f
        .logs(RunToolLogsParams {
            project_id: f.other.clone(),
            run_id: theirs,
            ..RunToolLogsParams::default()
        })
        .unwrap();
    assert_eq!(own.text, "their secret output");
}

/// 툴은 MCP 세션의 연결만 받는다(`agent_grant`) — 창 · Host 범위 연결은 permission denied, 없는 프로젝트는 invalid params.
#[test]
fn only_an_mcp_session_may_call_them() {
    let f = Fixture::new();
    let (run, _) = f.person_run();
    for scope in [
        ConnectionScope::Host,
        ConnectionScope::Project(f.project.clone()),
    ] {
        let e = f.runs.tool_logs(&scope, &f.params(&run)).unwrap_err();
        assert_eq!(e.code, code::PERMISSION_DENIED, "{scope:?}");
        let e = f
            .runs
            .tool_status(
                &scope,
                &RunToolStatusParams {
                    project_id: f.project.clone(),
                    run_id: None,
                },
            )
            .unwrap_err();
        assert_eq!(e.code, code::PERMISSION_DENIED, "{scope:?}");
        let e = f
            .runs
            .tool_config_list(
                &scope,
                &RunToolConfigListParams {
                    project_id: f.project.clone(),
                    worktree: None,
                },
            )
            .unwrap_err();
        assert_eq!(e.code, code::PERMISSION_DENIED, "{scope:?}");
    }
    let e = f
        .runs
        .tool_status(
            &ConnectionScope::Agent,
            &RunToolStatusParams {
                project_id: "0000000000000000".into(),
                run_id: None,
            },
        )
        .unwrap_err();
    assert_eq!(e.code, code::INVALID_PARAMS);
}

/// `run_status`: 그 프로젝트의 실행 중 · 끝난 실행, 끝 코드, 사람 · 에이전트가 띄웠는지 — 에이전트 세션 id · 클라이언트 이름은 싣지
/// 않는다(다른 세션의 정보 — M5-10).
#[test]
fn status_lists_this_projects_runs_and_who_started_them() {
    let f = Fixture::new();
    let (person, output) = f.person_run();
    output.write(b"bye\n");
    output.exited(RunExit {
        code: Some(3),
        signal: None,
    });
    let (agent, _) = f.run(
        &f.project,
        LaunchedBy::Agent {
            session: "session-secret-1".into(),
            client: Some("client-name-7".into()),
        },
    );
    let _ = f.run(&f.other, LaunchedBy::Window { window: None });
    let status = f
        .runs
        .tool_status(
            &ConnectionScope::Agent,
            &RunToolStatusParams {
                project_id: f.project.clone(),
                run_id: None,
            },
        )
        .unwrap();
    let ids: Vec<&str> = status.runs.iter().map(|r| r.run_id.as_str()).collect();
    assert_eq!(ids, [person.as_str(), agent.as_str()]);
    assert_eq!(status.runs[0].launched_by, RunToolLauncher::Person);
    assert_eq!(status.runs[0].state, RunToolState::Exited);
    assert_eq!(status.runs[0].exit.as_ref().unwrap().code, Some(3));
    assert_eq!(status.runs[1].launched_by, RunToolLauncher::Agent);
    assert_eq!(status.runs[1].state, RunToolState::Running);
    let json = serde_json::to_string(&status).unwrap();
    assert!(!json.contains("session-secret-1"), "{json}");
    assert!(!json.contains("client-name-7"), "{json}");
    assert!(
        !json.contains("main"),
        "the window label is not shown: {json}"
    );

    let one = f
        .runs
        .tool_status(
            &ConnectionScope::Agent,
            &RunToolStatusParams {
                project_id: f.project.clone(),
                run_id: Some(agent.clone()),
            },
        )
        .unwrap();
    assert_eq!(one.runs.len(), 1);
    assert_eq!(one.runs[0].run_id, agent);
}

/// `run_config_list`: 구성 · 쓸 수 없는 구성 · 감지된 태스크(출처). 비밀 이름의 env 값은 가린다.
#[test]
fn config_list_shows_configs_unusable_files_and_detected_tasks() {
    let f = Fixture::new();
    let run = f.root.join(".pitwall/run");
    fs::create_dir_all(&run).unwrap();
    fs::write(
        run.join("api.toml"),
        "schema = 1\nname = \"API\"\ntype = \"shell\"\ncommand = \"echo hi\"\n\
         [env]\nPORT = \"3000\"\nAPI_TOKEN = \"tok-value-123\"\n\
         [profiles.prod.env]\nDB_PASSWORD = \"pw-value-456\"\nMODE = \"prod\"\n",
    )
    .unwrap();
    fs::write(
        run.join("broken.toml"),
        "schema = 1\nname = \"B\"\ntype = \n",
    )
    .unwrap();
    fs::write(
        f.root.join("package.json"),
        r#"{"name":"x","scripts":{"dev":"vite"}}"#,
    )
    .unwrap();
    let list = f
        .runs
        .tool_config_list(
            &ConnectionScope::Agent,
            &RunToolConfigListParams {
                project_id: f.project.clone(),
                worktree: None,
            },
        )
        .unwrap();
    assert_eq!(list.configs.len(), 1, "{list:#?}");
    let api = &list.configs[0];
    assert_eq!(api.id, "api");
    assert_eq!(api.config.env["PORT"], "3000");
    assert_eq!(api.config.env["API_TOKEN"], REDACTED);
    assert_eq!(api.config.profiles["prod"].env["DB_PASSWORD"], REDACTED);
    assert_eq!(api.config.profiles["prod"].env["MODE"], "prod");
    assert_eq!(api.files[0].kind, RunToolConfigFileKind::Shared);
    assert_eq!(list.unusable.len(), 1);
    assert_eq!(list.unusable[0].id, "broken");
    assert!(
        list.detected
            .iter()
            .any(|t| t.key == "detected:npm:dev" && t.source.ends_with("package.json")),
        "{list:#?}"
    );
    let json = serde_json::to_string(&list).unwrap();
    assert!(
        !json.contains("tok-value-123") && !json.contains("pw-value-456"),
        "{json}"
    );
}

/// 리뷰 F1: 이름에 비밀 단어가 없는 env 값 안의 URL 비밀번호도 가린다(`DATABASE_URL`) — 구성 · 프로필 · `[debug]` 아래 `env`.
/// 비밀 이름이 아닌 다른 값 · 명령 · 인자는 그대로다. `GIT_CONFIG_KEY_<n>` 은 비밀 이름이 아니다(`shown::is_secret_name`).
#[test]
fn config_list_masks_url_passwords_in_env_values() {
    let f = Fixture::new();
    let run = f.root.join(".pitwall/run");
    fs::create_dir_all(&run).unwrap();
    fs::write(
        run.join("db.toml"),
        "schema = 1\nname = \"DB\"\ntype = \"shell\"\ncommand = \"psql postgres://cmd:cmd-pass-1@h/db\"\n\
         [env]\nDATABASE_URL = \"postgres://app:url-pass-789@db.local:5432/app\"\nGIT_CONFIG_KEY_0 = \"core.pager\"\n\
         HOME_URL = \"https://example.com/x\"\n\
         [profiles.ci.env]\nCACHE = \"redis://:prof-pass-321@cache:6379\"\n\
         [debug]\nadapter = \"js-debug\"\n[debug.env]\nREMOTE = \"amqp://u:dbg-pass-654@mq\"\n\
         [debug.target.env]\nDEEP = \"x://u:deep-pass-111@h\"\n",
    )
    .unwrap();
    let list = f
        .runs
        .tool_config_list(
            &ConnectionScope::Agent,
            &RunToolConfigListParams {
                project_id: f.project.clone(),
                worktree: None,
            },
        )
        .unwrap();
    assert_eq!(list.configs.len(), 1, "{list:#?}");
    let db = &list.configs[0].config;
    assert!(
        db.env["DATABASE_URL"].starts_with("postgres://app:")
            && db.env["DATABASE_URL"].ends_with("@db.local:5432/app"),
        "{}",
        db.env["DATABASE_URL"]
    );
    assert_eq!(db.env["GIT_CONFIG_KEY_0"], "core.pager");
    assert_eq!(db.env["HOME_URL"], "https://example.com/x");
    let json = serde_json::to_string(&list).unwrap();
    for secret in [
        "url-pass-789",
        "prof-pass-321",
        "dbg-pass-654",
        "deep-pass-111",
    ] {
        assert!(!json.contains(secret), "{secret} leaked: {json}");
    }
    assert!(json.contains("cmd-pass-1"), "commands are shown as written");
}

/// 리뷰 R-a: `tail` 모드에서 마지막 줄 하나가 상한보다 크면 빈 글이 아니라 그 줄의 끝(최신)을 잘라 싣는다.
#[test]
fn tail_keeps_the_end_of_a_last_line_larger_than_the_budget() {
    let f = Fixture::new();
    let (run, output) = f.person_run();
    output.write(format!("before\n{}END\n", "z".repeat(400 * 1024)).as_bytes());
    let logs = f.logs(f.params(&run)).unwrap();
    assert!(serde_json::to_vec(&logs).unwrap().len() <= 256 * 1024);
    assert!(logs.truncated);
    assert_eq!(logs.lines, 1);
    assert!(
        logs.text.ends_with("zzzEND"),
        "the newest end of the line is kept"
    );
    assert!(logs.text.len() > 100 * 1024, "{}", logs.text.len());
    assert_eq!(logs.next_since, logs.log_end);
}
