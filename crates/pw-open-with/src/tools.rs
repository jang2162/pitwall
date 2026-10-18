//! 이 OS 에서 쓸 수 있는 외부 도구 목록과 기본 도구.
//!
//! 목록: 확장의 `[contributes.external_tools.<id>]`(내장 확장 `extensions/external-tools` 가 기본 목록) 위에 전역 설정
//! (`settings.toml` · `settings.local.toml`)의 `[open_with.tools.<id>]` 를 덮는다. 설정은 등록된 도구를 고치거나 새 도구를 더한다
//! (`docs/design/03-architecture.md` 「`[open_with]`」).
//! - 고칠 때: 적은 키만 바꾼다. `command` 를 적으면 확장의 `file_command` 는 버린다(같이 적은 것만 남는다) — 실행 파일을 바꾼 사람이
//!   파일을 열 때만 옛 실행 파일이 도는 일이 없게. `file_command` 만 적으면 그것만 바꾼다.
//! - 새 도구는 `label` 과 `command` 가 있어야 한다.
//! - 형식이 틀린 설정은 그 도구를 **문제 있는 도구**로 남긴다(목록에 보이고 실행하면 그 문제를 알린다). 확장의 값으로 조용히
//!   물러나지 않는다 — 고치려던 명령 대신 옛 명령이 돌면 안 된다.
//!
//! 프로젝트 설정(`.pitwall/project.toml` · `.local.toml` · 개인 전용 `project.toml`)은 `[open_with] default` 로 **등록된 도구 중에서
//! 고르기만** 한다. 명령을 정의하거나 바꿀 수 없다 — 저장소를 클론해 열기만 했는데 그 저장소가 정한 명령이 돌면 안 된다.
//! 프로젝트 설정의 `[open_with.tools]` 는 경고하고 버린다.

use std::collections::BTreeMap;

use pw_ext::Registry;
use pw_ext::manifest::Platform;
use pw_ext::template::{CommandKind, check_command};
use serde_json::Value;

/// 설정 키.
pub const SETTINGS_KEY: &str = "open_with";

/// 실행할 수 있는(또는 문제를 알릴) 도구 하나.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tool {
    pub id: String,
    pub label: String,
    pub command: Vec<String>,
    pub file_command: Option<Vec<String>>,
    /// 기여한 확장의 id. 설정에서만 정의한 도구면 `None`.
    pub extension: Option<String>,
    /// 전역 설정이 고쳤거나 더했는가.
    pub from_settings: bool,
    /// 설정 형식 오류. 있으면 실행하지 않는다.
    pub problem: Option<String>,
}

pub fn current_platform() -> Platform {
    if cfg!(target_os = "macos") {
        Platform::Macos
    } else if cfg!(windows) {
        Platform::Windows
    } else {
        Platform::Linux
    }
}

/// 설정의 도구 하나(`[open_with.tools.<id>]`). 없는 키는 `None`.
#[derive(Debug, Default)]
struct ToolSetting {
    label: Option<String>,
    command: Option<Vec<String>>,
    file_command: Option<Vec<String>>,
    platforms: Option<Vec<Platform>>,
}

fn parse_tool_setting(
    id: &str,
    value: &Value,
    warnings: &mut Vec<String>,
) -> Result<ToolSetting, String> {
    let Some(table) = value.as_object() else {
        return Err(format!("`open_with.tools.{id}` must be a table"));
    };
    let mut out = ToolSetting::default();
    for (key, v) in table {
        match key.as_str() {
            "label" => match v.as_str() {
                Some(s) if !s.trim().is_empty() => out.label = Some(s.to_owned()),
                _ => {
                    return Err(format!(
                        "`open_with.tools.{id}.label` must be a non-empty string"
                    ));
                }
            },
            "command" | "file_command" => {
                let kind = if key == "command" {
                    CommandKind::Project
                } else {
                    CommandKind::File
                };
                let argv: Option<Vec<String>> = v
                    .as_array()
                    .and_then(|a| a.iter().map(|e| e.as_str().map(str::to_owned)).collect());
                let Some(argv) = argv else {
                    return Err(format!(
                        "`open_with.tools.{id}.{key}` must be an array of strings like [\"zed\", \"{{project_root}}\"] — shell command strings are not accepted"
                    ));
                };
                check_command(kind, &argv)
                    .map_err(|e| format!("`open_with.tools.{id}.{key}` {e}"))?;
                if kind == CommandKind::Project {
                    out.command = Some(argv);
                } else {
                    out.file_command = Some(argv);
                }
            }
            "platforms" => {
                let names: Option<Vec<&str>> = v
                    .as_array()
                    .and_then(|a| a.iter().map(Value::as_str).collect());
                let platforms = names
                    .unwrap_or_default()
                    .into_iter()
                    .map(|name| match name {
                        "macos" => Some(Platform::Macos),
                        "linux" => Some(Platform::Linux),
                        "windows" => Some(Platform::Windows),
                        _ => None,
                    })
                    .collect::<Option<Vec<_>>>()
                    .filter(|_| v.is_array())
                    .ok_or_else(|| {
                        format!("`open_with.tools.{id}.platforms` must list `macos`, `linux` or `windows`")
                    })?;
                out.platforms = Some(platforms);
            }
            _ => warnings.push(format!(
                "`open_with.tools.{id}`: unknown key `{key}` is ignored"
            )),
        }
    }
    Ok(out)
}

