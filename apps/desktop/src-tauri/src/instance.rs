//! 단일 인스턴스의 키(docs/design/03-architecture.md §3 「단일 인스턴스」).
//!
//! `tauri-plugin-single-instance` 는 앱 identifier 하나로 기존 인스턴스를 찾는다(2.5.4 macOS: `/tmp/<identifier>_si.sock`,
//! Windows: 같은 이름의 뮤텍스, Linux: D-Bus 이름). 그대로 두면 이 저장소처럼 여러 워크트리가 dev 앱을 동시에 띄울 때 A
//! 워크트리에서 실행한 `pitwall` 이 B 워크트리의 앱으로 전달된다. 그래서 **debug 빌드는 identifier 에 「어느 빌드 ·
//! 어느 데이터」를 붙여** 키를 가른다. **release 빌드는 기본 `$DATA` 면 identifier 그대로 하나이고, `PITWALL_DATA_DIR` 로
//! 기본이 아닌 `$DATA` 를 주면 그 `$DATA` 의 해시를 붙인다**([`release_identifier`]) — 임시 데이터로 띄운 release 빌드(성능
//! 측정 · 시험)가 정식 앱과 인자를 주고받지 않는다. 인자를 넘기는 쪽(두 번째 실행)과 받는 쪽(앱)은 같은 실행 파일의 같은
//! 함수(main.rs 의 `single_instance_key`)로 키를 계산하므로, 같은 `PITWALL_DATA_DIR` 를 줘야 서로 닿는다.
//!
//! - 빌드: 컴파일한 크레이트 폴더(`CARGO_MANIFEST_DIR`). 바이너리에 박히므로 어떻게 실행했든(심링크 · 상대 경로 ·
//!   Orca 의 `spawn`) 같은 빌드면 같다. 워크트리마다 다르다.
//! - 데이터: `$DATA`(정규화). 같은 워크트리에서 임시 데이터 폴더로 띄운 시험 앱이 사람의 앱으로 전달되지 않게 한다.
//!   `PITWALL_DATA_DIR` 없이 실행한 `pitwall` 은 기본 `$DATA` 의 앱에만 닿는다.
//!
//! identifier 를 바꾸면 Tauri 가 그것으로 정하는 경로도 바뀐다 — Windows · Linux 의 웹뷰 데이터 폴더(localStorage)가
//! 키마다 따로 생긴다. macOS WKWebView 는 identifier 를 쓰지 않고 번들 identifier 로 저장소를 정하므로, 기본이 아닌
//! `$DATA` 로 뜬 앱(debug 포함)은 그 `$DATA` 로 정한 별도 저장소를 쓴다([`webview_store_id`], macOS 14+ — 그보다 낮으면
//! wry 가 조용히 기본 저장소를 쓴다). 앱 데이터는 XDG 경로(`AppPaths`)라 영향이 없다.

use std::path::{Path, PathBuf};

/// 시험이 띄우는 바이너리의 키를 시험 전용으로 가르는 접미사(`tests/cli.rs`). 시험이 실패해 그 바이너리가 앱이 되어도 사람의
/// 앱(같은 빌드 · 같은 키)으로 전달되거나 사람의 키로 소켓을 남기지 않게 한다. **debug 빌드에서만 듣는다** — release 바이너리는
/// 변수가 있어도 키를 바꾸지 않으므로 사람이 우연히 정식 앱의 단일 인스턴스를 가를 수 없다.
pub const TEST_KEY_ENV: &str = "PITWALL_TEST_INSTANCE";

/// [`TEST_KEY_ENV`] 를 붙인 키. 접미사는 영숫자 1~16자만 받는다(소켓 경로 길이 · D-Bus 이름 규칙). 아니면 그대로.
pub fn with_test_suffix(identifier: String, suffix: Option<&str>, debug: bool) -> String {
    match suffix {
        Some(s)
            if debug
                && (1..=16).contains(&s.len())
                && s.bytes().all(|b| b.is_ascii_alphanumeric()) =>
        {
            format!("{identifier}.test-{s}")
        }
        _ => identifier,
    }
}

