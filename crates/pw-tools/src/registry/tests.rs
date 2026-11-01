//! 가져오기 · 캐시 · 물러남과 `tools.registry.*` 의 답(가짜 HTTP 서버).

use std::sync::Mutex as StdMutex;
use std::time::{Duration, SystemTime};

use serde_json::{Value, json};

use super::*;
use crate::fetch::sha256_hex;
use crate::methods::{ToolsRegistrySearchParams, get_in, search_in, status_of};
use crate::pinned::PinnedRegistry;
use crate::target::ToolTarget;
use crate::test_http::{FakeServer, Reply, zip_of};

const PINNED: &str = "2026-11-01-pinned-one";
const OLD: &str = "2026-10-01-old-one";

/// 패키지 정의 몇 개로 된 registry.json(`marker` 패키지 이름으로 판을 가린다).
fn registry_json(marker: &str) -> Vec<u8> {
    let snapshot = Index::snapshot();
    let ra = Value::Object(snapshot.get("rust-analyzer").unwrap().raw().clone());
    serde_json::to_vec(&json!([
        ra,
        {"name": marker, "description": "marker package", "homepage": "", "licenses": ["MIT"],
         "languages": ["Go"], "categories": ["Formatter"],
         "source": {"id": format!("pkg:npm/{marker}@1.0.0")}, "bin": {marker: format!("npm:{marker}")}},
    ]))
    .unwrap()
}

struct Fixture {
    server: FakeServer,
    cache: tempfile::TempDir,
    clock: Arc<StdMutex<SystemTime>>,
}

impl Fixture {
    async fn new() -> Fixture {
        Fixture {
            server: FakeServer::start().await,
            cache: tempfile::tempdir().unwrap(),
            clock: Arc::new(StdMutex::new(
                SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000),
            )),
        }
    }

    fn advance(&self, by: Duration) {
        *self.clock.lock().unwrap() += by;
    }

    /// 고정 판을 `path` 의 zip 으로 가리키는 Host. `zip_sha` 가 없으면 실제 zip 의 해시.
    fn registry(
        &self,
        mode: RegistryMode,
        zip: &[u8],
        json: &[u8],
        zip_sha: Option<&str>,
    ) -> Arc<ToolsRegistry> {
        self.server
            .route("/pinned/registry.json.zip", Reply::Body(zip.to_vec()));
        let pinned = Pinned {
            registry: PinnedRegistry {
                version: PINNED.into(),
                url: self.server.url("/pinned/registry.json.zip"),
                sha256: zip_sha.map_or_else(|| sha256_hex(zip), str::to_owned),
                json_sha256: sha256_hex(json),
            },
            bundled: Pinned::builtin().bundled,
            artifacts: Vec::new(),
        };
        let clock = Arc::clone(&self.clock);
        let r = ToolsRegistry::with_parts(pinned, Overrides::default(), Index::snapshot())
            .with_test_network(
                Endpoints {
                    latest_api: self.server.url("/api/latest"),
                    release_base: self.server.url("/download"),
                },
                Arc::new(move || *clock.lock().unwrap()),
            );
        r.attach_fixed(self.cache.path().to_path_buf(), mode);
        Arc::new(r)
    }

    fn cache_dir(&self, version: &str) -> std::path::PathBuf {
        self.cache.path().join("registry").join(version)
    }

    /// 이전에 받아 둔 판 하나(온전한 캐시).
    fn seed_cache(&self, version: &str, marker: &str, fetched_at_ms: u64) {
        let json = registry_json(marker);
        let dir = self.cache_dir(version);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("registry.json"), &json).unwrap();
        std::fs::write(
            dir.join("meta.json"),
            serde_json::to_vec(&json!({"version": version, "fetchedAtMs": fetched_at_ms,
                "zipSha256": "0".repeat(64), "jsonSha256": sha256_hex(&json)}))
            .unwrap(),
        )
        .unwrap();
    }
}