fn on(platforms: &[Platform], platform: Platform) -> bool {
    platforms.is_empty() || platforms.contains(&platform)
}

/// 이 OS(`platform`)의 도구 목록. 확장 순서 다음에 설정에서만 정의한 도구(이름순)다. `open_with` 는 전역 설정의 `[open_with]` 값.
pub fn tools(
    registry: &Registry,
    open_with: Option<&Value>,
    platform: Platform,
    warnings: &mut Vec<String>,
) -> Vec<Tool> {
    let mut settings: BTreeMap<String, Result<ToolSetting, String>> = BTreeMap::new();
    match open_with.and_then(|o| o.get("tools")) {
        None => {}
        Some(Value::Object(table)) => {
            for (id, value) in table {
                settings.insert(id.clone(), parse_tool_setting(id, value, warnings));
            }
        }
        Some(_) => warnings.push("`open_with.tools` must be a table; ignored".to_owned()),
    }

    let mut out = Vec::new();
    for (extension, decl) in registry.external_tools() {
        let setting = settings.remove(&decl.id);
        let mut tool = Tool {
            id: decl.id.clone(),
            label: decl.label.clone(),
            command: decl.command.clone(),
            file_command: decl.file_command.clone(),
            extension: Some(extension.manifest.id.clone()),
            from_settings: setting.is_some(),
            problem: None,
        };
        let mut platforms = decl.platforms.as_slice();
        match &setting {
            None => {}
            Some(Err(e)) => tool.problem = Some(e.clone()),
            Some(Ok(s)) => {
                if let Some(label) = &s.label {
                    tool.label.clone_from(label);
                }
                if let Some(command) = &s.command {
                    tool.command.clone_from(command);
                    tool.file_command.clone_from(&s.file_command);
                } else if let Some(file_command) = &s.file_command {
                    tool.file_command = Some(file_command.clone());
                }
                if let Some(p) = &s.platforms {
                    platforms = p;
                }
            }
        }
        if on(platforms, platform) {
            out.push(tool);
        }
    }
    for (id, setting) in settings {
        let mut tool = Tool {
            label: id.clone(),
            id,
            command: Vec::new(),
            file_command: None,
            extension: None,
            from_settings: true,
            problem: None,
        };
        match setting {
            Err(e) => tool.problem = Some(e),
            Ok(s) => {
                if s.platforms.as_deref().is_some_and(|p| !on(p, platform)) {
                    continue;
                }
                tool.file_command = s.file_command;
                match (s.label, s.command) {
                    (Some(label), Some(command)) => {
                        tool.label = label;
                        tool.command = command;
                    }
                    (label, _) => {
                        if let Some(label) = label {
                            tool.label = label;
                        }
                        tool.problem = Some(format!(
                            "`open_with.tools.{}` is not a registered tool, so it needs both `label` and `command`",
                            tool.id
                        ));
                    }
                }
            }
        }
        out.push(tool);
    }
    out
}

/// 기본 도구를 고른 결과.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DefaultTool {
    Tool(String),
    /// 아무 설정도 고르지 않았다.
    Unset,
    /// 고른 도구가 이 OS 의 목록에 없다(마지막으로 본 값).
    Unknown(String),
}

/// 프로젝트 설정의 `open_with.default` 가 먼저, 없거나 이 OS 에 없는 도구면 전역 설정의 것. 둘 다 없으면 [`DefaultTool::Unset`].
pub fn default_tool(
    project: Option<&Value>,
    global: Option<&Value>,
    tools: &[Tool],
    warnings: &mut Vec<String>,
) -> DefaultTool {
    let mut unknown = None;
    for (layer, value) in [("project", project), ("global", global)] {
        let Some(value) = value else { continue };
        let Some(id) = value.as_str() else {
            warnings.push(format!(
                "{layer} `open_with.default` must be a tool id string; ignored"
            ));
            continue;
        };
        if tools.iter().any(|t| t.id == id) {
            return DefaultTool::Tool(id.to_owned());
        }
        warnings.push(format!(
            "{layer} `open_with.default` names `{id}`, which is not a tool on this system"
        ));
        unknown.get_or_insert_with(|| id.to_owned());
    }
    unknown.map_or(DefaultTool::Unset, DefaultTool::Unknown)
}

#[cfg(test)]
mod tests {
    use semver::Version;
    use serde_json::json;

    use super::*;

    fn builtin() -> Registry {
        Registry::from_roots(&Version::new(0, 0, 0), pw_ext::builtin::roots())
    }

    fn ids(tools: &[Tool]) -> Vec<&str> {
        tools.iter().map(|t| t.id.as_str()).collect()
    }

