//! 앱 파일 위치(`$CONFIG`/`$DATA`/`$STATE`/`$CACHE`) 해석 — `docs/design/03-architecture.md` 「5-0. 파일 위치 — XDG 규칙」.
//!
//! 해석은 [`PathEnv`] 를 받는 순수 함수([`AppPaths::resolve`])다. 실제 환경 변수 · 홈 디렉터리는
//! [`PathEnv::current`] 만 읽는다. 테스트는 `PathEnv` 를 직접 채워 실제 `HOME` · 환경 변수를 건드리지 않는다.

use std::collections::HashMap;
use std::ffi::OsString;
use std::fmt;
use std::path::{Path, PathBuf};

/// XDG 기본 디렉터리 아래에 두는 앱 폴더 이름.
pub const APP_DIR: &str = "pitwall";

/// 해석된 네 디렉터리. 존재 여부는 보지 않는다 — 쓰는 쪽이 필요할 때 만든다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppPaths {
    /// 사람이 편집하는 TOML 만 둔다(dotfiles 로 동기화 가능해야 한다).
    pub config: PathBuf,
    pub data: PathBuf,
    pub state: PathBuf,
    pub cache: PathBuf,
}

/// 경로 규칙이 갈리는 OS 계열. macOS 는 [`OsFamily::Unix`] — `~/Library` 대신 XDG 를 따른다(D27).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OsFamily {
    Unix,
    Windows,
}

impl OsFamily {
    pub fn current() -> Self {
        if cfg!(windows) {
            OsFamily::Windows
        } else {
            OsFamily::Unix
        }
    }
}

/// 경로 해석의 입력 전부.
#[derive(Debug, Clone)]
pub struct PathEnv {
    pub os: OsFamily,
    /// 환경 변수. `PITWALL_*_DIR` 와 (Unix 에서) `XDG_*_HOME` 만 본다.
    pub vars: HashMap<String, OsString>,
    /// Unix 의 홈 디렉터리. `XDG_*_HOME` 이 없을 때의 기본값을 만든다.
    pub home: Option<PathBuf>,
    /// Windows 의 `%APPDATA%`(Roaming).
    pub roaming_app_data: Option<PathBuf>,
    /// Windows 의 `%LOCALAPPDATA%`.
    pub local_app_data: Option<PathBuf>,
}

impl PathEnv {
    /// 아무 값도 없는 입력. 테스트가 필요한 값만 채운다.
    pub fn empty(os: OsFamily) -> Self {
        Self {
            os,
            vars: HashMap::new(),
            home: None,
            roaming_app_data: None,
            local_app_data: None,
        }
    }

    #[must_use]
    pub fn var(mut self, name: &str, value: impl Into<OsString>) -> Self {
        self.vars.insert(name.to_owned(), value.into());
        self
    }

    /// 이 프로세스의 실제 환경. 홈 · Windows 표준 폴더는 `etcetera` 가 찾는다(Windows 는 환경 변수가 없으면 Known Folder API).
    pub fn current() -> Self {
        use etcetera::BaseStrategy;

        let os = OsFamily::current();
        let names = Dir::ALL
            .iter()
            .flat_map(|d| [d.override_var(), d.xdg_var()]);
        let vars = names
            .filter_map(|name| std::env::var_os(name).map(|v| (name.to_owned(), v)))
            .collect();
        let mut env = Self {
            os,
            vars,
            home: etcetera::home_dir().ok(),
            roaming_app_data: None,
            local_app_data: None,
        };
        if os == OsFamily::Windows
            && let Ok(win) = etcetera::base_strategy::Windows::new()
        {
            // etcetera 의 Windows 규칙: config_dir 는 Roaming, cache_dir 는 Local AppData 다.
            env.roaming_app_data = Some(win.config_dir());
            env.local_app_data = Some(win.cache_dir());
        }
        env
    }

    /// 비어 있는 값은 없는 것으로 본다(`FOO= cmd` 로 끄는 관례).
    fn get(&self, name: &str) -> Option<&OsString> {
        self.vars.get(name).filter(|v| !v.is_empty())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathError {
    /// `PITWALL_*_DIR` 가 상대 경로다. 실행 위치에 따라 다른 폴더를 가리키게 되므로 받지 않는다.
    RelativeOverride { var: &'static str, value: PathBuf },
    /// Unix 에서 `XDG_*_HOME` 도 홈 디렉터리도 없다.
    NoHome,
    /// Windows 에서 `%APPDATA%` · `%LOCALAPPDATA%` 를 찾지 못했다.
    NoAppData,
}

impl fmt::Display for PathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PathError::RelativeOverride { var, value } => {
                write!(
                    f,
                    "{var} must be an absolute path, got `{}`",
                    value.display()
                )
            }
            PathError::NoHome => f.write_str("cannot determine the home directory"),
            PathError::NoAppData => f.write_str("cannot determine %APPDATA% / %LOCALAPPDATA%"),
        }
    }
}

impl std::error::Error for PathError {}