/// 완료 기준 ①: 고정 판 산출물의 해시가 틀리면 캐시에 넣지 않는다. 고정 모드는 다른 판의 캐시가 아니라 내장 스냅숏으로
/// 물러난다(메인 결정, 리뷰 F2 — 계획 문장 「오프라인이면 마지막 캐시」 는 최신 모드만).
#[tokio::test]
async fn a_pinned_artifact_with_a_wrong_hash_is_not_cached_and_pinned_mode_falls_back_to_the_snapshot()
 {
    let f = Fixture::new().await;
    f.seed_cache(OLD, "old-marker", 1_700_000_000_000);
    let json = registry_json("pinned-marker");
    let zip = zip_of(&[("registry.json", &json)]);
    let r = f.registry(RegistryMode::Pinned, &zip, &json, Some(&"a".repeat(64)));
    r.refresh().await;
    assert_eq!(f.server.hits("/pinned/registry.json.zip"), 1);
    assert!(
        !f.cache_dir(PINNED).exists(),
        "a mismatching artifact must not be cached"
    );
    let index = r.index();
    assert_eq!(index.origin, RegistryOrigin::Snapshot);
    assert!(
        index.get("old-marker").is_none(),
        "another version's cache must not be used in pinned mode"
    );
    let err = r.last_error().unwrap();
    assert!(err.contains("SHA-256"), "{err}");
    // 상태에도 그대로 보인다.
    let status = status_of(&r);
    assert_eq!(status.origin, RegistryOrigin::Snapshot);
    assert_eq!(status.pinned_version, PINNED);
    assert!(status.last_error.is_some());

    // zip 은 맞는데 안의 registry.json 이 다르면(고정 json 해시와 다르면) 역시 넣지 않는다.
    let other = registry_json("tampered");
    let zip = zip_of(&[("registry.json", &other)]);
    let f2 = Fixture::new().await;
    let r = f2.registry(RegistryMode::Pinned, &zip, &json, None);
    r.refresh().await;
    assert!(!f2.cache_dir(PINNED).exists());
    assert_eq!(r.index().origin, RegistryOrigin::Snapshot);
    assert!(r.last_error().unwrap().contains("registry.json SHA-256"));
}

/// 리뷰 F1: 고정 판 폴더의 캐시라도 `registry.json` 이 고정 해시(`registry.toml` 의 `json_sha256`)와 다르면 쓰지 않는다 —
/// `meta.json` 이 그 내용에 맞춰 고쳐져 있어도.
#[tokio::test]
async fn a_pinned_cache_that_differs_from_the_pinned_hash_is_not_used() {
    let f = Fixture::new().await;
    let json = registry_json("pinned-marker");
    let zip = zip_of(&[("registry.json", &json)]);
    // 같은 판 폴더에 다른 내용 + 그 내용의 해시를 적은 meta.json.
    f.seed_cache(PINNED, "tampered-marker", 5);
    let r = f.registry(RegistryMode::Pinned, &zip, &json, None);
    r.load_local().await;
    assert_eq!(r.index().origin, RegistryOrigin::Snapshot);
    assert!(r.index().get("tampered-marker").is_none());
    // 받기는 그 캐시를 「이미 있음」 으로 보지 않고 다시 받아 고정 해시대로 바꿔 넣는다.
    assert!(r.due(RegistryMode::Pinned, SystemTime::now()));
    r.refresh().await;
    assert_eq!(f.server.hits("/pinned/registry.json.zip"), 1);
    assert_eq!(r.index().origin, RegistryOrigin::Cache);
    assert!(r.index().get("pinned-marker").is_some());
    assert_eq!(
        std::fs::read(f.cache_dir(PINNED).join("registry.json")).unwrap(),
        json
    );

    // 실제 고정 표 · 스냅숏으로도: 고정 판 폴더의 rust-analyzer 주소를 바꾼 캐시는 쓰지 않는다(리뷰 재현).
    let g = Fixture::new().await;
    let pinned = Pinned::builtin();
    let snapshot = Index::snapshot();
    let mut ra = Value::Object(snapshot.get("rust-analyzer").unwrap().raw().clone());
    ra["source"]["id"] = json!("pkg:github/evil-org/rust-analyzer@2026-10-26");
    let evil = serde_json::to_vec(&json!([ra])).unwrap();
    let dir = g.cache_dir(&pinned.registry.version);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("registry.json"), &evil).unwrap();
    std::fs::write(
        dir.join("meta.json"),
        serde_json::to_vec(
            &json!({"version": pinned.registry.version, "fetchedAtMs": 1,
            "zipSha256": "0".repeat(64), "jsonSha256": sha256_hex(&evil)}),
        )
        .unwrap(),
    )
    .unwrap();
    let real = ToolsRegistry::with_parts(pinned, Overrides::builtin(), snapshot);
    real.attach_fixed(g.cache.path().to_path_buf(), RegistryMode::Pinned);
    real.load_local().await;
    assert_eq!(real.index().origin, RegistryOrigin::Snapshot);
}

