//! 감사 로그의 보관(16-m5-plan M5-11 ③ — 결정 M5-D10): 보관 일수는 **전역 설정의 `[mcp.audit] retention_days` 만** 읽는다.
//! 프로젝트 설정(공유 `.pitwall/project.toml` · 개인 `.pitwall/project.local.toml`)의 그 표는 효과가 없고 경고가 남는다 —
//! 저장소를 클론해 연 것만으로 감사 기록이 지워지면 안 된다. 설정은 진짜 파일에서 읽는다(Host 의 설정 · 레지스트리).

use std::path::Path;
use std::sync::Arc;

use pw_core::LocalHost;
use pw_core::config::AppPaths;
use pw_core::registry::{NewProject, Project};
use pw_mcp::audit::{self, AuditLog, Entry, Outcome};
use pw_mcp::exposure::{self, Exposure};

const DAY_MS: i64 = 24 * 60 * 60 * 1000;

struct Fixture {
    _tmp: tempfile::TempDir,
    paths: AppPaths,
    host: Arc<LocalHost>,
}

impl Fixture {
    /// `global` 은 `settings.toml` 의 `schema = 1` 뒤에 붙는다.
    fn new(global: &str) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let base = pw_core::registry::path::simplify(std::fs::canonicalize(tmp.path()).unwrap());
        let paths = AppPaths::under(&base.join("app"));
        std::fs::create_dir_all(&paths.config).unwrap();
        std::fs::write(
            paths.config.join("settings.toml"),
            format!("schema = 1\n{global}"),
        )
        .unwrap();
        let host = Arc::new(LocalHost::builder("local").config(paths.clone()).build());
        Self {
            _tmp: tmp,
            paths,
            host,
        }
    }

    /// `.pitwall/` 에 `files`(이름, 내용)를 둔 저장소 모드 프로젝트.
    fn project(&self, name: &str, files: &[(&str, &str)]) -> Project {
        let root = self.paths.config.parent().unwrap().join("work").join(name);
        std::fs::create_dir_all(root.join(".pitwall")).unwrap();
        for (file, text) in files {
            std::fs::write(root.join(".pitwall").join(file), text).unwrap();
        }
        self.host
            .registry()
            .unwrap()
            .create(NewProject {
                root,
                ..NewProject::default()
            })
            .unwrap()
    }

    fn global(&self) -> pw_core::config::ConfigStore {
        self.host.config().unwrap().global.clone()
    }

    fn log(&self) -> AuditLog {
        AuditLog::under_state(&self.paths.state)
    }

    /// `days` 일 전의 행.
    fn row(&self, project: Option<&str>, days: i64) -> i64 {
        let log = self.log();
        let id = log
            .record(&Entry {
                session: "s".into(),
                client: None,
                project_id: project.map(str::to_owned),
                tool: "project_info".into(),
                args: "{}".into(),
                outcome: Outcome::Ok,
                detail: None,
                result_size: None,
                duration: std::time::Duration::ZERO,
                approval: None,
                plan_hash: None,
                transport: None,
            })
            .unwrap();
        rusqlite::Connection::open(log.file())
            .unwrap()
            .execute(
                "UPDATE calls SET at = at - ?1 WHERE id = ?2",
                (days * DAY_MS, id),
            )
            .unwrap();
        id
    }

    fn ids(&self) -> Vec<i64> {
        let (mut rows, _) = self
            .log()
            .list(&audit::Query {
                limit: 1000,
                ..Default::default()
            })
            .unwrap();
        rows.reverse();
        rows.into_iter().map(|r| r.id).collect()
    }

    /// 프로젝트 `[mcp]` 를 읽는 자리(세션 · 대상 프로젝트 · `mcp.tools.list` 가 쓰는 `exposure`)의 경고. 프로젝트 설정은 진짜
    /// 파일에서 디스크에 쓰지 않고 읽는다(`exposure::of_project` 와 같은 읽기).
    fn project_warnings(&self, project: &Project) -> Vec<String> {
        let snapshot = project
            .config_source(&self.paths, project.root())
            .unwrap()
            .without_writes()
            .load()
            .unwrap();
        let global = self.global().snapshot();
        let (_, warnings) = Exposure::from_layers(
            global.get("settings", exposure::KEY),
            snapshot.get("project", exposure::KEY),
        );
        warnings
    }
}