#[derive(Debug, Clone, Copy)]
enum Dir {
    Config,
    Data,
    State,
    Cache,
}

impl Dir {
    const ALL: [Dir; 4] = [Dir::Config, Dir::Data, Dir::State, Dir::Cache];

    fn override_var(self) -> &'static str {
        match self {
            Dir::Config => "PITWALL_CONFIG_DIR",
            Dir::Data => "PITWALL_DATA_DIR",
            Dir::State => "PITWALL_STATE_DIR",
            Dir::Cache => "PITWALL_CACHE_DIR",
        }
    }

    fn xdg_var(self) -> &'static str {
        match self {
            Dir::Config => "XDG_CONFIG_HOME",
            Dir::Data => "XDG_DATA_HOME",
            Dir::State => "XDG_STATE_HOME",
            Dir::Cache => "XDG_CACHE_HOME",
        }
    }

    /// `XDG_*_HOME` 이 없을 때 홈 아래 기본값(XDG Base Directory 명세).
    fn xdg_default(self) -> &'static [&'static str] {
        match self {
            Dir::Config => &[".config"],
            Dir::Data => &[".local", "share"],
            Dir::State => &[".local", "state"],
            Dir::Cache => &[".cache"],
        }
    }

    fn resolve(self, env: &PathEnv) -> Result<PathBuf, PathError> {
        if let Some(value) = env.get(self.override_var()) {
            let path = PathBuf::from(value);
            if !path.is_absolute() {
                return Err(PathError::RelativeOverride {
                    var: self.override_var(),
                    value: path,
                });
            }
            return Ok(path);
        }
        match env.os {
            OsFamily::Unix => {
                // 명세대로 상대 경로인 XDG 값은 무효로 보고 기본값으로 간다.
                let xdg = env
                    .get(self.xdg_var())
                    .map(PathBuf::from)
                    .filter(|p| p.is_absolute());
                let base = match xdg {
                    Some(base) => base,
                    None => {
                        let home = env.home.as_deref().ok_or(PathError::NoHome)?;
                        self.xdg_default()
                            .iter()
                            .fold(home.to_path_buf(), |p, c| p.join(c))
                    }
                };
                Ok(base.join(APP_DIR))
            }
            OsFamily::Windows => {
                let (base, leaf) = match self {
                    Dir::Config => (&env.roaming_app_data, "config"),
                    Dir::Data => (&env.local_app_data, "data"),
                    Dir::State => (&env.local_app_data, "state"),
                    Dir::Cache => (&env.local_app_data, "cache"),
                };
                let base = base.as_deref().ok_or(PathError::NoAppData)?;
                Ok(base.join(APP_DIR).join(leaf))
            }
        }
    }
}

impl AppPaths {
    /// 우선순위: `PITWALL_*_DIR` → (Unix) `XDG_*_HOME` + `/pitwall` → (Unix) 홈 아래 XDG 기본값 + `/pitwall`.
    /// Windows 는 `XDG_*` 를 보지 않고 `%APPDATA%\pitwall\config`, `%LOCALAPPDATA%\pitwall\{data,state,cache}` 다.
    pub fn resolve(env: &PathEnv) -> Result<Self, PathError> {
        Ok(Self {
            config: Dir::Config.resolve(env)?,
            data: Dir::Data.resolve(env)?,
            state: Dir::State.resolve(env)?,
            cache: Dir::Cache.resolve(env)?,
        })
    }

    /// 이 프로세스의 실제 환경으로 해석한다.
    pub fn from_env() -> Result<Self, PathError> {
        Self::resolve(&PathEnv::current())
    }

    /// `PITWALL_DATA_DIR` 를 보지 않은 `$DATA` — 덮어쓰지 않은 실행이 쓰는 기본 자리. 데스크톱 앱이 「기본이 아닌
    /// `$DATA` 로 떴는가」 를 가르는 데 쓴다(apps/desktop/src-tauri/src/instance.rs).
    pub fn default_data(env: &PathEnv) -> Result<PathBuf, PathError> {
        let mut env = env.clone();
        env.vars.remove(Dir::Data.override_var());
        Dir::Data.resolve(&env)
    }

    /// 네 디렉터리를 한 루트 아래에 둔다. 테스트 · 포터블 실행용.
    pub fn under(root: &Path) -> Self {
        Self {
            config: root.join("config"),
            data: root.join("data"),
            state: root.join("state"),
            cache: root.join("cache"),
        }
    }

    /// 전역 설정 파일의 마이그레이션 원본 보관 위치.
    pub fn backup_dir(&self) -> PathBuf {
        self.state.join("backup")
    }