/// 리뷰 F2: 고정 모드에서 받기가 실패하고 다른 판의 캐시(옛 Pitwall 의 고정 판)만 있으면, 번들 패키지는 내장 스냅숏(고정 판)으로
/// 풀리고 고정 해시가 붙는다.
#[tokio::test]
async fn pinned_mode_without_the_pinned_cache_answers_bundled_packages_from_the_snapshot() {
    let f = Fixture::new().await;
    // 옛 판 캐시: rust-analyzer 의 판이 다르다.
    let snapshot = Index::snapshot();
    let mut ra = Value::Object(snapshot.get("rust-analyzer").unwrap().raw().clone());
    ra["source"]["id"] = json!("pkg:github/rust-lang/rust-analyzer@2020-01-01");
    let old = serde_json::to_vec(&json!([ra])).unwrap();
    let dir = f.cache_dir(OLD);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("registry.json"), &old).unwrap();
    std::fs::write(
        dir.join("meta.json"),
        serde_json::to_vec(&json!({"version": OLD, "fetchedAtMs": 9_999_999_999_999u64,
            "zipSha256": "0".repeat(64), "jsonSha256": sha256_hex(&old)}))
        .unwrap(),
    )
    .unwrap();
    let closed = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap()
    };
    let mut pinned = Pinned::builtin();
    pinned.registry.url = format!("http://{closed}/registry.json.zip");
    let r = Arc::new(
        ToolsRegistry::with_parts(pinned, Overrides::builtin(), Index::snapshot())
            .with_test_network(Endpoints::default(), Arc::new(SystemTime::now)),
    );
    r.attach_fixed(f.cache.path().to_path_buf(), RegistryMode::Pinned);
    r.load_local().await;
    r.refresh().await;
    assert!(r.last_error().is_some());
    assert_eq!(r.index().origin, RegistryOrigin::Snapshot);
    let d = get_in(&r, "rust-analyzer").unwrap();
    assert_eq!(d.registry_version, Pinned::builtin().registry.version);
    if ToolTarget::current().is_some() {
        assert_eq!(d.resolved.unwrap().version, "2026-10-26");
        assert!(!d.pinned.is_empty());
    }
}

/// 리뷰 F8: 마지막 실패에 로컬 경로를 싣지 않는다(Read tier 라 원격 readonly 기기도 본다).
#[tokio::test]
async fn the_last_error_does_not_show_local_paths() {
    let f = Fixture::new().await;
    let json = registry_json("pinned-marker");
    let zip = zip_of(&[("registry.json", &json)]);
    // 캐시 폴더 자리에 파일을 두어 쓰기를 실패시킨다.
    std::fs::write(f.cache.path().join("registry"), b"not a folder").unwrap();
    let r = f.registry(RegistryMode::Pinned, &zip, &json, None);
    r.refresh().await;
    let err = r.last_error().unwrap();
    let local = f.cache.path().display().to_string();
    assert!(!err.contains(&local), "{err}");
    assert!(err.contains("$CACHE/registry"), "{err}");
}

