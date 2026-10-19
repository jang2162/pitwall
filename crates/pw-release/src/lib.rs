//! 릴리스 워크플로(`.github/workflows/release.yml`)가 부르는 판정. 무엇을 하는지는 CONTRIBUTING.md 「릴리스」 절, 결정은
//! `docs/design/02-tech-stack.md` §3-1.
//!
//! - [`plan`]: 이 실행이 무엇을 만드는가 — 버전 · 태그 · 채널 · 공개 여부 · MSI 버전 · Linux 번들 · updater 서명 키 ·
//!   바이너리 업로드. **버전의 원천은 소스다**(워크스페이스 `Cargo.toml` 의 `version`, 데스크톱 크레이트가 그것을 쓴다).
//!   태그 push 는 태그가 소스 버전과 같고 태그 커밋이 `origin/main` 의 조상이어야 하며, 아니면 실패한다.
//! - [`manifest`]: 빌드 산출물에서 updater 매니페스트(`latest.json`)를 만든다.
//! - [`publish_action`]: 같은 태그의 Release 가 이미 있을 때 publish 잡이 무엇을 할지(재실행).

use std::fmt;
use std::path::{Path, PathBuf};

use semver::Version;
use serde_json::{Map, Value, json};

/// 소스의 버전. 이 크레이트도 데스크톱 앱처럼 `version.workspace = true` 다(시험이 지킨다).
pub const SOURCE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// preview 버전의 prerelease 접두.
const PREVIEW: &str = "preview.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    Stable,
    Preview,
}

impl Channel {
    pub fn as_str(self) -> &'static str {
        match self {
            Channel::Stable => "stable",
            Channel::Preview => "preview",
        }
    }
}

/// 이 실행이 만드는 것.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub version: Version,
    /// 릴리스 태그. dry-run 이면 「이 버전을 태그로 냈다면」의 이름이다(매니페스트 주소에 쓴다).
    pub tag: String,
    pub channel: Channel,
    /// 태그 push 일 때만 참이다 — GitHub Release 를 만든다.
    pub publish: bool,
    /// WiX(MSI) 버전 `major.minor.patch[.build]`. MSI 는 prerelease 를 숫자로만 받아 preview 의 `N` 을 build 칸에 둔다.
    pub wix_version: String,
    /// Linux 잡이 만드는 번들(`tauri build --bundles`). preview 는 AppImage 만이다 — deb · rpm 의 패키지 관리자는
    /// `X.Y.Z-preview.N` 을 `X.Y.Z` 보다 높게 정렬해, preview 패키지를 설치한 사람이 같은 X.Y.Z 의 stable 로 올라가지 못한다.
    /// rpm 은 Version 에 `-` 를 받지 않기도 한다. preview 를 쓰는 Linux 사용자는 자동 업데이트되는 AppImage 를 쓴다.
    pub linux_bundles: &'static str,
    /// updater 산출물을 서명할 키. 운영 키(`TAURI_SIGNING_PRIVATE_KEY`)는 공개하는 실행(tag push)에만 쓴다 — dry-run 은 잡
    /// 안에서 만든 일회용 키로 서명한다.
    pub updater_key: UpdaterKey,
    /// 설치 파일 · updater 산출물 · `.sig` 를 워크플로 아티팩트로 올리는가. 공개하는 실행만 올린다 — 공개 저장소의 아티팩트는
    /// 로그인한 누구나 받을 수 있어, dry-run 이 올리면 병합 안 된 코드의 Developer ID 서명 · 공증 바이너리가 배포된다.
    /// dry-run 은 체크섬 · 검증 로그 · `latest.json` 같은 글만 올린다.
    pub binaries: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdaterKey {
    /// 운영 키(저장소 Secret).
    Release,
    /// 잡 안에서 만든 일회용 키.
    Ephemeral,
}