    #[test]
    fn filters_the_builtin_tools_by_platform() {
        let registry = builtin();
        let mut w = Vec::new();
        assert_eq!(
            ids(&tools(&registry, None, Platform::Macos, &mut w)),
            [
                "orca", "zed", "idea", "vscode", "cursor", "finder", "terminal"
            ]
        );
        assert_eq!(
            ids(&tools(&registry, None, Platform::Windows, &mut w)),
            ["orca", "zed", "idea", "vscode", "cursor", "explorer"]
        );
        assert_eq!(
            ids(&tools(&registry, None, Platform::Linux, &mut w)),
            ["orca", "zed", "idea", "vscode", "cursor", "file-manager"]
        );
        assert_eq!(w, Vec::<String>::new());
    }

    #[test]
    fn settings_override_a_tool_and_add_new_ones() {
        let registry = builtin();
        let settings = json!({"tools": {
            "zed": {"command": ["/opt/zed/bin/zed", "{project_root}"]},
            "idea": {"label": "IDEA EAP", "file_command": ["idea-eap", "{file}"]},
            "fake": {"label": "Fake", "command": ["/tmp/fake", "{project_root}"], "extra": 1},
        }});
        let mut w = Vec::new();
        let list = tools(&registry, Some(&settings), Platform::Macos, &mut w);
        assert_eq!(ids(&list).last(), Some(&"fake"));
        let zed = list.iter().find(|t| t.id == "zed").unwrap();
        assert_eq!(zed.command, ["/opt/zed/bin/zed", "{project_root}"]);
        // command 를 바꾸면 확장의 file_command 는 따라오지 않는다.
        assert_eq!(zed.file_command, None);
        assert!(zed.from_settings);
        let idea = list.iter().find(|t| t.id == "idea").unwrap();
        assert_eq!(idea.label, "IDEA EAP");
        assert_eq!(idea.command, ["idea", "{project_root}"]);
        assert_eq!(
            idea.file_command.as_deref(),
            Some(&["idea-eap".to_owned(), "{file}".to_owned()][..])
        );
        let fake = list.iter().find(|t| t.id == "fake").unwrap();
        assert_eq!(fake.extension, None);
        assert_eq!(fake.problem, None);
        assert_eq!(
            w,
            ["`open_with.tools.fake`: unknown key `extra` is ignored"]
        );
    }

    /// 틀린 설정은 확장의 명령으로 물러나지 않고 문제로 남는다.
    #[test]
    fn a_bad_override_marks_the_tool_instead_of_falling_back() {
        let registry = builtin();
        let settings = json!({"tools": {
            "zed": {"command": "zed {project_root}"},
            "code2": {"label": "Code 2"},
            "bad": {"label": "Bad", "command": ["{project_root}/run"]},
        }});
        let mut w = Vec::new();
        let list = tools(&registry, Some(&settings), Platform::Macos, &mut w);
        let problem = |id: &str| {
            list.iter()
                .find(|t| t.id == id)
                .unwrap()
                .problem
                .clone()
                .unwrap()
        };
        assert!(
            problem("zed").contains("shell command strings are not accepted"),
            "{}",
            problem("zed")
        );
        assert!(problem("code2").contains("needs both `label` and `command`"));
        assert!(problem("bad").contains("cannot use variables"));
    }

    #[test]
    fn settings_can_move_a_tool_between_platforms() {
        let registry = builtin();
        let settings = json!({"tools": {
            "terminal": {"platforms": ["linux"], "command": ["gnome-terminal", "--working-directory={project_root}"]},
            "zed": {"platforms": ["windows"]},
        }});
        let mut w = Vec::new();
        let linux = tools(&registry, Some(&settings), Platform::Linux, &mut w);
        assert_eq!(
            ids(&linux),
            [
                "orca",
                "idea",
                "vscode",
                "cursor",
                "file-manager",
                "terminal"
            ]
        );
    }

    #[test]
    fn the_project_default_wins_and_falls_back_to_the_global_one() {
        let mut w = Vec::new();
        let list = tools(&builtin(), None, Platform::Macos, &mut w);
        let pick = |p: Option<Value>, g: Option<Value>, w: &mut Vec<String>| {
            default_tool(p.as_ref(), g.as_ref(), &list, w)
        };
        assert_eq!(
            pick(Some(json!("zed")), Some(json!("idea")), &mut w),
            DefaultTool::Tool("zed".into())
        );
        assert_eq!(
            pick(None, Some(json!("idea")), &mut w),
            DefaultTool::Tool("idea".into())
        );
        assert_eq!(pick(None, None, &mut w), DefaultTool::Unset);
        assert!(w.is_empty(), "{w:?}");
        // 프로젝트가 이 OS 에 없는 도구(Windows 의 explorer)를 골랐으면 전역 값으로.
        assert_eq!(
            pick(Some(json!("explorer")), Some(json!("zed")), &mut w),
            DefaultTool::Tool("zed".into())
        );
        assert_eq!(
            pick(Some(json!("explorer")), None, &mut w),
            DefaultTool::Unknown("explorer".into())
        );
        assert_eq!(pick(Some(json!(3)), None, &mut w), DefaultTool::Unset);
        assert_eq!(w.len(), 3, "{w:?}");
    }
}