#[tokio::test]
async fn a_good_pinned_artifact_is_cached_and_read_back_without_the_network() {
    let f = Fixture::new().await;
    let json = registry_json("pinned-marker");
    let zip = zip_of(&[("registry.json", &json)]);
    let r = f.registry(RegistryMode::Pinned, &zip, &json, None);
    r.refresh().await;
    assert_eq!(r.last_error(), None);
    let index = r.index();
    assert_eq!(
        (index.version.as_str(), index.origin),
        (PINNED, RegistryOrigin::Cache)
    );
    assert!(index.get("pinned-marker").is_some());
    assert_eq!(
        std::fs::read(f.cache_dir(PINNED).join("registry.json")).unwrap(),
        json
    );
    // 다른 임시 폴더가 남지 않는다.
    let leftovers: Vec<_> = std::fs::read_dir(f.cache.path().join("registry"))
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.file_name())
        .collect();
    assert_eq!(leftovers, [std::ffi::OsString::from(PINNED)]);

    // 새 Host(앱을 다시 띄운 것)는 캐시를 읽고 받지 않는다.
    let r2 = f.registry(RegistryMode::Pinned, &zip, &json, None);
    r2.ensure().await;
    assert_eq!(r2.index().version, PINNED);
    assert!(!r2.due(RegistryMode::Pinned, SystemTime::now()));
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(f.server.hits("/pinned/registry.json.zip"), 1);

    // 캐시가 깨지면(내용이 meta 와 다르면) 쓰지 않는다.
    std::fs::write(f.cache_dir(PINNED).join("registry.json"), b"[]").unwrap();
    assert!(
        Cache::new(f.cache.path().join("registry"))
            .load(PINNED)
            .is_err()
    );
}

/// 완료 기준 ②: 네트워크가 없을 때(캐시도 없을 때) 내장 스냅숏으로 번들 서버의 설치 정보가 나온다.
#[tokio::test]
async fn without_network_or_cache_the_snapshot_answers_for_the_bundled_servers() {
    let f = Fixture::new().await;
    let json = registry_json("x");
    let zip = zip_of(&[("registry.json", &json)]);
    let r = f.registry(RegistryMode::Pinned, &zip, &json, None);
    // 닫힌 포트(연결 거절) — 오프라인과 같다.
    let closed = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap()
    };
    let pinned = Pinned {
        registry: PinnedRegistry {
            url: format!("http://{closed}/registry.json.zip"),
            ..r.pinned().registry.clone()
        },
        ..r.pinned().clone()
    };
    let r = Arc::new(
        ToolsRegistry::with_parts(pinned, Overrides::default(), Index::snapshot())
            .with_test_network(Endpoints::default(), Arc::new(SystemTime::now)),
    );
    r.attach_fixed(f.cache.path().to_path_buf(), RegistryMode::Pinned);
    r.refresh().await;
    assert!(
        r.last_error().unwrap().contains("network"),
        "{:?}",
        r.last_error()
    );
    let status = status_of(&r);
    assert_eq!(status.origin, RegistryOrigin::Snapshot);
    assert_eq!(status.supported + status.unsupported, 6);
    for name in Pinned::builtin().bundled {
        let detail = get_in(&r, &name).unwrap();
        assert!(detail.summary.bundled, "{name}");
        if let Some(t) = ToolTarget::current()
            && !(name == "kotlin-lsp" && t == ToolTarget::LinuxArm64Gnu)
        {
            let resolved = detail
                .resolved
                .unwrap_or_else(|| panic!("{name}: {:?}", detail.summary.unsupported));
            assert!(!resolved.bin.is_empty(), "{name}");
        }
    }
    // 이 기기의 번들 서버는 받을 파일과 고정 해시가 짝지어 나온다(실제 고정 표 — 스냅숏과 같은 판).
    if let Some(t) = ToolTarget::current() {
        let real = Arc::new(ToolsRegistry::new());
        let ra = get_in(&real, "rust-analyzer").unwrap();
        let crate::package::ToolSource::Files { files } = &ra.resolved.as_ref().unwrap().source
        else {
            panic!()
        };
        assert_eq!(ra.pinned.len(), 1, "{t}");
        assert_eq!(ra.pinned[0].name, files[0].name);
        assert_eq!(ra.pinned[0].url, files[0].url);
    }
}

