//! 단일 인스턴스의 키(docs/design/03-architecture.md §3 「단일 인스턴스」).
//!
//! `tauri-plugin-single-instance` 는 앱 identifier 하나로 기존 인스턴스를 찾는다(2.5.4 macOS: `/tmp/<identifier>_si.sock`,
//! Windows: 같은 이름의 뮤텍스, Linux: D-Bus 이름). 그대로 두면 이 저장소처럼 여러 워크트리가 dev 앱을 동시에 띄울 때 A
//! 워크트리에서 실행한 `pitwall` 이 B 워크트리의 앱으로 전달된다. 그래서 **debug 빌드는 identifier 에 「어느 빌드 ·
//! 어느 데이터」를 붙여** 키를 가른다. release 빌드는 identifier 그대로 하나다.
//!
//! - 빌드: 컴파일한 크레이트 폴더(`CARGO_MANIFEST_DIR`). 바이너리에 박히므로 어떻게 실행했든(심링크 · 상대 경로 ·
//!   Orca 의 `spawn`) 같은 빌드면 같다. 워크트리마다 다르다.
//! - 데이터: `$DATA`(정규화). 같은 워크트리에서 임시 데이터 폴더로 띄운 시험 앱이 사람의 앱으로 전달되지 않게 한다.
//!   `PITWALL_DATA_DIR` 없이 실행한 `pitwall` 은 기본 `$DATA` 의 앱에만 닿는다.
//!
//! identifier 를 바꾸면 Tauri 가 그것으로 정하는 경로도 바뀐다 — Windows · Linux 의 웹뷰 데이터 폴더(localStorage)가
//! dev 키마다 따로 생긴다. macOS WKWebView 는 identifier 를 쓰지 않는다. 앱 데이터는 XDG 경로(`AppPaths`)라 영향이 없다.

use std::path::Path;

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
    fn the_separator_keeps_build_and_data_apart() {
        assert_ne!(dev("/a", "/b/c"), dev("/a/b", "/c"));
    }
}
