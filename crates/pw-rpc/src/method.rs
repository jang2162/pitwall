//! 메서드 정의: 이름 · 입력/출력 타입 · 권한 등급 · role 을 한 선언에 모은다.
//!
//! 선언은 [`rpc_method!`](crate::rpc_method) 로 한다. 선언이 만든 [`MethodMeta`] 는
//! [`Router::methods`](crate::Router::methods) 로 열거할 수 있어, TS 타입 · JSON Schema · MCP 툴 목록이
//! 같은 정의에서 파생된다(`docs/design/02-tech-stack.md` §2.6).

use std::fmt;

use serde::de::DeserializeOwned;
use serde::ser::SerializeSeq;
use serde::{Serialize, Serializer};

/// 권한 등급. 값과 순서는 `docs/design/05-mcp.md` §3 「권한 모델」 표를 따른다.
///
/// 디스패처는 이 값을 강제하지 않는다 — 승인 정책(자동 허용 · UI 승인)은 이 값을 읽는 MCP 계층이 정한다.
/// 호출 가능 여부는 [`RoleSet`] 이 정한다.
///
/// 메서드가 도달할 수 있는 **가장 높은** 등급으로 선언한다. `roles` 를 생략하면 등급에서 role 이 정해지므로
/// (`read` 는 readonly 까지 연다), 낮게 선언하면 그 위의 조작이 조용히 열린다.
/// 입력에 따라 등급이 오르는 메서드(`db_query` 의 read / dangerous 등)는 조회용과 쓰기용으로 나누거나
/// (`db.query` read / `db.execute` dangerous), 높은 등급으로 선언하고 `roles` 를 명시한다.
/// 둘 중 어느 쪽을 정책으로 할지는 MCP 툴 레지스트리(M0-17)에서 확정한다.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Tier {
    Read,
    Write,
    Exec,
    Dangerous,
}

/// 호출자(페어링 기기)의 role. 값은 `docs/design/08-remote-mobile.md` 「보안 모델」 절을 따른다.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// Desktop. 전체.
    Owner,
    /// 조회 + 메서드가 명시적으로 허용한 조작.
    Mobile,
    /// 조회만.
    Readonly,
}

impl Role {
    pub const ALL: [Role; 3] = [Role::Owner, Role::Mobile, Role::Readonly];

    const fn bit(self) -> u8 {
        1 << self as u8
    }
}

/// 메서드를 호출할 수 있는 role 의 집합.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct RoleSet(u8);

impl RoleSet {
    pub const EMPTY: Self = Self(0);

    #[must_use]
    pub const fn with(self, role: Role) -> Self {
        Self(self.0 | role.bit())
    }

    pub const fn contains(self, role: Role) -> bool {
        self.0 & role.bit() != 0
    }

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// `roles` 를 생략한 선언의 기본값. `read` 는 모든 role, 그 외 등급은 `owner` 만.
    ///
    /// `mobile` 에게 조작을 열려면 선언에 `roles` 를 명시한다 — 「허용된 조작만」은 메서드마다 고르는 것이다.
    pub const fn default_for(tier: Tier) -> Self {
        match tier {
            Tier::Read => Self::EMPTY
                .with(Role::Owner)
                .with(Role::Mobile)
                .with(Role::Readonly),
            Tier::Write | Tier::Exec | Tier::Dangerous => Self::EMPTY.with(Role::Owner),
        }
    }

    pub fn iter(self) -> impl Iterator<Item = Role> {
        Role::ALL.into_iter().filter(move |r| self.contains(*r))
    }
}

impl fmt::Debug for RoleSet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_set().entries(self.iter()).finish()
    }
}

impl Serialize for RoleSet {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut seq = serializer.serialize_seq(None)?;
        for role in self.iter() {
            seq.serialize_element(&role)?;
        }
        seq.end()
    }
}

/// 응답 형태. `unary` 는 결과 하나, `stream` 은 `streamId` 를 돌려주고 청크를 `stream.chunk` 로 보낸다.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum MethodKind {
    Unary,
    Stream,
}

/// 메서드 하나의 메타데이터. `rpc_method!` 가 상수로 만든다.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MethodMeta {
    /// `<module>.<action>` (`docs/design/03-architecture.md` §4).
    pub name: &'static str,
    pub kind: MethodKind,
    pub tier: Tier,
    pub roles: RoleSet,
    /// 입력 타입의 소스 표기(`stringify!`). 타입 생성은 M0-06 이 실제 타입으로 한다 — 이 문자열은 표시용이다.
    pub params: &'static str,
    /// unary 는 결과 타입, stream 은 청크 항목 타입의 소스 표기.
    pub output: &'static str,
    /// 선언의 문서 주석. MCP 툴 설명의 원천.
    pub doc: &'static str,
}

