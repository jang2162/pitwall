//! `roots` 밖은 프로젝트로 등록하지 않는다(18-m7-plan M7-04 ③ · 메인 판정 M7-P5 — 「Relay 가 노출하는 프로젝트 경로는
//! 화이트리스트」, 08). 레지스트리의 모든 등록이 지나는 `Registry::create` 가 심링크 · `..` 를 푼 실제 경로로 묻는다 — `roots` 안의
//! 심링크가 밖을 가리켜도 그 실제 경로는 밖이라 거절된다. `roots` 자체도 같은 규칙(`pw_core::registry::path::normalize`)으로 푼다.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use pw_core::registry::RootPolicy;
use pw_core::registry::path::normalize;

/// 정규화한 `roots`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Roots {
    allowed: Vec<PathBuf>,
}

impl Roots {
    /// `relay.toml` 의 `roots`. 절대 경로가 아닌 것은 버린다(설정 읽기가 이미 거절한다).
    pub fn new(roots: &[PathBuf]) -> Self {
        Self {
            allowed: roots.iter().filter_map(|r| normalize(r).ok()).collect(),
        }
    }

    pub fn allowed(&self) -> &[PathBuf] {
        &self.allowed
    }

    /// 정규화한 실제 경로 `real` 이 `roots` 중 하나와 같거나 그 아래인가(구성 요소 단위).
    pub fn allows(&self, real: &Path) -> bool {
        self.allowed.iter().any(|root| real.starts_with(root))
    }

    /// 레지스트리에 넣는 판정.
    pub fn policy(self) -> RootPolicy {
        Arc::new(move |real: &Path| {
            if self.allows(real) {
                Ok(())
            } else if self.allowed.is_empty() {
                Err("this machine allows no project folders yet — add one to `roots` in relay.toml on that machine".to_owned())
            } else {
                Err("outside the folders this machine allows — see `roots` in relay.toml on that machine".to_owned())
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use pw_core::bus::EventBus;
    use pw_core::config::AppPaths;
    use pw_core::registry::{NewProject, Registry};

    use super::*;

    fn registry(dir: &Path, roots: &[PathBuf]) -> Registry {
        let registry = Registry::new(AppPaths::under(&dir.join("app")), None, EventBus::new(16));
        registry.set_root_policy(Roots::new(roots).policy());
        registry
    }

    fn create(registry: &Registry, root: &Path) -> Result<(), String> {
        registry
            .create(NewProject {
                root: root.to_path_buf(),
                ..NewProject::default()
            })
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    /// `roots` 안은 받고, 밖 · `roots` 안의 심링크로 밖을 가리키는 경로 · `..` 로 빠져나간 경로는 거절한다(18-m7-plan M7-04 시험 ①).
    #[test]
    fn only_folders_inside_the_roots_are_registered() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().canonicalize().unwrap();
        let allowed = base.join("allowed");
        let outside = base.join("outside");
        for d in [
            allowed.join("app"),
            allowed.join("deep/er"),
            outside.clone(),
        ] {
            std::fs::create_dir_all(d).unwrap();
        }
        std::os::unix::fs::symlink(&outside, allowed.join("escape")).unwrap();
        let registry = registry(dir.path(), std::slice::from_ref(&allowed));

        assert_eq!(create(&registry, &allowed.join("app")), Ok(()));
        assert_eq!(create(&registry, &allowed.join("deep/er")), Ok(()));
        for refused in [
            outside.clone(),
            allowed.join("escape"),
            allowed.join("../outside"),
            base.clone(),
        ] {
            let err = create(&registry, &refused).expect_err(&refused.display().to_string());
            assert!(err.contains("relay.toml"), "{err}");
        }
        // 접두 글자가 같은 이웃 폴더(`allowed-2`)는 안이 아니다.
        let sibling = base.join("allowed-2");
        std::fs::create_dir_all(&sibling).unwrap();
        assert!(create(&registry, &sibling).is_err());
        assert_eq!(registry.list().unwrap().len(), 2);
    }

    /// `roots` 가 심링크면 그 실제 경로가 뿌리다. `roots` 가 비면 아무것도 등록되지 않는다.
    #[test]
    fn roots_are_resolved_and_empty_roots_allow_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().canonicalize().unwrap();
        let real = base.join("real");
        std::fs::create_dir_all(real.join("p")).unwrap();
        std::os::unix::fs::symlink(&real, base.join("alias")).unwrap();
        let registry = registry(dir.path(), &[base.join("alias")]);
        assert_eq!(create(&registry, &real.join("p")), Ok(()));

        let empty = registry_at(dir.path(), "empty");
        let err = create(&empty, &real.join("p")).unwrap_err();
        assert!(err.contains("allows no project folders"), "{err}");
    }

    fn registry_at(dir: &Path, name: &str) -> Registry {
        let registry = Registry::new(AppPaths::under(&dir.join(name)), None, EventBus::new(16));
        registry.set_root_policy(Roots::new(&[]).policy());
        registry
    }
}