impl UpdaterKey {
    pub fn as_str(self) -> &'static str {
        match self {
            UpdaterKey::Release => "release",
            UpdaterKey::Ephemeral => "ephemeral",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error(String);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

fn error(message: impl Into<String>) -> Error {
    Error(message.into())
}

/// 버전의 채널. prerelease 가 없으면 stable, `preview.<N>` 이면 preview, 그 밖(빌드 메타데이터 포함)은 오류다.
pub fn channel_of(version: &Version) -> Result<Channel, Error> {
    if !version.build.is_empty() {
        return Err(error(format!(
            "version {version} has build metadata; release versions are X.Y.Z or X.Y.Z-preview.N"
        )));
    }
    if version.pre.is_empty() {
        return Ok(Channel::Stable);
    }
    preview_number(version).map(|_| Channel::Preview)
}

fn preview_number(version: &Version) -> Result<u16, Error> {
    version
        .pre
        .as_str()
        .strip_prefix(PREVIEW)
        .and_then(|n| n.parse::<u16>().ok())
        .ok_or_else(|| {
            error(format!(
                "version {version} is neither X.Y.Z nor X.Y.Z-preview.N (N a number up to 65535)"
            ))
        })
}

/// MSI 버전. major · minor 는 255, patch · build 는 65535 까지다(Windows Installer 의 ProductVersion).
pub fn wix_version(version: &Version) -> Result<String, Error> {
    if version.major > 255 || version.minor > 255 || version.patch > 65535 {
        return Err(error(format!(
            "version {version} does not fit an MSI version (major and minor up to 255, patch up to 65535)"
        )));
    }
    let base = format!("{}.{}.{}", version.major, version.minor, version.patch);
    Ok(match channel_of(version)? {
        Channel::Stable => base,
        Channel::Preview => format!("{base}.{}", preview_number(version)?),
    })
}

/// 이 실행의 계획. `git_ref` 는 `GITHUB_REF`(예: `refs/tags/v0.1.0-preview.1`, `refs/pull/3/merge`), `source` 는 소스의 버전,
/// `on_main` 은 이 실행의 커밋이 `origin/main` 의 조상인지(`git merge-base --is-ancestor`). `refs/tags/` 가 아니면 dry-run 이다
/// (공개하지 않는다).
pub fn plan(git_ref: &str, source: &str, on_main: bool) -> Result<Plan, Error> {
    let version = Version::parse(source)
        .map_err(|e| error(format!("source version {source} is not semver: {e}")))?;
    let publish = match git_ref.strip_prefix("refs/tags/") {
        Some(tag) => {
            let tagged = tag.strip_prefix('v').ok_or_else(|| {
                error(format!(
                    "tag {tag} does not start with v (vX.Y.Z or vX.Y.Z-preview.N)"
                ))
            })?;
            if tagged != source {
                return Err(error(format!(
                    "tag {tag} does not match the source version {source} (workspace Cargo.toml). \
                     Set the workspace version to {tagged} in a commit first and tag that commit"
                )));
            }
            // 태그만 달면 그 커밋의 워크플로 · 코드로 공개까지 간다. 리뷰를 거쳐 main 에 들어간 커밋만 공개한다.
            if !on_main {
                return Err(error(format!(
                    "tag {tag} points to a commit that is not on main. Releases are cut from reviewed commits: \
                     merge the version bump into main and tag that commit"
                )));
            }
            true
        }
        None => false,
    };
    let channel = channel_of(&version)?;
    Ok(Plan {
        linux_bundles: match channel {
            Channel::Stable => "appimage,deb,rpm",
            Channel::Preview => "appimage",
        },
        updater_key: if publish {
            UpdaterKey::Release
        } else {
            UpdaterKey::Ephemeral
        },
        binaries: publish,
        channel,
        wix_version: wix_version(&version)?,
        tag: format!("v{version}"),
        version,
        publish,
    })
}

impl Plan {
    /// `$GITHUB_OUTPUT` 에 쓰는 `key=value` 줄.
    pub fn outputs(&self) -> String {
        format!(
            "version={}\ntag={}\nchannel={}\nprerelease={}\npublish={}\nwix_version={}\n\
             linux_bundles={}\nupdater_key={}\nbinaries={}\n",
            self.version,
            self.tag,
            self.channel.as_str(),
            self.channel == Channel::Preview,
            self.publish,
            self.wix_version,
            self.linux_bundles,
            self.updater_key.as_str(),
            self.binaries,
        )
    }
}

/// 매니페스트의 플랫폼 키와 그 키가 가리킬 updater 산출물(파일 이름 끝).
///
/// - macOS 는 유니버설 하나를 두 아키텍처 키에 건다.
/// - Linux 는 `-appimage` 를 붙인다. updater 는 `{os}-{arch}-{설치 형식}` 을 먼저, 없으면 `{os}-{arch}` 를 찾는다 — 맨 키를
///   쓰면 deb · rpm 으로 설치한 앱이 AppImage 를 받아 설치하려 든다. deb · rpm 은 자동 업데이트하지 않는다(02 §3-1).
/// - Windows 는 MSI 만 낸다.
const PLATFORMS: &[(&[&str], &str)] = &[
    (&["darwin-aarch64", "darwin-x86_64"], ".app.tar.gz"),
    (&["linux-x86_64-appimage"], ".AppImage"),
    (&["windows-x86_64"], ".msi"),
];

/// GitHub 릴리스 자산 이름으로 그대로 쓸 수 있는 파일 이름인가. 그 밖의 글자는 GitHub 가 바꿔 올려 주소가 어긋난다.
fn plain_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// `dir` 아래(하위 폴더 포함)의 파일 경로.
fn files_under(dir: &Path) -> Result<Vec<PathBuf>, Error> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = std::fs::read_dir(&dir)
            .map_err(|e| error(format!("cannot read {}: {e}", dir.display())))?;
        for entry in entries {
            let path = entry
                .map_err(|e| error(format!("cannot read {}: {e}", dir.display())))?
                .path();
            if path.is_dir() {
                stack.push(path);
            } else {
                out.push(path);
            }
        }
    }
    out.sort();
    Ok(out)
}

