//! 설치 대상(플랫폼). Mason registry 의 `target` 값(`darwin_arm64` · `linux_x64` · `unix` 등)을 Pitwall 이 지원하는 여섯 대상에 맞춘다
//! (15-m4-plan.md M4-03 ⑥).

use serde::{Deserialize, Serialize};

/// Pitwall 이 도구를 설치하는 대상. 이 밖의 기기에서는 [`ToolTarget::current`] 가 `None` 이고 모든 패키지가 「지원하지 않음」 이다.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    specta::Type,
    schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum ToolTarget {
    DarwinArm64,
    DarwinX64,
    LinuxX64Gnu,
    LinuxArm64Gnu,
    LinuxX64Musl,
    WinX64,
}

impl ToolTarget {
    pub const ALL: [ToolTarget; 6] = [
        ToolTarget::DarwinArm64,
        ToolTarget::DarwinX64,
        ToolTarget::LinuxX64Gnu,
        ToolTarget::LinuxArm64Gnu,
        ToolTarget::LinuxX64Musl,
        ToolTarget::WinX64,
    ];

    /// 이 바이너리가 빌드된 기기의 대상.
    pub fn current() -> Option<ToolTarget> {
        if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
            Some(ToolTarget::DarwinArm64)
        } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
            Some(ToolTarget::DarwinX64)
        } else if cfg!(all(
            target_os = "linux",
            target_arch = "x86_64",
            target_env = "gnu"
        )) {
            Some(ToolTarget::LinuxX64Gnu)
        } else if cfg!(all(
            target_os = "linux",
            target_arch = "aarch64",
            target_env = "gnu"
        )) {
            Some(ToolTarget::LinuxArm64Gnu)
        } else if cfg!(all(
            target_os = "linux",
            target_arch = "x86_64",
            target_env = "musl"
        )) {
            Some(ToolTarget::LinuxX64Musl)
        } else if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
            Some(ToolTarget::WinX64)
        } else {
            None
        }
    }

    /// `registry.toml` · 화면에 쓰는 이름(Mason 의 가장 좁은 이름과 같다).
    pub fn as_str(self) -> &'static str {
        match self {
            ToolTarget::DarwinArm64 => "darwin_arm64",
            ToolTarget::DarwinX64 => "darwin_x64",
            ToolTarget::LinuxX64Gnu => "linux_x64_gnu",
            ToolTarget::LinuxArm64Gnu => "linux_arm64_gnu",
            ToolTarget::LinuxX64Musl => "linux_x64_musl",
            ToolTarget::WinX64 => "win_x64",
        }
    }

    pub fn parse(s: &str) -> Option<ToolTarget> {
        ToolTarget::ALL.into_iter().find(|t| t.as_str() == s)
    }

    /// 이 대상이 만족하는 Mason 플랫폼 이름. Mason 처럼 넓은 이름(`linux` · `unix`)도 맞는다. `linux_x64` 는 libc 를 가리지
    /// 않으므로 gnu · musl 둘 다 맞는다(ruff 의 `linux_x64` 항목은 musl 파일이지만 gnu 에서도 돈다 — Mason 도 같게 고른다).
    pub fn mason_names(self) -> &'static [&'static str] {
        match self {
            ToolTarget::DarwinArm64 => &["darwin_arm64", "darwin", "unix"],
            ToolTarget::DarwinX64 => &["darwin_x64", "darwin", "unix"],
            ToolTarget::LinuxX64Gnu => &["linux_x64_gnu", "linux_x64", "linux", "unix"],
            ToolTarget::LinuxArm64Gnu => &["linux_arm64_gnu", "linux_arm64", "linux", "unix"],
            ToolTarget::LinuxX64Musl => &["linux_x64_musl", "linux_x64", "linux", "unix"],
            ToolTarget::WinX64 => &["win_x64", "win"],
        }
    }

    /// Mason 의 `target` 값 하나가 이 대상에 맞는가.
    pub fn matches(self, mason: &str) -> bool {
        self.mason_names().contains(&mason)
    }
}

impl std::fmt::Display for ToolTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip_and_widen_like_mason() {
        for t in ToolTarget::ALL {
            assert_eq!(ToolTarget::parse(t.as_str()), Some(t));
            assert!(t.matches(t.as_str()));
        }
        assert!(ToolTarget::LinuxX64Gnu.matches("linux_x64"));
        assert!(ToolTarget::LinuxX64Musl.matches("linux_x64"));
        assert!(!ToolTarget::LinuxX64Musl.matches("linux_x64_gnu"));
        assert!(!ToolTarget::LinuxX64Gnu.matches("linux_x64_musl"));
        assert!(ToolTarget::DarwinArm64.matches("unix"));
        assert!(!ToolTarget::WinX64.matches("unix"));
        assert!(!ToolTarget::DarwinArm64.matches("darwin_x64"));
        assert!(!ToolTarget::LinuxArm64Gnu.matches("linux_arm64_musl"));
        assert_eq!(ToolTarget::parse("linux_x64"), None);
    }

    #[test]
    fn this_machine_has_a_target_on_the_platforms_we_build() {
        // CI · 개발 기기(macOS · ubuntu x64 · Windows x64).
        if cfg!(any(
            target_os = "macos",
            all(target_os = "windows", target_arch = "x86_64"),
            all(
                target_os = "linux",
                target_arch = "x86_64",
                target_env = "gnu"
            )
        )) {
            assert!(ToolTarget::current().is_some());
        }
    }
}