/// 내장 프로토콜 메서드가 쓰는 접두사. 선언한 메서드는 이 이름들을 쓸 수 없다.
/// `rpc.` 는 JSON-RPC 2.0 이 시스템 확장용으로 예약했다.
pub const RESERVED_PREFIXES: [&str; 4] = ["rpc.", "request.", "stream.", "events."];

impl MethodMeta {
    /// 선언 규칙 검사. `rpc_method!` 가 컴파일 타임에 부른다 — 규칙을 어긴 선언은 빌드가 실패한다.
    pub const fn validate(&self) -> Result<(), &'static str> {
        let name = self.name.as_bytes();
        if name.is_empty() || name[0] == b'.' || name[name.len() - 1] == b'.' {
            return Err("method name must be `<module>.<action>`");
        }
        let mut has_dot = false;
        let mut i = 0;
        while i < name.len() {
            if name[i] == b'.' {
                has_dot = true;
            }
            i += 1;
        }
        if !has_dot {
            return Err("method name must be `<module>.<action>`");
        }
        let mut p = 0;
        while p < RESERVED_PREFIXES.len() {
            if starts_with(name, RESERVED_PREFIXES[p].as_bytes()) {
                return Err(
                    "method name uses a reserved prefix (rpc. / request. / stream. / events.)",
                );
            }
            p += 1;
        }
        if self.roles.is_empty() {
            return Err("method must allow at least one role");
        }
        // readonly 는 「조회만」이다. 조회가 아닌 등급에 readonly 를 열면 그 role 의 정의가 깨진다.
        if self.roles.contains(Role::Readonly) && !matches!(self.tier, Tier::Read) {
            return Err("readonly role may only call read-tier methods");
        }
        Ok(())
    }

    /// 이 role 의 호출자가 이 메서드를 부를 수 있는가. 디스패처의 권한 검사가 이것 하나다.
    pub const fn permits(&self, role: Role) -> bool {
        self.roles.contains(role)
    }

    /// 이름의 첫 마디(`git.log` → `git`). MCP 의 모듈 그룹 on/off 단위.
    pub fn module(&self) -> &'static str {
        match self.name.split_once('.') {
            Some((module, _)) => module,
            None => self.name,
        }
    }
}

const fn starts_with(s: &[u8], prefix: &[u8]) -> bool {
    if s.len() < prefix.len() {
        return false;
    }
    let mut i = 0;
    while i < prefix.len() {
        if s[i] != prefix[i] {
            return false;
        }
        i += 1;
    }
    true
}

/// 선언된 메서드. `rpc_method!` 가 구현한다 — 손으로 구현하지 않는다(컴파일 타임 검사를 건너뛴다).
pub trait Method: Copy + Send + Sync + 'static {
    type Params: DeserializeOwned + Send + 'static;
    const META: MethodMeta;
}

/// 결과 하나를 돌려주는 메서드.
pub trait UnaryMethod: Method {
    type Result: Serialize + Send + 'static;
}

/// `streamId` 를 돌려주고 `Item` 청크를 credit 만큼 보내는 메서드.
pub trait StreamMethod: Method {
    type Item: Serialize + Send + 'static;
}

