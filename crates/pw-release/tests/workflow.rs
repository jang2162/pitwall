//! 릴리스 워크플로(`.github/workflows/release.yml`)의 구조 규칙. actionlint 는 문법만 보므로, 어느 잡이 비밀 · 쓰기 토큰을
//! 갖고 어느 잡이 저장소 코드를 돌리는지는 여기서 지킨다(CONTRIBUTING.md 「릴리스」).
//!
//! 잡은 `jobs:` 아래 두 칸 들여쓴 `<이름>:` 줄로 가른다. 주석 줄은 보지 않는다.
//! 보지 않는 것: 식의 의미(조건이 실제로 참이 되는지), 재사용 워크플로 · 합성 액션 안의 단계.

const WORKFLOW: &str = include_str!("../../../.github/workflows/release.yml");
const MISE: &str = include_str!("../../../mise.toml");
const PNPM_LOCK: &str = include_str!("../../../pnpm-lock.yaml");

/// (잡 이름, 주석을 뺀 줄들).
fn jobs() -> Vec<(String, Vec<&'static str>)> {
    let mut lines = WORKFLOW.lines().skip_while(|l| *l != "jobs:").skip(1);
    let mut out: Vec<(String, Vec<&'static str>)> = Vec::new();
    for line in lines.by_ref() {
        let is_job = line.starts_with("  ")
            && !line.starts_with("   ")
            && line.ends_with(':')
            && !line.trim_start().starts_with('#');
        if is_job {
            out.push((line.trim().trim_end_matches(':').to_owned(), Vec::new()));
        } else if let Some((_, body)) = out.last_mut()
            && !line.trim_start().starts_with('#')
        {
            body.push(line);
        }
    }
    assert!(
        out.len() >= 5,
        "잡을 가르지 못했다: {:?}",
        out.iter().map(|j| &j.0).collect::<Vec<_>>()
    );
    out
}

fn job(name: &str) -> Vec<&'static str> {
    jobs()
        .into_iter()
        .find(|(n, _)| n == name)
        .unwrap_or_else(|| panic!("잡 {name} 이 없다"))
        .1
}

fn text(body: &[&str]) -> String {
    body.join("\n")
}

/// `mise.toml` 의 `<도구> = "<버전>"`.
fn mise_version(tool: &str) -> String {
    MISE.lines()
        .find_map(|l| l.strip_prefix(&format!("{tool} = \"")))
        .and_then(|rest| rest.strip_suffix('"'))
        .unwrap_or_else(|| panic!("mise.toml 에 {tool} 이 없다"))
        .to_owned()
}

/// 운영 updater 키 Secret 은 sign-updater 잡만 참조한다 — 조건식 안에서도. 다른 잡(특히 PR 코드가 도는 build)의 runner 에
/// 그 값이 내려가지 않게.
#[test]
fn only_sign_updater_references_the_production_updater_key() {
    let outside_jobs = WORKFLOW
        .lines()
        .take_while(|l| *l != "jobs:")
        .filter(|l| !l.trim_start().starts_with('#'))
        .any(|l| l.contains("secrets.TAURI_SIGNING"));
    assert!(!outside_jobs, "워크플로 수준 env 에 운영 키가 있다");
    for (name, body) in jobs() {
        let uses = body
            .iter()
            .filter(|l| l.contains("secrets.TAURI_SIGNING"))
            .count();
        if name == "sign-updater" {
            assert!(uses > 0, "sign-updater 가 운영 키를 쓰지 않는다");
        } else {
            assert_eq!(uses, 0, "{name} 잡이 운영 updater 키를 참조한다");
        }
    }
}

/// 비밀(sign-updater) · 쓰기 토큰(publish)을 가진 잡은 저장소를 checkout 하지도, 저장소 코드를 빌드 · 실행하지도 않는다.
#[test]
fn privileged_jobs_do_not_run_repository_code() {
    for name in ["sign-updater", "publish"] {
        let body = text(&job(name));
        for forbidden in [
            "actions/checkout",
            "cargo ",
            "pnpm install",
            "mise run",
            "pnpm --filter",
        ] {
            assert!(
                !body.contains(forbidden),
                "{name} 잡에 {forbidden:?} 가 있다"
            );
        }
    }
}