/// 빌드 산출물(`dir`)에서 updater 매니페스트를 만든다. 플랫폼마다 산출물과 그 `.sig` 가 정확히 하나씩 있어야 한다 — 빠진
/// 플랫폼이 있는 매니페스트를 내면 그 플랫폼의 설치본은 업데이트를 못 받는다.
/// 주소는 `https://github.com/<repo>/releases/download/<tag>/<파일 이름>` 이다(릴리스 자산은 폴더 없이 올라간다).
pub fn manifest(
    plan: &Plan,
    repo: &str,
    dir: &Path,
    notes: &str,
    pub_date: &str,
) -> Result<Value, Error> {
    let files = files_under(dir)?;
    let mut platforms = Map::new();
    for (keys, suffix) in PLATFORMS {
        let found: Vec<&PathBuf> = files
            .iter()
            .filter(|p| p.to_string_lossy().ends_with(suffix))
            .collect();
        let artifact = match found.as_slice() {
            [one] => *one,
            [] => {
                return Err(error(format!(
                    "no *{suffix} under {} (for {})",
                    dir.display(),
                    keys.join(", ")
                )));
            }
            many => {
                return Err(error(format!(
                    "more than one *{suffix} under {}: {many:?}",
                    dir.display()
                )));
            }
        };
        let name = artifact
            .file_name()
            .and_then(|n| n.to_str())
            .filter(|n| plain_name(n))
            .ok_or_else(|| {
                error(format!(
                    "{} is not a plain asset name (letters, digits, . _ -)",
                    artifact.display()
                ))
            })?;
        let sig_path = PathBuf::from(format!("{}.sig", artifact.display()));
        let signature = std::fs::read_to_string(&sig_path)
            .map_err(|e| error(format!("cannot read {}: {e}", sig_path.display())))?;
        let signature = signature.trim();
        if signature.is_empty() {
            return Err(error(format!("{} is empty", sig_path.display())));
        }
        let url = format!(
            "https://github.com/{repo}/releases/download/{}/{name}",
            plan.tag
        );
        for key in *keys {
            platforms.insert(
                (*key).to_owned(),
                json!({"signature": signature, "url": url}),
            );
        }
    }
    Ok(json!({
        "version": plan.version.to_string(),
        "notes": notes,
        "pub_date": pub_date,
        "platforms": platforms,
    }))
}