    /// 「개인 전용」 프로젝트의 설정 폴더(`.pitwall/` 과 같은 구조). slug 검사는 [`super::ConfigSource::personal`] 이 한다.
    pub fn personal_project_dir(&self, slug: &str) -> PathBuf {
        self.config.join("projects").join(slug)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unix_home() -> PathEnv {
        let mut env = PathEnv::empty(OsFamily::Unix);
        env.home = Some(PathBuf::from("/home/u"));
        env
    }

    #[test]
    fn unix_defaults_follow_xdg_under_home() {
        let paths = AppPaths::resolve(&unix_home()).unwrap();
        assert_eq!(paths.config, Path::new("/home/u/.config/pitwall"));
        assert_eq!(paths.data, Path::new("/home/u/.local/share/pitwall"));
        assert_eq!(paths.state, Path::new("/home/u/.local/state/pitwall"));
        assert_eq!(paths.cache, Path::new("/home/u/.cache/pitwall"));
    }

    #[test]
    fn the_default_data_dir_ignores_only_the_data_override() {
        let env = unix_home()
            .var("PITWALL_DATA_DIR", "/override/data")
            .var("XDG_DATA_HOME", "/xdg/data");
        assert_eq!(
            AppPaths::resolve(&env).unwrap().data,
            Path::new("/override/data")
        );
        assert_eq!(
            AppPaths::default_data(&env).unwrap(),
            Path::new("/xdg/data/pitwall")
        );
        assert_eq!(
            AppPaths::default_data(&unix_home()).unwrap(),
            Path::new("/home/u/.local/share/pitwall")
        );
    }

    #[test]
    fn xdg_vars_beat_home_and_pitwall_vars_beat_xdg() {
        let env = unix_home()
            .var("XDG_CONFIG_HOME", "/xdg/config")
            .var("XDG_STATE_HOME", "/xdg/state")
            .var("PITWALL_STATE_DIR", "/override/state")
            .var("PITWALL_CACHE_DIR", "/override/cache");
        let paths = AppPaths::resolve(&env).unwrap();
        assert_eq!(paths.config, Path::new("/xdg/config/pitwall"));
        assert_eq!(paths.data, Path::new("/home/u/.local/share/pitwall"));
        // PITWALL_*_DIR 는 그 디렉터리 자체다(`pitwall` 을 붙이지 않는다).
        assert_eq!(paths.state, Path::new("/override/state"));
        assert_eq!(paths.cache, Path::new("/override/cache"));
    }

    #[test]
    fn empty_and_relative_xdg_values_are_ignored() {
        let env = unix_home()
            .var("XDG_CONFIG_HOME", "")
            .var("XDG_DATA_HOME", "relative/data");
        let paths = AppPaths::resolve(&env).unwrap();
        assert_eq!(paths.config, Path::new("/home/u/.config/pitwall"));
        assert_eq!(paths.data, Path::new("/home/u/.local/share/pitwall"));
    }

    #[test]
    fn relative_pitwall_override_is_an_error() {
        let env = unix_home().var("PITWALL_DATA_DIR", "rel");
        assert_eq!(
            AppPaths::resolve(&env),
            Err(PathError::RelativeOverride {
                var: "PITWALL_DATA_DIR",
                value: PathBuf::from("rel"),
            })
        );
    }

    #[test]
    fn overrides_alone_need_no_home() {
        let env = PathEnv::empty(OsFamily::Unix)
            .var("PITWALL_CONFIG_DIR", "/p/c")
            .var("PITWALL_DATA_DIR", "/p/d")
            .var("PITWALL_STATE_DIR", "/p/s")
            .var("PITWALL_CACHE_DIR", "/p/k");
        assert!(AppPaths::resolve(&env).is_ok());
        assert_eq!(
            AppPaths::resolve(&PathEnv::empty(OsFamily::Unix)),
            Err(PathError::NoHome)
        );
    }

    /// Windows 매핑은 실제 Windows 에서 돌려 보지 않았다 — 이 표가 설계 §5-0 과 같은지만 고정한다.
    #[test]
    fn windows_maps_to_appdata_and_ignores_xdg() {
        let mut env = PathEnv::empty(OsFamily::Windows).var("XDG_CONFIG_HOME", "/xdg");
        env.roaming_app_data = Some(PathBuf::from(r"C:\Users\u\AppData\Roaming"));
        env.local_app_data = Some(PathBuf::from(r"C:\Users\u\AppData\Local"));
        let paths = AppPaths::resolve(&env).unwrap();
        let roaming = Path::new(r"C:\Users\u\AppData\Roaming");
        let local = Path::new(r"C:\Users\u\AppData\Local");
        assert_eq!(paths.config, roaming.join("pitwall").join("config"));
        assert_eq!(paths.data, local.join("pitwall").join("data"));
        assert_eq!(paths.state, local.join("pitwall").join("state"));
        assert_eq!(paths.cache, local.join("pitwall").join("cache"));

        let env = env.var("PITWALL_CONFIG_DIR", r"C:\portable\config");
        if cfg!(windows) {
            // Windows 경로의 절대 여부는 Windows 에서만 판정된다.
            assert_eq!(
                AppPaths::resolve(&env).unwrap().config,
                Path::new(r"C:\portable\config")
            );
        }
        assert_eq!(
            AppPaths::resolve(&PathEnv::empty(OsFamily::Windows)),
            Err(PathError::NoAppData)
        );
    }
}