/// sign-updater · publish 는 push 이벤트이면서 plan 이 공개로 판정했을 때만 돈다.
#[test]
fn privileged_jobs_run_only_for_published_tag_pushes() {
    for name in ["sign-updater", "publish"] {
        let body = job(name);
        let condition = body
            .iter()
            .find(|l| l.trim_start().starts_with("if:"))
            .unwrap_or_else(|| panic!("{name} 잡에 if 가 없다"));
        assert!(
            condition.contains("github.event_name == 'push'"),
            "{name}: {condition}"
        );
        assert!(
            condition.contains("needs.plan.outputs.publish == 'true'"),
            "{name}: {condition}"
        );
    }
}

/// build 잡은 늘 일회용 키로 서명하고 그 .sig 를 모으지 않는다. manifest 는 sign-updater 다음이다.
#[test]
fn the_build_job_signs_with_a_throwaway_key_and_keeps_no_signatures() {
    let build = job("build");
    assert!(text(&build).contains("signer generate"));
    assert!(
        !build
            .iter()
            .any(|l| l.contains("cp ") && l.contains(".sig")),
        "build 잡이 .sig 를 산출물에 모은다"
    );
    let manifest = text(&job("manifest"));
    assert!(
        manifest.contains("needs: [plan, build, sign-updater]"),
        "{manifest}"
    );
}

/// 재서명은 버전을 서명에 싣고(앱의 requireSignedVersion), 저장소와 같은 도구 버전을 쓴다.
#[test]
fn sign_updater_binds_the_version_with_the_locked_tools() {
    let body = text(&job("sign-updater"));
    assert!(body.contains("--app-version \"$VERSION\""), "{body}");
    let cli = PNPM_LOCK
        .lines()
        .find_map(|l| l.trim().strip_prefix("'@tauri-apps/cli@"))
        .and_then(|rest| rest.strip_suffix("':"))
        .expect("pnpm-lock.yaml 의 @tauri-apps/cli");
    for pinned in [
        format!("@tauri-apps/cli@{cli}"),
        format!("node@{}", mise_version("node")),
        format!("npm:pnpm@{}", mise_version("\"npm:pnpm\"")),
    ] {
        assert!(
            body.contains(&pinned),
            "sign-updater 가 {pinned} 를 쓰지 않는다"
        );
    }
}

/// publish 재실행: draft 면 덮어 올려 공개하고, 이미 공개됐으면 손대지 않고 실패한다.
#[test]
fn publish_resumes_a_draft_and_refuses_a_published_release() {
    let body = job("publish");
    let branch = |label: &str| {
        let start = body
            .iter()
            .position(|l| l.trim() == label)
            .unwrap_or_else(|| panic!("publish 에 {label} 분기가 없다"));
        let end = body[start..]
            .iter()
            .position(|l| l.trim_end().ends_with(";;"))
            .map_or(body.len(), |i| start + i + 1);
        text(&body[start..end])
    };
    assert!(branch("draft)").contains("--clobber"));
    let published = branch("published)");
    assert!(published.contains("exit 1"), "{published}");
    assert!(!published.contains("gh release upload"), "{published}");
    assert!(branch("none)").contains("--draft"));
}