/// RPC 메서드를 선언한다. 한 번에 여러 개를 선언할 수 있다.
///
/// ```
/// use pw_rpc::rpc_method;
///
/// #[derive(serde::Deserialize)]
/// pub struct LogParams { pub limit: u32 }
///
/// rpc_method! {
///     /// 커밋 로그를 스트림으로 보낸다.
///     pub GitLog: "git.log" {
///         params: LogParams,
///         stream: String,
///         tier: Read,
///     }
///
///     /// 실행 구성을 재시작한다. mobile 에게도 허용한다.
///     pub RunRestart: "run.restart" {
///         params: (),
///         result: (),
///         tier: Exec,
///         roles: [Owner, Mobile],
///     }
/// }
/// ```
///
/// - `result:` 는 unary, `stream:` 은 스트림 메서드가 된다.
/// - `tier` 는 [`Tier`] 의 변형 이름, `roles` 는 [`Role`] 의 변형 이름이다.
///   `roles` 를 생략하면 [`RoleSet::default_for`] 를 쓴다.
/// - 이름 규칙 · role 규칙은 [`MethodMeta::validate`] 가 컴파일 타임에 검사한다.
///   `roles: []` 는 생략과 다르게 빈 집합이라 빌드가 실패한다.
///
/// ```compile_fail
/// pw_rpc::rpc_method! {
///     Nobody: "x.nobody" { params: (), result: (), tier: Read, roles: [] }
/// }
/// ```
///
/// ```compile_fail
/// pw_rpc::rpc_method! {
///     ReadonlyWrite: "x.write" { params: (), result: (), tier: Write, roles: [Owner, Readonly] }
/// }
/// ```
#[macro_export]
macro_rules! rpc_method {
    () => {};
    (
        $(#[doc = $doc:literal])*
        $vis:vis $name:ident : $method:literal {
            params: $params:ty,
            result: $result:ty,
            tier: $tier:ident
            $(, roles: [$($role:ident),* $(,)?])?
            $(,)?
        }
        $($rest:tt)*
    ) => {
        $crate::__rpc_method_define! {
            [$($doc)*] $vis $name, $method, $params, Unary, $result, $tier, [$(explicit $($role)*)?]
        }
        impl $crate::UnaryMethod for $name {
            type Result = $result;
        }
        $crate::rpc_method! { $($rest)* }
    };
    (
        $(#[doc = $doc:literal])*
        $vis:vis $name:ident : $method:literal {
            params: $params:ty,
            stream: $item:ty,
            tier: $tier:ident
            $(, roles: [$($role:ident),* $(,)?])?
            $(,)?
        }
        $($rest:tt)*
    ) => {
        $crate::__rpc_method_define! {
            [$($doc)*] $vis $name, $method, $params, Stream, $item, $tier, [$(explicit $($role)*)?]
        }
        impl $crate::StreamMethod for $name {
            type Item = $item;
        }
        $crate::rpc_method! { $($rest)* }
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __rpc_method_define {
    (
        [$($doc:literal)*] $vis:vis $name:ident, $method:literal, $params:ty,
        $kind:ident, $output:ty, $tier:ident, [$($roles:tt)*]
    ) => {
        $(#[doc = $doc])*
        #[derive(Debug, Clone, Copy, Default)]
        $vis struct $name;

        impl $crate::Method for $name {
            type Params = $params;
            const META: $crate::MethodMeta = $crate::MethodMeta {
                name: $method,
                kind: $crate::MethodKind::$kind,
                tier: $crate::Tier::$tier,
                roles: $crate::__rpc_roles!($tier; $($roles)*),
                params: stringify!($params),
                output: stringify!($output),
                doc: concat!($($doc, "\n"),*),
            };
        }

        const _: () = if let Err(reason) = <$name as $crate::Method>::META.validate() {
            panic!("{}", reason)
        };
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __rpc_roles {
    ($tier:ident;) => {
        $crate::RoleSet::default_for($crate::Tier::$tier)
    };
    // `roles: []` 는 생략과 다르다. 빈 집합이 되어 validate 가 컴파일 에러로 만든다.
    ($tier:ident; explicit $($role:ident)*) => {
        $crate::RoleSet::EMPTY $(.with($crate::Role::$role))*
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(name: &'static str, tier: Tier, roles: RoleSet) -> MethodMeta {
        MethodMeta {
            name,
            kind: MethodKind::Unary,
            tier,
            roles,
            params: "()",
            output: "()",
            doc: "",
        }
    }

    #[test]
    fn default_roles_open_read_to_everyone_and_the_rest_to_owner_only() {
        let read = RoleSet::default_for(Tier::Read);
        assert!(Role::ALL.iter().all(|r| read.contains(*r)));
        for tier in [Tier::Write, Tier::Exec, Tier::Dangerous] {
            let set = RoleSet::default_for(tier);
            assert_eq!(set.iter().collect::<Vec<_>>(), vec![Role::Owner]);
        }
    }

    #[test]
    fn validate_rejects_readonly_on_non_read_tier() {
        let roles = RoleSet::EMPTY.with(Role::Owner).with(Role::Readonly);
        assert!(meta("fs.write", Tier::Write, roles).validate().is_err());
        assert!(meta("fs.read", Tier::Read, roles).validate().is_ok());
    }

    #[test]
    fn validate_rejects_bad_names_and_empty_roles() {
        let owner = RoleSet::EMPTY.with(Role::Owner);
        for bad in [
            "",
            "plain",
            ".git",
            "git.",
            "stream.open",
            "events.x",
            "request.y",
            "rpc.z",
        ] {
            assert!(meta(bad, Tier::Read, owner).validate().is_err(), "{bad}");
        }
        assert!(
            meta("git.log", Tier::Read, RoleSet::EMPTY)
                .validate()
                .is_err()
        );
        assert!(
            meta("db.query.execute", Tier::Read, owner)
                .validate()
                .is_ok()
        );
    }

    #[test]
    fn module_is_the_first_segment() {
        let owner = RoleSet::EMPTY.with(Role::Owner);
        assert_eq!(meta("db.query.execute", Tier::Read, owner).module(), "db");
    }
}