#[tokio::test]
async fn latest_mode_checks_once_a_day_and_verifies_with_the_release_checksums() {
    let f = Fixture::new().await;
    let tag = "2026-11-02-fresh-tag";
    let json = registry_json("latest-marker");
    let zip = zip_of(&[("registry.json", &json)]);
    f.server.route(
        "/api/latest",
        Reply::Body(serde_json::to_vec(&json!({"tag_name": tag})).unwrap()),
    );
    f.server.route(
        &format!("/download/{tag}/checksums.txt"),
        Reply::Body(
            format!(
                "{}  registry.json\n{}  registry.json.zip\n",
                sha256_hex(&json),
                sha256_hex(&zip)
            )
            .into_bytes(),
        ),
    );
    f.server.route(
        &format!("/download/{tag}/registry.json.zip"),
        Reply::Body(zip.clone()),
    );
    let r = f.registry(RegistryMode::Latest, &zip, &json, None);
    assert!(r.due(RegistryMode::Latest, *f.clock.lock().unwrap()));
    r.refresh().await;
    assert_eq!(r.last_error(), None);
    assert_eq!(r.index().version, tag);
    assert!(r.index().get("latest-marker").is_some());
    assert!(f.cache_dir(tag).join("meta.json").exists());

    // 하루 안에는 다시 보지 않는다 — 앱을 다시 띄워도(latest.json).
    f.advance(Duration::from_secs(23 * 3600));
    assert!(!r.due(RegistryMode::Latest, *f.clock.lock().unwrap()));
    let r2 = f.registry(RegistryMode::Latest, &zip, &json, None);
    r2.load_local().await;
    assert_eq!(r2.index().version, tag);
    assert!(!r2.due(RegistryMode::Latest, *f.clock.lock().unwrap()));
    // 하루가 지나면 다시 본다. 같은 태그면 받지 않는다.
    f.advance(Duration::from_secs(2 * 3600));
    assert!(r.due(RegistryMode::Latest, *f.clock.lock().unwrap()));
    r.refresh().await;
    assert_eq!(f.server.hits("/api/latest"), 2);
    assert_eq!(
        f.server.hits(&format!("/download/{tag}/registry.json.zip")),
        1
    );
    // 모드가 바뀌면 바로 다시 한다.
    assert!(r.due(RegistryMode::Pinned, *f.clock.lock().unwrap()));
}

#[tokio::test]
async fn latest_mode_rejects_a_mismatching_checksum_and_a_bad_tag() {
    let f = Fixture::new().await;
    f.seed_cache(OLD, "old-marker", 1);
    let tag = "2026-11-02-fresh-tag";
    let json = registry_json("latest-marker");
    let zip = zip_of(&[("registry.json", &json)]);
    f.server.route(
        "/api/latest",
        Reply::Body(serde_json::to_vec(&json!({"tag_name": tag})).unwrap()),
    );
    f.server.route(
        &format!("/download/{tag}/checksums.txt"),
        Reply::Body(
            format!(
                "{}  registry.json\n{}  registry.json.zip\n",
                sha256_hex(&json),
                "b".repeat(64)
            )
            .into_bytes(),
        ),
    );
    f.server.route(
        &format!("/download/{tag}/registry.json.zip"),
        Reply::Body(zip.clone()),
    );
    let r = f.registry(RegistryMode::Latest, &zip, &json, None);
    r.refresh().await;
    assert!(!f.cache_dir(tag).exists());
    assert_eq!(r.index().version, OLD);
    assert!(r.last_error().unwrap().contains("SHA-256"));

    // 폴더 이름이 될 태그는 꼴을 본다(`..` 등).
    f.server.route(
        "/api/latest",
        Reply::Body(serde_json::to_vec(&json!({"tag_name": "../../evil"})).unwrap()),
    );
    f.advance(RETRY_AFTER);
    r.refresh().await;
    assert!(r.last_error().unwrap().contains("unexpected tag"));
    assert!(!f.cache.path().join("evil").exists());
    // 실패한 뒤에는 한 시간이 지나야 다시 한다.
    assert!(!r.due(
        RegistryMode::Latest,
        *f.clock.lock().unwrap() + Duration::from_secs(60)
    ));
    assert!(r.due(RegistryMode::Latest, *f.clock.lock().unwrap() + RETRY_AFTER));
}

