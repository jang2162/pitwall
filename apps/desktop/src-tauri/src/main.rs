// 릴리스 빌드에서 Windows 가 콘솔 창을 함께 띄우지 않게 한다.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::Arc;

use pw_core::{HostRouter, LocalHost};

mod ipc;
mod menu;
mod windows;

/// 데스크톱 프로세스 안의 Host 의 id. 프론트엔드가 프로젝트 없이 연결할 때 이 id 로 고른다(src/host-demo.tsx).
const LOCAL_HOST_ID: &str = "local";

/// 앱이 쓰는 LocalHost. 생성물과 등록된 메서드를 맞춰 보는 테스트도 이것을 쓴다.
fn local_host() -> LocalHost {
    LocalHost::new(LOCAL_HOST_ID)
}

fn main() {
    let router = Arc::new(HostRouter::new());
    router
        .add_host(Arc::new(local_host()))
        .expect("빈 라우터에 Host 를 처음 등록한다");

    ipc::install(tauri::Builder::default(), router)
        .menu(menu::build)
        .on_menu_event(menu::handle)
        .setup(|app| {
            windows::open_welcome(app.handle())?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("Tauri 앱을 시작하지 못했다");
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    /// 생성물(`packages/rpc-client`)의 메서드 목록 == 앱의 Host 가 실제로 등록한 메서드 목록.
    ///
    /// 메서드를 선언한 크레이트를 pw-rpc-gen 에 링크하지 않으면 그 메서드는 생성물에서 오류 없이 빠지고 생성물 최신 검사도
    /// 통과한다(crates/pw-rpc-gen/src/main.rs 머리 주석). 그 빠뜨림을 잡는 곳이 여기다. 핸들러만 있고 생성물이 낡은 경우,
    /// 선언만 하고 핸들러를 안 붙인 경우도 함께 잡는다. 앱과 같은 크레이트 집합을 링크하므로 CI 의 세 OS 에서
    /// 「다른 크레이트의 선언 수집」도 이 테스트로 실증된다.
    /// 보지 않는 것: 어느 Host 에도 등록하지 않을 선언. 그런 선언을 허용하게 되면 비교를 「등록 ⊆ 생성」으로 낮춘다.
    #[test]
    fn generated_rpc_covers_the_router() {
        let schema: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../packages/rpc-client/src/generated/schema.json"
        ))
        .unwrap();
        let generated: BTreeSet<String> = schema["methods"]
            .as_object()
            .expect("schema.json 의 methods 객체")
            .keys()
            .cloned()
            .collect();
        let host = local_host();
        let routed: BTreeSet<String> = host
            .router()
            .methods()
            .into_iter()
            .map(|m| m.name.to_owned())
            .collect();
        assert!(!routed.is_empty());
        assert_eq!(
            generated, routed,
            "`mise run gen` 을 돌리고, 메서드를 선언한 크레이트를 pw-rpc-gen 에 링크했는지 본다"
        );
    }
}
