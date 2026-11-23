//! 채널을 열 때 Desktop 이 싣는 호출자의 주장과, Relay 가 그것을 링크의 role 로 자르는 판정(18-m7-plan M7-02 ② · 메인 판정
//! M7-P4).
//!
//! - role 은 **링크가 증명한 기기의 role** 보다 높아지지 않는다(`owner` > `mobile` > `readonly`). Desktop 이 `owner` 를 주장해도
//!   `readonly` 기기의 링크면 `readonly` 다.
//! - 범위(`Host` · `Project` · `Agent`)는 **증명된 `owner` 기기의 링크**의 주장만 받는다 — 범위는 Desktop 이 자기 연결을 좁히는
//!   장치이고, `Project` 범위는 창 권한(프로젝트 쓰기 · 승인 답)을 여는 열쇠이기도 하다. 다른 기기의 링크는 `Host` 범위다.
//! - `File` 범위는 링크로 받지 않는다(파일 창은 로컬 Host 만 연다) — 채널을 거절한다.
//! - 창 label 은 기기 id 를 앞에 붙인다 — 두 Desktop 의 같은 label 이 서로의 창 자원(DB 세션 — M3-05)을 이어받지 않게.

use pw_rpc::{Caller, ConnectionScope, Role};
use serde::{Deserialize, Serialize};

use crate::device::DeviceId;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub(crate) enum ScopeClaim {
    Host,
    Project { id: String },
    Agent,
    File { path: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CallerClaim {
    pub role: Role,
    pub scope: ScopeClaim,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<String>,
}

impl From<&Caller> for CallerClaim {
    fn from(caller: &Caller) -> Self {
        let scope = match &caller.scope {
            ConnectionScope::Host => ScopeClaim::Host,
            ConnectionScope::Project(id) => ScopeClaim::Project { id: id.clone() },
            ConnectionScope::Agent => ScopeClaim::Agent,
            ConnectionScope::File(file) => ScopeClaim::File {
                path: file.path().display().to_string(),
            },
        };
        Self {
            role: caller.role,
            scope,
            window: caller.window.clone(),
        }
    }
}

/// 낮을수록 넓다.
fn rank(role: Role) -> u8 {
    match role {
        Role::Owner => 0,
        Role::Mobile => 1,
        Role::Readonly => 2,
    }
}

/// 링크 기기의 role 로 자른 호출자. 받지 않는 주장이면 사람이 읽는 까닭(영어 — 상대 Desktop 의 진단에 실린다).
pub(crate) fn admit(
    claim: CallerClaim,
    device_role: Role,
    device: &DeviceId,
) -> Result<Caller, String> {
    let role = if rank(claim.role) >= rank(device_role) {
        claim.role
    } else {
        device_role
    };
    let proven_owner = device_role == Role::Owner;
    let scope = match claim.scope {
        ScopeClaim::File { .. } => {
            return Err("file-scoped connections are not accepted over a link".to_owned());
        }
        ScopeClaim::Host => ConnectionScope::Host,
        ScopeClaim::Project { id } if proven_owner => ConnectionScope::Project(id),
        ScopeClaim::Agent if proven_owner => ConnectionScope::Agent,
        ScopeClaim::Project { .. } | ScopeClaim::Agent => ConnectionScope::Host,
    };
    let window = match scope {
        ConnectionScope::Agent => None,
        _ => claim.window.map(|w| format!("{device}/{w}")),
    };
    Ok(Caller {
        role,
        scope,
        window,
    })
}

/// 링크로 온 연결의 창 label(`<기기 id>/<label>` — [`admit`])을 (기기 id, 그 Desktop 의 창 label)로 나눈다. 접두가 없으면
/// `None`(이 머신의 창이거나 창이 없다). 기기 id 의 base64 에는 `/` 가 들 수 있고 Desktop 의 창 label 에는 들지 않으므로 **마지막**
/// `/` 에서 나눈다. Host 쪽에서 창 label 을 해석하는 자리(Relay 의 설정 창 판정 등 — 18-m7-plan M7-11)가 이것으로 본다.
pub fn link_window(label: &str) -> Option<(&str, &str)> {
    label.rsplit_once('/')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claim(role: Role, scope: ScopeClaim) -> CallerClaim {
        CallerClaim {
            role,
            scope,
            window: Some("project-1".into()),
        }
    }

    fn project() -> ScopeClaim {
        ScopeClaim::Project { id: "p1".into() }
    }

    #[test]
    fn a_link_window_label_splits_back_into_the_device_and_the_desktop_label() {
        let device = DeviceId::from_stored("SHA256:ab/c+d");
        let caller = admit(claim(Role::Owner, project()), Role::Owner, &device).unwrap();
        let label = caller.window.unwrap();
        assert_eq!(link_window(&label), Some(("SHA256:ab/c+d", "project-1")));
        assert_eq!(link_window("settings"), None);
    }

    #[test]
    fn the_role_never_rises_above_the_device_role() {
        let device = DeviceId::from_stored("SHA256:x");
        let cases = [
            (Role::Owner, Role::Readonly, Role::Readonly),
            (Role::Owner, Role::Mobile, Role::Mobile),
            (Role::Mobile, Role::Readonly, Role::Readonly),
            (Role::Readonly, Role::Owner, Role::Readonly),
            (Role::Owner, Role::Owner, Role::Owner),
        ];
        for (claimed, device_role, want) in cases {
            let caller = admit(claim(claimed, ScopeClaim::Host), device_role, &device).unwrap();
            assert_eq!(caller.role, want, "{claimed:?} on a {device_role:?} link");
        }
    }

    #[test]
    fn only_a_proven_owner_link_gets_the_scope_it_claims() {
        let device = DeviceId::from_stored("SHA256:x");
        let owner = admit(claim(Role::Owner, project()), Role::Owner, &device).unwrap();
        assert_eq!(owner.scope, ConnectionScope::Project("p1".into()));
        assert_eq!(owner.window.as_deref(), Some("SHA256:x/project-1"));
        let agent = admit(claim(Role::Owner, ScopeClaim::Agent), Role::Owner, &device).unwrap();
        assert_eq!(agent.scope, ConnectionScope::Agent);
        assert_eq!(agent.window, None);

        for role in [Role::Mobile, Role::Readonly] {
            let caller = admit(claim(Role::Owner, project()), role, &device).unwrap();
            assert_eq!(caller.scope, ConnectionScope::Host, "{role:?} link");
            let caller = admit(claim(Role::Owner, ScopeClaim::Agent), role, &device).unwrap();
            assert_eq!(caller.scope, ConnectionScope::Host, "{role:?} link");
        }
    }

    #[test]
    fn a_file_scope_is_refused_on_any_link() {
        let device = DeviceId::from_stored("SHA256:x");
        for role in Role::ALL {
            let file = ScopeClaim::File { path: "/x".into() };
            assert!(admit(claim(Role::Owner, file), role, &device).is_err());
        }
    }
}