#[tokio::test]
async fn search_and_get_answer_from_the_current_index() {
    let f = Fixture::new().await;
    f.seed_cache(PINNED, "gofmt-plus", 5);
    let json = registry_json("gofmt-plus");
    let zip = zip_of(&[("registry.json", &json)]);
    let r = f.registry(RegistryMode::Pinned, &zip, &json, None);
    r.ensure().await;
    assert_eq!(r.index().origin, RegistryOrigin::Cache);
    let params = |query: &str| ToolsRegistrySearchParams {
        query: query.into(),
        category: None,
        language: None,
        limit: None,
    };
    let names = |res: crate::methods::ToolsRegistrySearch| -> Vec<String> {
        res.items.into_iter().map(|i| i.name).collect()
    };
    assert_eq!(
        names(search_in(&r, &params("RUST")).unwrap()),
        ["rust-analyzer"]
    );
    // 이름이 먼저, 설명에서만 맞는 것은 뒤.
    assert_eq!(
        names(search_in(&r, &params("marker")).unwrap()),
        ["gofmt-plus"]
    );
    assert_eq!(names(search_in(&r, &params("")).unwrap()).len(), 2);
    let mut p = params("");
    p.category = Some("formatter".into());
    assert_eq!(names(search_in(&r, &p).unwrap()), ["gofmt-plus"]);
    let mut p = params("");
    p.language = Some("rust".into());
    assert_eq!(names(search_in(&r, &p).unwrap()), ["rust-analyzer"]);
    let mut p = params("");
    p.limit = Some(1);
    let res = search_in(&r, &p).unwrap();
    assert!(res.truncated);
    assert_eq!(res.items.len(), 1);
    p.limit = Some(0);
    assert!(search_in(&r, &p).is_err());
    assert!(search_in(&r, &params(&"x".repeat(300))).is_err());

    let rust = search_in(&r, &params("rust")).unwrap().items.remove(0);
    assert!(rust.bundled);
    let err = get_in(&r, "nope").unwrap_err();
    assert_eq!(err.code, pw_rpc::code::NOT_FOUND);
    let d = get_in(&r, "gofmt-plus").unwrap();
    assert_eq!(d.registry_version, PINNED);
    assert!(!d.summary.bundled);
    assert!(d.pinned.is_empty());
}

/// 리뷰 F4: `get` 의 고정 해시는 받을 파일과 (이름, 주소)로 짝지어지고, 짝이 없으면 「체크섬 없음」 이다.
#[tokio::test]
async fn get_pairs_pinned_hashes_with_what_will_be_downloaded() {
    use crate::methods::ToolChecksum;
    let Some(target) = ToolTarget::current() else {
        return;
    };
    let real = ToolsRegistry::new();
    for name in Pinned::builtin().bundled {
        let d = get_in(&real, &name).unwrap();
        if name == "kotlin-lsp" && target == ToolTarget::LinuxArm64Gnu {
            assert_eq!(d.checksum, None);
            continue;
        }
        assert_eq!(d.checksum, Some(ToolChecksum::Pinned), "{name}");
        if let crate::package::ToolSource::Files { files } = &d.resolved.unwrap().source {
            let mut want: Vec<(&str, &str)> = files
                .iter()
                .map(|f| (f.name.as_str(), f.url.as_str()))
                .collect();
            let mut got: Vec<(&str, &str)> = d
                .pinned
                .iter()
                .map(|p| (p.name.as_str(), p.url.as_str()))
                .collect();
            want.sort();
            got.sort();
            assert_eq!(got, want, "{name}");
        } else {
            assert!(!d.pinned.is_empty(), "{name}");
        }
    }
    // 같은 판인데 registry 가 주소를 바꾼 정의(최신 모드 · 오염)에는 고정 해시를 내보이지 않는다.
    let snapshot = Index::snapshot();
    let mut ra = Value::Object(snapshot.get("rust-analyzer").unwrap().raw().clone());
    ra["source"]["id"] = json!("pkg:github/evil-org/rust-analyzer@2026-10-26");
    let mut ts = Value::Object(
        snapshot
            .get("typescript-language-server")
            .unwrap()
            .raw()
            .clone(),
    );
    ts["source"]["extra_packages"] = json!(["typescript@9.9.9"]);
    let json = serde_json::to_vec(&json!([ra, ts])).unwrap();
    let index =
        Index::from_registry_json(&json, "2026-11-01-batty-dew", RegistryOrigin::Cache, None)
            .unwrap();
    let r = ToolsRegistry::with_parts(Pinned::builtin(), Overrides::default(), index);
    for name in ["rust-analyzer", "typescript-language-server"] {
        let d = get_in(&r, name).unwrap();
        assert!(d.summary.bundled);
        assert_eq!(d.checksum, Some(ToolChecksum::None), "{name}");
        assert!(d.pinned.is_empty(), "{name}");
    }
}
