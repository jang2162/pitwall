//! 세션 툴(18-m7-plan M7-19 — 05-mcp.md 「체크아웃」): Host 메서드가 아니라 MCP 세션이 스스로 처리하는 툴. Host 는 `checkout` 도
//! 묶음도 모른다 — 두 머신을 아는 것은 Desktop 의 세션이다. 그래서 `rpc_method!` 선언이 아니고(생성물 · 라우터에 들지 않는다) 이
//! 모듈이 이름 · 그룹 · 등급 · 스키마를 정하고 카탈로그가 더한다([`crate::Catalog::linked`]). 처리는 `server` 의 `checkout_tools`.
//!
//! - `checkout_list`(read): 그 묶음의 체크아웃(머신 · 워크트리 · 브랜치)과 머신의 연결 상태. 링크를 열지 않는다 — 붙어 있는 머신만
//!   그 워크트리 · 브랜치를 읽는다(연결 상태는 `Machines::link_state` 의 가벼운 확인).
//! - `checkout_send`(write): 원본 체크아웃의 브랜치를 대상 체크아웃으로 보내고 대상을 그 커밋으로 맞춘다(`git.send.*` — M7-17 과 같은
//!   길). 계획은 세션이 두 머신에서 읽어 세운다 — 대상의 `theirs` · 브랜치 끝과 원본의 끝 커밋 · 보낼 커밋 수 · fast-forward 여부,
//!   그리고 대상 Host 가 낸 대상 계획 해시(`git.send.plan`). 승인은 대상 머신 기준(다른 머신이면 「이 세션 동안 이 머신 허용」).
//!
//! 두 툴 다 다른 머신을 아는 세션(`Services::machines` — Desktop 앱)에만 있다. 헤드리스 · 원격 머신 Relay 의 세션에는 없다(05 「원격
//! 머신 Relay 의 세션은 그 머신의 체크아웃만」).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::catalog::Tool;

/// `checkout_list` 의 메서드 이름(세션 안에서만 — Host 에 없다).
pub const LIST_METHOD: &str = "checkout.list";
/// `checkout_send` 의 메서드 이름(세션 안에서만 — Host 에 없다). 계획의 메서드도 이것이다.
pub const SEND_METHOD: &str = "checkout.send";
/// 세션 툴의 메서드 이름들.
pub const METHODS: [&str; 2] = [LIST_METHOD, SEND_METHOD];
/// 세션 툴의 그룹(`[mcp.groups]` 의 키).
pub const GROUP: &str = "checkout";

// `checkout_list` 의 입력.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CheckoutListInput {
    /// Project or bundle id (see project_list).
    pub project_id: String,
}

/// One checkout: a machine and a worktree on it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CheckoutRef {
    /// Machine name shown in Pitwall (see checkout_list), or `local` for this computer.
    pub machine: String,
    /// Worktree path on that machine. Empty or null for its main checkout.
    #[serde(default)]
    pub worktree: Option<String>,
}

// `checkout_send` 의 입력.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CheckoutSendInput {
    /// Project or bundle id (see project_list).
    pub project_id: String,
    /// The checkout to send from.
    pub source: CheckoutRef,
    /// The checkout to send to. To bring commits here from another machine, give that machine as `source` and `local`
    /// as `target`.
    pub target: CheckoutRef,
    /// Branch to send (short name). Empty means the branch the source checkout is on. Only its committed tip goes;
    /// uncommitted changes stay on the source.
    #[serde(default)]
    pub branch: Option<String>,
}

// `checkout_list` 의 결과.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CheckoutListResult {
    /// Pitwall's name of the project or bundle.
    pub name: String,
    pub machines: Vec<CheckoutMachine>,
}

/// One machine of the project's bundle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CheckoutMachine {
    /// Machine name to give as `checkout.machine` (`local` for this computer).
    pub machine: String,
    /// `local` (this computer), `connected`, `connecting`, `available` (not connected, but a call can open the link
    /// without asking anyone), `disconnected` (connect to it in Pitwall first) or `unknown`.
    pub state: String,
    /// The machine's checkouts: its main checkout and linked worktrees, with their branches. Only for this computer and
    /// connected machines — `null` otherwise (listing does not open a link).
    pub checkouts: Option<Vec<CheckoutEntry>>,
}