/// Relay 바이너리(18-m7-plan M7-05)는 공개하는 실행만 아티팩트로 올린다 — dry-run 은 항목 · `file` 결과 · 체크섬(글)만.
#[test]
fn relay_binaries_are_uploaded_only_by_publishing_runs() {
    let body = job("relay");
    let uploads: Vec<usize> = body
        .iter()
        .enumerate()
        .filter(|(_, l)| l.contains("actions/upload-artifact"))
        .map(|(i, _)| i)
        .collect();
    assert_eq!(uploads.len(), 2, "{}", text(&body));
    let mut binary_uploads = 0;
    for start in uploads {
        let step = text(&body[start..(start + 6).min(body.len())]);
        if step.contains("pitwall-relay-") {
            binary_uploads += 1;
            assert!(
                step.contains("if: needs.plan.outputs.binaries == 'true'"),
                "{step}"
            );
            assert!(step.contains("name: release-relay-"), "{step}");
        } else {
            assert!(step.contains("name: report-relay-"), "{step}");
        }
    }
    assert_eq!(binary_uploads, 1);
    // 빌드는 저장소 스크립트(정적 링크 · 크기 상한 판정)를 지나고, 채널을 plan 에서 받아 박는다.
    let all = text(&body);
    assert!(all.contains("node scripts/relay-build.mjs"), "{all}");
    assert!(
        all.contains("PITWALL_RELEASE_CHANNEL: ${{ needs.plan.outputs.channel }}"),
        "{all}"
    );
    // 매트릭스는 자산 표(`pw_release::relay::PLATFORMS`)와 같다 — 플랫폼 · 타깃 짝이 하나씩.
    let platforms = body
        .iter()
        .filter(|l| l.trim_start().starts_with("- platform:"))
        .count();
    assert_eq!(platforms, pw_release::relay::PLATFORMS.len(), "{all}");
    for platform in &pw_release::relay::PLATFORMS {
        let pair = format!("- platform: {}\n", platform.asset);
        let at = all
            .find(&pair)
            .unwrap_or_else(|| panic!("no relay matrix entry for {}", platform.asset));
        let entry: String = all[at..].lines().take(3).collect::<Vec<_>>().join("\n");
        assert!(
            entry.contains(&format!("target: {}", platform.rust_target)),
            "{entry}"
        );
    }
}

/// Relay 매니페스트는 세 플랫폼이 다 있어야 만들고, 공개 실행은 올릴 바이너리를 다시 해시해 견준다. 서명은 sign-updater 가 같은 운영
/// 키 · 판 묶기로 하고, publish 가 매니페스트를 Release 에 붙인다.
#[test]
fn the_relay_manifest_is_complete_checked_signed_and_published() {
    let manifest = text(&job("relay-manifest"));
    assert!(manifest.contains("--complete true"), "{manifest}");
    assert!(manifest.contains("--check binaries"), "{manifest}");
    assert!(!manifest.contains("secrets."), "{manifest}");
    let sign = text(&job("sign-updater"));
    assert!(
        sign.contains("needs: [plan, build, relay-manifest]"),
        "{sign}"
    );
    assert!(
        sign.contains("signer sign --app-version \"$VERSION\" relay/relay-manifest.json"),
        "{sign}"
    );
    assert!(
        sign.contains("mv relay/relay-manifest.json.sig signatures/"),
        "{sign}"
    );
    let publish = text(&job("publish"));
    assert!(publish.contains("name: relay-manifest"), "{publish}");
}

/// 워크플로 전체에서 Relay 바이너리(`pitwall-relay-*` 또는 relay 바이너리 아티팩트)를 올리는 업로드는 모두 공개하는 실행에서만이다 — 새 잡이
/// 바이너리를 조건 없이 올리면 실패한다(M7-05 리뷰 권고 5).
#[test]
fn every_relay_binary_upload_is_for_publishing_runs_only() {
    let mut seen = 0;
    for (name, body) in jobs() {
        for (i, line) in body.iter().enumerate() {
            if !line.contains("actions/upload-artifact") {
                continue;
            }
            let end = body[i + 1..]
                .iter()
                .position(|l| l.trim_start().starts_with("- "))
                .map_or(body.len(), |n| i + 1 + n);
            let step = text(&body[i..end]);
            let binaries = step.contains("pitwall-relay-")
                || step.contains("name: release-relay-")
                || step.contains("path: binaries");
            if binaries {
                seen += 1;
                assert!(
                    step.contains("if: needs.plan.outputs.binaries == 'true'"),
                    "{name}: {step}"
                );
            }
        }
    }
    assert!(seen >= 1, "no relay binary upload found");
}