/// 단일 인스턴스 키로 쓸 identifier. `dev` 가 `None` 이면(release) `base` 그대로.
pub fn identifier(base: &str, dev: Option<(&Path, &Path)>) -> String {
    match dev {
        None => base.to_owned(),
        Some((build, data)) => {
            let mut hash = Fnv::new();
            hash.write(build.as_os_str().as_encoded_bytes());
            hash.write(&[0]);
            hash.write(data.as_os_str().as_encoded_bytes());
            // D-Bus 이름의 요소는 숫자로 시작할 수 없어 `dev-` 를 앞에 둔다. macOS 소켓 경로(104바이트 제한)도 짧게 남는다.
            format!("{base}.dev-{:016x}", hash.0)
        }
    }
}

/// 기본이 아닌 `$DATA`(정규화한 값). 기본 `$DATA`(`default`, 모르면 `None`)와 정규화해 같으면 `None` — 기본 자리를
/// `PITWALL_DATA_DIR` 로 다시 적어도(심링크 · 끝의 `/` · `..`) 정식 앱과 같은 키 · 같은 웹뷰 저장소다.
pub fn custom_data(data: &Path, default: Option<&Path>) -> Option<PathBuf> {
    // 아직 없는 폴더도 만들어진 뒤와 같은 값이 나오게 레지스트리와 같은 정규화를 쓴다.
    let normalize =
        |p: &Path| pw_core::registry::path::normalize(p).unwrap_or_else(|_| p.to_path_buf());
    let data = normalize(data);
    match default {
        Some(default) if normalize(default) == data => None,
        _ => Some(data),
    }
}

/// release 빌드의 키(머리 주석). `custom_data` 는 [`custom_data`] 의 값.
pub fn release_identifier(base: &str, custom_data: Option<&Path>) -> String {
    match custom_data {
        None => base.to_owned(),
        Some(data) => {
            let mut hash = Fnv::new();
            hash.write(data.as_os_str().as_encoded_bytes());
            format!("{base}.data-{:016x}", hash.0)
        }
    }
}

/// 기본이 아닌 `$DATA` 로 뜬 앱의 웹뷰 저장소 id(WKWebView `data_store_identifier`). 같은 `$DATA` 면 실행마다 같다.
pub fn webview_store_id(custom_data: &Path) -> [u8; 16] {
    let half = |seed: u8| {
        let mut hash = Fnv::new();
        hash.write(&[seed]);
        hash.write(custom_data.as_os_str().as_encoded_bytes());
        hash.0.to_be_bytes()
    };
    let mut id = [0; 16];
    id[..8].copy_from_slice(&half(1));
    id[8..].copy_from_slice(&half(2));
    id
}