/// 같은 태그의 Release 상태(`none` · `draft` · `published`)에 따른 publish 잡의 할 일.
/// - `none` → `create`: draft 로 만들어 자산을 올리고 공개한다.
/// - `draft` → `resume`: 앞선 실행이 draft 까지 만들고 멈췄다. 자산을 덮어 올리고(`--clobber`) 공개한다 — 재실행이 성공한다.
/// - `published` → 실패: 이미 공개된 릴리스의 자산을 바꾸지 않는다(받은 사람과 다른 바이트가 된다).
pub fn publish_action(existing: &str) -> Result<&'static str, Error> {
    match existing {
        "none" => Ok("create"),
        "draft" => Ok("resume"),
        "published" => Err(error(
            "a published release with this tag already exists; it is left unchanged. \
             Cut a new version instead of replacing published assets",
        )),
        other => Err(error(format!(
            "unknown release state {other} (none | draft | published)"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn version(s: &str) -> Version {
        Version::parse(s).unwrap()
    }

    #[test]
    fn a_matching_tag_publishes() {
        let p = plan("refs/tags/v0.1.0-preview.1", "0.1.0-preview.1", true).unwrap();
        assert!(p.publish);
        assert_eq!(p.channel, Channel::Preview);
        assert_eq!(p.tag, "v0.1.0-preview.1");
        assert_eq!(p.wix_version, "0.1.0.1");
        let p = plan("refs/tags/v1.2.3", "1.2.3", true).unwrap();
        assert_eq!((p.channel, p.publish), (Channel::Stable, true));
        assert_eq!(p.wix_version, "1.2.3");
    }

    #[test]
    fn a_tag_that_differs_from_the_source_fails() {
        let e = plan("refs/tags/v0.1.0-preview.2", "0.1.0-preview.1", true).unwrap_err();
        assert!(e.to_string().contains("does not match"), "{e}");
        assert!(
            plan("refs/tags/0.1.0", "0.1.0", true).is_err(),
            "v 가 없는 태그"
        );
        assert!(plan("refs/tags/v0.1", "0.1.0", true).is_err());
    }

    /// 태그가 아닌 실행(PR · 브랜치 dispatch)은 공개하지 않는다.
    #[test]
    fn anything_but_a_tag_is_a_dry_run() {
        for git_ref in ["refs/pull/12/merge", "refs/heads/main", ""] {
            let p = plan(git_ref, "0.0.0", true).unwrap();
            assert!(!p.publish, "{git_ref}");
            assert_eq!(p.tag, "v0.0.0");
            assert_eq!(p.channel, Channel::Stable);
        }
    }

    #[test]
    fn only_stable_and_preview_versions_are_released() {
        for bad in [
            "0.1.0-rc.1",
            "0.1.0-preview",
            "0.1.0-preview.x",
            "0.1.0-preview.01",
            "0.1.0+build.5",
            "0.1.0-preview.70000",
        ] {
            // semver 가 먼저 거절하는 것(앞자리 0)도 있다.
            let parsed = Version::parse(bad).map_err(drop);
            assert!(
                parsed.and_then(|v| channel_of(&v).map_err(drop)).is_err(),
                "{bad}"
            );
            assert!(plan("refs/pull/1/merge", bad, true).is_err(), "{bad}");
        }
        assert_eq!(
            channel_of(&version("0.1.0-preview.0")).unwrap(),
            Channel::Preview
        );
    }

    #[test]
    fn msi_versions_fit_windows_installer_limits() {
        assert_eq!(
            wix_version(&version("0.1.0-preview.65535")).unwrap(),
            "0.1.0.65535"
        );
        assert!(wix_version(&version("256.0.0")).is_err());
        assert!(wix_version(&version("0.256.0")).is_err());
        assert!(wix_version(&version("0.0.65536")).is_err());
    }

    #[test]
    fn outputs_are_github_output_lines() {
        let p = plan("refs/tags/v0.1.0-preview.1", "0.1.0-preview.1", true).unwrap();
        assert_eq!(
            p.outputs(),
            "version=0.1.0-preview.1\ntag=v0.1.0-preview.1\nchannel=preview\nprerelease=true\npublish=true\nwix_version=0.1.0.1\n\
             linux_bundles=appimage\nupdater_key=release\nbinaries=true\n"
        );
    }

    /// main 밖 커밋에 단 태그는 공개하지 않는다. 태그가 아닌 실행(dry-run)은 main 밖이어도 된다(PR 브랜치).
    #[test]
    fn a_tag_off_main_fails() {
        let e = plan("refs/tags/v0.1.0", "0.1.0", false).unwrap_err();
        assert!(e.to_string().contains("not on main"), "{e}");
        assert!(!plan("refs/pull/3/merge", "0.1.0", false).unwrap().publish);
    }

    /// preview 는 Linux 에 AppImage 만 낸다(deb · rpm 정렬 문제), stable 은 셋 다.
    #[test]
    fn preview_ships_only_the_appimage_on_linux() {
        assert_eq!(
            plan("refs/tags/v0.1.0-preview.1", "0.1.0-preview.1", true)
                .unwrap()
                .linux_bundles,
            "appimage"
        );
        assert_eq!(
            plan("refs/pull/1/merge", "0.1.0-preview.1", true)
                .unwrap()
                .linux_bundles,
            "appimage"
        );
        assert_eq!(
            plan("refs/tags/v0.1.0", "0.1.0", true)
                .unwrap()
                .linux_bundles,
            "appimage,deb,rpm"
        );
    }

    /// dry-run 은 운영 updater 키를 쓰지 않고 바이너리를 올리지 않는다. 공개하는 실행만 둘 다 한다.
    #[test]
    fn dry_runs_sign_with_a_throwaway_key_and_upload_no_binaries() {
        for git_ref in [
            "refs/pull/12/merge",
            "refs/heads/main",
            "refs/heads/feature",
        ] {
            let p = plan(git_ref, "0.1.0-preview.1", true).unwrap();
            assert_eq!(p.updater_key, UpdaterKey::Ephemeral, "{git_ref}");
            assert!(!p.binaries, "{git_ref}");
        }
        let p = plan("refs/tags/v0.1.0-preview.1", "0.1.0-preview.1", true).unwrap();
        assert_eq!((p.updater_key, p.binaries), (UpdaterKey::Release, true));
    }

    #[test]
    fn a_rerun_resumes_a_draft_but_never_touches_a_published_release() {
        assert_eq!(publish_action("none").unwrap(), "create");
        assert_eq!(publish_action("draft").unwrap(), "resume");
        let e = publish_action("published").unwrap_err();
        assert!(e.to_string().contains("left unchanged"), "{e}");
        assert!(publish_action("").is_err());
    }

    /// 데스크톱 앱이 이 크레이트와 같은 워크스페이스 버전을 쓴다 — 그래야 [`SOURCE_VERSION`] 이 앱의 버전이다.
    #[test]
    fn the_desktop_app_uses_the_workspace_version() {
        let manifest = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../apps/desktop/src-tauri/Cargo.toml"
        ))
        .unwrap();
        assert!(
            manifest
                .lines()
                .any(|l| l.trim() == "version.workspace = true"),
            "apps/desktop/src-tauri/Cargo.toml 이 version.workspace = true 가 아니다"
        );
        let conf = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../apps/desktop/src-tauri/tauri.conf.json"
        ))
        .unwrap();
        let conf: Value = serde_json::from_str(&conf).unwrap();
        assert!(
            conf.get("version").is_none(),
            "tauri.conf.json 의 version 은 Cargo 버전을 가린다"
        );
    }

    fn artifacts(names: &[&str]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for name in names {
            let path = dir.path().join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, b"bytes").unwrap();
            std::fs::write(
                format!("{}.sig", path.display()),
                format!("sig-of-{}\n", path.file_name().unwrap().to_str().unwrap()),
            )
            .unwrap();
        }
        dir
    }

    const ALL: &[&str] = &[
        "macos/Pitwall.app.tar.gz",
        "linux/Pitwall_0.1.0-preview.1_amd64.AppImage",
        "windows/Pitwall_0.1.0_x64_en-US.msi",
    ];

    #[test]
    fn the_manifest_lists_every_platform_with_its_signature() {
        let dir = artifacts(ALL);
        let p = plan("refs/tags/v0.1.0-preview.1", "0.1.0-preview.1", true).unwrap();
        let m = manifest(&p, "o/r", dir.path(), "notes", "2026-10-19T00:00:00Z").unwrap();
        assert_eq!(m["version"], "0.1.0-preview.1");
        assert_eq!(m["pub_date"], "2026-10-19T00:00:00Z");
        let platforms = m["platforms"].as_object().unwrap();
        let mut keys: Vec<_> = platforms.keys().cloned().collect();
        keys.sort();
        assert_eq!(
            keys,
            [
                "darwin-aarch64",
                "darwin-x86_64",
                "linux-x86_64-appimage",
                "windows-x86_64"
            ]
        );
        assert_eq!(platforms["darwin-x86_64"], platforms["darwin-aarch64"]);
        assert_eq!(
            platforms["darwin-aarch64"]["url"],
            "https://github.com/o/r/releases/download/v0.1.0-preview.1/Pitwall.app.tar.gz"
        );
        assert_eq!(
            platforms["darwin-aarch64"]["signature"],
            "sig-of-Pitwall.app.tar.gz"
        );
        assert_eq!(
            platforms["windows-x86_64"]["url"],
            "https://github.com/o/r/releases/download/v0.1.0-preview.1/Pitwall_0.1.0_x64_en-US.msi"
        );
        assert!(
            platforms.get("linux-x86_64").is_none(),
            "deb · rpm 설치본이 AppImage 로 떨어지지 않게"
        );
    }

    #[test]
    fn a_missing_or_duplicated_platform_fails() {
        let p = plan("refs/pull/1/merge", "0.1.0", true).unwrap();
        let dir = artifacts(&ALL[..2]);
        let e = manifest(&p, "o/r", dir.path(), "", "d").unwrap_err();
        assert!(e.to_string().contains(".msi"), "{e}");
        let mut two = ALL.to_vec();
        two.push("more/Other.app.tar.gz");
        let dir = artifacts(&two);
        assert!(manifest(&p, "o/r", dir.path(), "", "d").is_err());
    }

    #[test]
    fn a_missing_or_empty_signature_fails() {
        let p = plan("refs/pull/1/merge", "0.1.0", true).unwrap();
        let dir = artifacts(ALL);
        std::fs::remove_file(dir.path().join("windows/Pitwall_0.1.0_x64_en-US.msi.sig")).unwrap();
        assert!(manifest(&p, "o/r", dir.path(), "", "d").is_err());
        let dir = artifacts(ALL);
        std::fs::write(dir.path().join("macos/Pitwall.app.tar.gz.sig"), " \n").unwrap();
        assert!(manifest(&p, "o/r", dir.path(), "", "d").is_err());
    }

    #[test]
    fn asset_names_must_survive_the_upload_unchanged() {
        assert!(plain_name("Pitwall_0.1.0-preview.1_amd64.AppImage"));
        assert!(!plain_name("Pitwall 0.1.0.msi"));
        assert!(!plain_name("Pitwall+x.msi"));
    }
}