const SHORT: &str = "schema = 1\n[mcp.audit]\nretention_days = 1\n";

fn has_audit_warning(warnings: &[String]) -> bool {
    warnings
        .iter()
        .any(|w| w.contains("mcp.audit") && w.contains("no effect"))
}

/// 전역의 30일이 먹고, 공유 `project.toml` · 개인 `.local.toml` 의 1일은 먹지 않는다 — 2일 전 행이 남는다. 프로젝트 쪽 표에는
/// 경고가 남는다.
#[test]
fn project_settings_do_not_shorten_the_retention_and_leave_a_warning() {
    let f = Fixture::new("[mcp.audit]\nretention_days = 30\n");
    let shared = f.project("shared", &[("project.toml", SHORT)]);
    let local = f.project(
        "local",
        &[
            ("project.toml", "schema = 1\n"),
            ("project.local.toml", SHORT),
        ],
    );
    let plain = f.project("plain", &[("project.toml", "schema = 1\n")]);
    let recent_shared = f.row(Some(&shared.id), 2);
    let recent_local = f.row(Some(&local.id), 2);
    let recent_none = f.row(None, 2);
    let old = f.row(Some(&shared.id), 31);
    assert_eq!(audit::prune_by_settings(&f.log(), &f.global()), 1);
    assert_eq!(f.ids(), [recent_shared, recent_local, recent_none]);
    assert!(!f.ids().contains(&old));

    assert!(has_audit_warning(&f.project_warnings(&shared)), "공유 설정");
    assert!(has_audit_warning(&f.project_warnings(&local)), "개인 설정");
    assert!(
        !has_audit_warning(&f.project_warnings(&plain)),
        "표가 없으면 경고도 없다"
    );
    // 전역 설정의 표는 경고가 아니다.
    let (_, global_only) =
        Exposure::from_layers(f.global().snapshot().get("settings", exposure::KEY), None);
    assert!(!has_audit_warning(&global_only), "{global_only:?}");
}

/// `retention_days = 0` 이면 아무리 오래된 행도 지우지 않는다.
#[test]
fn zero_retention_keeps_everything() {
    let f = Fixture::new("[mcp.audit]\nretention_days = 0\n");
    let ancient = f.row(None, 5000);
    let recent = f.row(None, 1);
    assert_eq!(audit::prune_by_settings(&f.log(), &f.global()), 0);
    assert_eq!(f.ids(), [ancient, recent]);
}

/// 표가 없으면 90일이다.
#[test]
fn the_default_retention_is_ninety_days() {
    let f = Fixture::new("");
    let kept = f.row(None, 89);
    f.row(None, 91);
    assert_eq!(audit::prune_by_settings(&f.log(), &f.global()), 1);
    assert_eq!(f.ids(), [kept]);
}

/// 감사 파일이 없으면 정리가 파일을 만들지 않는다.
#[test]
fn pruning_without_a_file_creates_nothing() {
    let f = Fixture::new("");
    assert_eq!(audit::prune_by_settings(&f.log(), &f.global()), 0);
    assert!(!Path::new(f.log().file()).exists());
}

/// 보관 정리 태스크(`keep_pruned` — 앱이 시작할 때 띄운다)는 띄우자마자 한 번 지운다 — 하루를 기다리지 않는다.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_prune_task_prunes_as_soon_as_it_starts() {
    let f = Fixture::new("");
    let kept = f.row(None, 1);
    f.row(None, 91);
    let cancel = tokio_util::sync::CancellationToken::new();
    let task = tokio::spawn(audit::keep_pruned(
        Arc::new(f.log()),
        f.global(),
        cancel.clone(),
    ));
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    while f.ids() != [kept] {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the start-up prune did not run: {:?}",
            f.ids()
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    cancel.cancel();
    tokio::time::timeout(std::time::Duration::from_secs(5), task)
        .await
        .expect("the prune task did not stop when cancelled")
        .unwrap();
}