/// One checkout on a machine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CheckoutEntry {
    /// Worktree path on that machine (give it as `checkout.worktree`; the main checkout can also be given as empty).
    pub path: String,
    pub main: bool,
    /// The branch it is on. `null` for a detached HEAD or an unreadable worktree.
    pub branch: Option<String>,
}

// `checkout_send` 의 결과.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CheckoutSendResult {
    pub branch: String,
    /// The commit the target checkout is now on.
    pub commit: String,
    /// The previous tip of the target branch. `null` if the branch was created.
    pub previous: Option<String>,
    /// Number of commits sent.
    pub commits: u32,
    pub created: bool,
    /// The target checkout was switched to the branch.
    pub switched: bool,
    /// Files left as Git LFS pointers on the target (run `git lfs pull` there).
    pub lfs_pointers: u32,
    /// The source had uncommitted changes; they were not sent.
    pub uncommitted: bool,
    /// The work tree path of the target checkout.
    pub work_tree: String,
    pub audit: String,
}

/// 카탈로그에 더할 세션 툴.
pub fn tools() -> Vec<Tool> {
    vec![
        Tool::session(
            "checkout_list",
            LIST_METHOD,
            GROUP,
            pw_rpc::Tier::Read,
            "Checkouts of a project across the machines of its bundle (this computer and other machines added in \
             Pitwall): each machine's connection state and, for this computer and connected machines, its checkouts \
             (main checkout and worktrees) with their branches. Listing does not connect to a machine. Give a \
             machine and worktree from here as `checkout` to other tools, or as `source` / `target` to checkout_send.",
            crate::catalog::schema_of::<CheckoutListInput>(),
            Some(crate::catalog::schema_of_output::<CheckoutListResult>()),
        ),
        Tool::session(
            "checkout_send",
            SEND_METHOD,
            GROUP,
            pw_rpc::Tier::Write,
            "Send a branch from one checkout of a project to a checkout on another machine and check it out there, \
             without going through origin (a bundle carries only the commits the target lacks). The target branch is \
             fast-forwarded, or created if it does not exist, and the target checkout switches to it. A diverged \
             target branch or a change Git refuses stops it with Git's message and leaves the target unchanged. Asks \
             for approval in Pitwall (once per machine in a session for another machine). Git hooks of the target \
             repository run on the target machine.",
            crate::catalog::schema_of::<CheckoutSendInput>(),
            Some(crate::catalog::schema_of_output::<CheckoutSendResult>()),
        ),
    ]
}

/// 원본 머신 id 를 전용 ref 의 한 마디(`[A-Za-z0-9._-]`, 128자)로(웹뷰 `git/send/send-flow.ts` 의 `sourceMachineName` 과 같은 규칙 —
/// 같은 원본은 사람이 보내든 에이전트가 보내든 같은 전용 ref 다). 이 머신은 `local` 그대로다.
pub fn source_machine_name(machine: &str) -> String {
    let mut name = match machine.strip_prefix("SHA256:") {
        Some(rest) => format!("sha256-{rest}"),
        None => machine.to_owned(),
    };
    while name.ends_with('=') {
        name.pop();
    }
    let mut name: String = name
        .chars()
        .map(|c| match c {
            '+' => '-',
            '/' => '_',
            c if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') => c,
            _ => '_',
        })
        .collect();
    if name.is_empty() || name.starts_with('.') {
        name = format!("m{name}");
    }
    if name.len() > 128 {
        name.truncate(128);
    }
    if let Some(stem) = name.strip_suffix(".lock") {
        name = format!("{stem}_lock");
    }
    name
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_machine_names_follow_the_engines_rule() {
        assert_eq!(source_machine_name("local"), "local");
        assert_eq!(
            source_machine_name("SHA256:ab+c/d=="),
            "sha256-ab-c_d",
            "웹뷰 sourceMachineName 과 같다"
        );
        assert_eq!(source_machine_name(".x"), "m.x");
        assert_eq!(source_machine_name("a.lock"), "a_lock");
        assert_eq!(source_machine_name(&"a".repeat(200)).len(), 128);
    }
}