/// UUID 문자열(대문자, `8-4-4-4-12`). WebKit 이 저장소 폴더 이름으로 쓰는 형식이다 — 측정 하네스가 로그의 이 값으로 그
/// 폴더를 찾아 지운다(`crates/pw-perf`).
pub fn uuid_string(id: [u8; 16]) -> String {
    let hex: String = id.iter().map(|b| format!("{b:02X}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    )
}

/// 실행마다 같은 값을 내야 하므로(두 프로세스가 같은 키를 계산한다) 표준 라이브러리의 무작위 해시가 아닌 FNV-1a.
struct Fnv(u64);

impl Fnv {
    fn new() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }

    fn write(&mut self, bytes: &[u8]) {
        for b in bytes {
            self.0 ^= u64::from(*b);
            self.0 = self.0.wrapping_mul(0x0100_0000_01b3);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = "dev.pitwall.desktop";

    fn dev(build: &str, data: &str) -> String {
        identifier(BASE, Some((Path::new(build), Path::new(data))))
    }

    #[test]
    fn release_uses_the_app_identifier() {
        assert_eq!(identifier(BASE, None), BASE);
    }

    #[test]
    fn dev_key_depends_on_build_and_data() {
        let a = dev(
            "/repo/.worktrees/a/apps/desktop/src-tauri",
            "/home/me/.local/share/pitwall",
        );
        assert_eq!(
            a,
            dev(
                "/repo/.worktrees/a/apps/desktop/src-tauri",
                "/home/me/.local/share/pitwall"
            ),
            "같은 빌드 · 같은 데이터면 같은 키"
        );
        assert_ne!(
            a,
            dev(
                "/repo/.worktrees/b/apps/desktop/src-tauri",
                "/home/me/.local/share/pitwall"
            ),
            "워크트리가 다르면 다른 키"
        );
        assert_ne!(
            a,
            dev(
                "/repo/.worktrees/a/apps/desktop/src-tauri",
                "/tmp/test-data"
            ),
            "데이터 폴더가 다르면 다른 키"
        );
        assert_ne!(a, BASE);
        assert!(a.starts_with("dev.pitwall.desktop.dev-"));
    }

    #[test]
    fn a_test_suffix_splits_the_key_only_in_debug_builds() {
        let key = || "dev.pitwall.desktop".to_owned();
        assert_eq!(
            with_test_suffix(key(), Some("cli42"), true),
            "dev.pitwall.desktop.test-cli42"
        );
        assert_eq!(
            with_test_suffix(key(), Some("cli42"), false),
            "dev.pitwall.desktop",
            "release 는 듣지 않는다"
        );
        for bad in ["", "a.b", "../x", "12345678901234567"] {
            assert_eq!(with_test_suffix(key(), Some(bad), true), key(), "{bad:?}");
        }
        assert_eq!(with_test_suffix(key(), None, true), key());
    }

    /// 인자를 넘기는 쪽과 받는 쪽은 각자 `PITWALL_DATA_DIR` 를 정규화해 키를 계산한다. 같은 폴더를 다르게 적어도(심링크 ·
    /// 끝의 `/` · `..`) 같은 키, 기본 자리를 다시 적으면 정식 앱의 키다.
    #[test]
    fn release_keys_follow_the_normalized_data_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let real = std::fs::canonicalize(tmp.path()).unwrap();
        let default = real.join("default");
        let custom = real.join("perf").join("data");
        std::fs::create_dir_all(&default).unwrap();
        std::fs::create_dir_all(&custom).unwrap();
        let key =
            |data: &Path| release_identifier(BASE, custom_data(data, Some(&default)).as_deref());

        assert_eq!(key(&default), BASE, "기본 `$DATA` 는 정식 앱의 키");
        assert_eq!(key(&real.join("perf/../default/")), BASE);
        let perf = key(&custom);
        assert!(perf.starts_with("dev.pitwall.desktop.data-"), "{perf}");
        #[cfg(unix)]
        {
            let link = real.join("link");
            std::os::unix::fs::symlink(real.join("perf"), &link).unwrap();
            assert_eq!(key(&link.join("data")), perf, "심링크로 적어도 같은 키");
        }
        assert_eq!(key(Path::new(&format!("{}/", custom.display()))), perf);
        assert_ne!(key(&real.join("other")), perf);
        assert_eq!(
            release_identifier(BASE, custom_data(&custom, None).as_deref()),
            perf,
            "기본 `$DATA` 를 모르면 덮어쓴 값을 기본이 아닌 것으로 본다"
        );
    }

    #[test]
    fn webview_stores_differ_per_data_dir_and_read_as_uuids() {
        let a = webview_store_id(Path::new("/tmp/a"));
        assert_eq!(a, webview_store_id(Path::new("/tmp/a")));
        assert_ne!(a, webview_store_id(Path::new("/tmp/b")));
        assert_ne!(a[..8], a[8..]);
        let text = uuid_string([0xab; 16]);
        assert_eq!(text, "ABABABAB-ABAB-ABAB-ABAB-ABABABABABAB");
    }

    #[test]
    fn the_separator_keeps_build_and_data_apart() {
        assert_ne!(dev("/a", "/b/c"), dev("/a/b", "/c"));
    }
}
