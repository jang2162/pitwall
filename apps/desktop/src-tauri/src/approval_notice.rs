//! MCP 승인 요청의 OS 알림(16-m5-plan M5-14). 승인 다이얼로그는 요청을 받은 창의 웹뷰가 띄운다(`src/mcp/approvals-host.tsx`) —
//! 그 창이 앞에 없으면 사람이 요청을 못 보고 시한(기본 5분)이 지난다. 그래서 웹뷰가 새 요청마다 [`mcp_approval_notify`] 를 부르고,
//! 여기서 가른다:
//!
//! - 그 창이 앞에 있으면 아무것도 하지 않는다(다이얼로그가 보인다).
//! - 앞에 없으면 OS 알림 「<에이전트>가 <툴> 승인을 기다린다」 를 낸다. **창이 다시 앞에 올 때까지 창마다 한 번** — 에이전트가 요청을
//!   여럿 보내도 알림이 쌓이지 않는다(Host 는 세션당 대기를 8개로 묶는다 — `pw_mcp::approval::MAX_PENDING_PER_SESSION`).
//! - 알림을 낼 수 없으면(권한 없음 · 내기 실패) 그 창의 주의 요청(macOS 는 Dock 아이콘 튀기기 — [`UserAttentionType::Critical`],
//!   앱이 앞에 올 때까지)으로 대신한다. 데스크톱의 `tauri-plugin-notification` 은 권한을 늘 「있음」 으로 답한다 — macOS 에서 사람이
//!   알림을 껐으면 알림이 조용히 사라질 수 있고 그때는 주의 요청도 없다(플러그인이 알 길이 없다).
//! - 사람이 알림을 누르면(`tap`) 그 창을 앞으로 가져오고 그 창에 [`REVEAL_EVENT`](요청 id)를 보낸다 — 웹뷰가 그 다이얼로그를 다시
//!   연다(Esc 로 닫았던 것이어도).
//!
//! 알림의 글은 웹뷰가 지금 언어로 만들고 에이전트가 보낸 이름은 웹뷰가 거른다(`src/mcp/agent-text.ts`). 여기서는 길이만 다시 묶고
//! 제어 문자를 지운다 — 알림 서버가 받는 글이다.

use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, MutexGuard, PoisonError};

use tauri::{AppHandle, Emitter, Manager, Runtime, State, UserAttentionType, WebviewWindow};
use tauri_plugin_notification::{NotificationExt, PermissionState};

/// 알림을 누른 창에 보내는 이벤트(payload 는 요청 id). `src/mcp/approval-native.ts` 의 `REVEAL_EVENT` 와 같다.
pub const REVEAL_EVENT: &str = "mcp-approval-reveal";

/// 알림 제목 · 본문의 글자 상한.
const NOTICE_CHARS: usize = 200;

/// 알림을 내는 쪽. 앱은 [`PluginNotifier`], 시험은 가짜.
pub trait Notifier {
    /// 알림을 낼 권한이 있는가.
    fn permitted(&self) -> bool;
    /// 알림 하나를 낸다. `id` 는 누름을 받을 때 그 알림을 가른다.
    fn show(&self, id: i32, title: &str, body: &str) -> Result<(), String>;
}

/// [`ApprovalNotices::announce`] 가 한 것.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Notice {
    /// 창이 앞에 있다 — 아무것도 하지 않았다.
    InFront,
    /// 그 창이 앞에 오기 전에 이미 알렸다.
    AlreadyAnnounced,
    /// OS 알림을 냈다.
    Notified,
    /// 알림 대신 창의 주의를 청했다.
    Attention,
}

/// 창마다의 알림 상태. 앱에 하나(`manage`).
#[derive(Default)]
pub struct ApprovalNotices {
    state: Mutex<NoticeState>,
}

#[derive(Default)]
struct NoticeState {
    next: i32,
    /// 알렸고 아직 앞에 오지 않은 창(label).
    announced: HashSet<String>,
    /// 낸 알림 id → (창 label, 요청 id). 창이 앞에 오거나 닫히면 그 창의 것을 지운다.
    shown: HashMap<i32, (String, u64)>,
}

impl ApprovalNotices {
    fn lock(&self) -> MutexGuard<'_, NoticeState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// 창 `window` 에 요청 `request` 가 왔다. `in_front` 는 그 창이 지금 앞에 있는가, `attention` 은 창의 주의 요청.
    pub fn announce(
        &self,
        window: &str,
        in_front: bool,
        request: u64,
        title: &str,
        body: &str,
        notifier: &dyn Notifier,
        attention: impl FnOnce(),
    ) -> Notice {
        if in_front {
            return Notice::InFront;
        }
        let mut state = self.lock();
        if state.announced.contains(window) {
            return Notice::AlreadyAnnounced;
        }
        state.announced.insert(window.to_owned());
        if notifier.permitted() {
            state.next = state.next.wrapping_add(1);
            let id = state.next;
            match notifier.show(id, &clean(title), &clean(body)) {
                Ok(()) => {
                    state.shown.insert(id, (window.to_owned(), request));
                    return Notice::Notified;
                }
                Err(error) => {
                    tracing::warn!(%error, "could not show the approval notification; asking for attention instead");
                }
            }
        }
        drop(state);
        attention();
        Notice::Attention
    }

    /// 창이 앞에 왔다 — 다음 요청은 다시 알린다.
    pub fn focused(&self, window: &str) {
        let mut state = self.lock();
        state.announced.remove(window);
        state.shown.retain(|_, (w, _)| w != window);
    }

    /// 사람이 알림 `id` 를 눌렀다 — 그 창과 요청.
    pub fn tapped(&self, id: i32) -> Option<(String, u64)> {
        self.lock().shown.get(&id).cloned()
    }
}

/// 알림 서버에 넘길 글: 제어 · 양방향 제어 · 0폭 글자를 빼고 길이를 묶는다(웹뷰가 이미 거른 글이다 — 한 번 더).
fn clean(text: &str) -> String {
    let mut out: String = text
        .chars()
        .filter(|c| !invisible(*c))
        .take(NOTICE_CHARS)
        .collect();
    if text
        .chars()
        .filter(|c| !invisible(*c))
        .nth(NOTICE_CHARS)
        .is_some()
    {
        out.push('…');
    }
    out
}

fn invisible(c: char) -> bool {
    c.is_control()
        || matches!(c,
            '\u{00AD}' | '\u{061C}' | '\u{180E}' | '\u{200B}'..='\u{200F}' | '\u{2028}'..='\u{202E}'
            | '\u{2060}'..='\u{206F}' | '\u{FEFF}' | '\u{FE00}'..='\u{FE0F}' | '\u{E0000}'..='\u{E0FFF}')
}

/// `tauri-plugin-notification` 으로 내는 알림.
pub struct PluginNotifier<R: Runtime>(pub AppHandle<R>);

impl<R: Runtime> Notifier for PluginNotifier<R> {
    fn permitted(&self) -> bool {
        matches!(
            self.0.notification().permission_state(),
            Ok(PermissionState::Granted)
        )
    }

    fn show(&self, id: i32, title: &str, body: &str) -> Result<(), String> {
        self.0
            .notification()
            .builder()
            .id(id)
            .title(title)
            .body(body)
            .show()
            .map_err(|e| e.to_string())
    }
}

/// 웹뷰: 이 창에 승인 요청 `request` 가 왔다(`src/mcp/approval-native.ts`).
#[tauri::command]
pub async fn mcp_approval_notify<R: Runtime>(
    window: WebviewWindow<R>,
    notices: State<'_, ApprovalNotices>,
    request: u64,
    title: String,
    body: String,
) -> Result<(), ()> {
    let in_front = window.is_focused().unwrap_or(false);
    let notifier = PluginNotifier(window.app_handle().clone());
    notices.announce(
        window.label(),
        in_front,
        request,
        &title,
        &body,
        &notifier,
        || {
            if let Err(error) = window.request_user_attention(Some(UserAttentionType::Critical)) {
                tracing::warn!(%error, "could not ask for attention for an approval request");
            }
        },
    );
    Ok(())
}

/// 알림을 누르면 그 창을 앞으로 가져오고 다이얼로그를 다시 열게 한다. 앱 setup 에서 한 번 부른다.
pub fn install<R: Runtime>(app: &AppHandle<R>) {
    let handle = app.clone();
    let registered = app.notification().on_action(move |performed| {
        if performed.action_id() != "tap" {
            return;
        }
        let Some(id) = performed.notification().map(|n| n.id()) else {
            return;
        };
        let Some((label, request)) = handle.state::<ApprovalNotices>().tapped(id) else {
            return;
        };
        reveal(&handle, &label, request);
    });
    if let Err(error) = registered {
        tracing::warn!(%error, "could not listen for clicks on approval notifications");
    }
}

fn reveal<R: Runtime>(app: &AppHandle<R>, label: &str, request: u64) {
    let Some(window) = app.get_webview_window(label) else {
        return;
    };
    let _ = window.unminimize();
    let _ = window.show();
    let _ = window.set_focus();
    if let Err(error) = app.emit_to(label, REVEAL_EVENT, request) {
        tracing::warn!(%error, "could not reopen the approval dialog");
    }
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};

    use super::*;

    /// 가짜 알림: 권한과 실패를 정하고, 낸 알림을 모은다.
    struct FakeNotifier {
        permitted: bool,
        fails: bool,
        shown: RefCell<Vec<(i32, String, String)>>,
    }

    impl FakeNotifier {
        fn new(permitted: bool) -> Self {
            Self {
                permitted,
                fails: false,
                shown: RefCell::default(),
            }
        }
    }

    impl Notifier for FakeNotifier {
        fn permitted(&self) -> bool {
            self.permitted
        }

        fn show(&self, id: i32, title: &str, body: &str) -> Result<(), String> {
            if self.fails {
                return Err("no notification server".into());
            }
            self.shown
                .borrow_mut()
                .push((id, title.to_owned(), body.to_owned()));
            Ok(())
        }
    }

    fn announce(
        notices: &ApprovalNotices,
        window: &str,
        in_front: bool,
        request: u64,
        notifier: &FakeNotifier,
        attention: &Cell<u32>,
    ) -> Notice {
        notices.announce(
            window,
            in_front,
            request,
            "Pitwall approval request",
            "agent is waiting for approval of fake_write",
            notifier,
            || attention.set(attention.get() + 1),
        )
    }

    #[test]
    fn a_window_in_front_gets_no_notification() {
        let notices = ApprovalNotices::default();
        let notifier = FakeNotifier::new(true);
        let attention = Cell::new(0);
        assert_eq!(
            announce(&notices, "project-1", true, 1, &notifier, &attention),
            Notice::InFront
        );
        assert!(notifier.shown.borrow().is_empty());
        assert_eq!(attention.get(), 0);
    }

    #[test]
    fn a_window_not_in_front_is_notified_once_until_it_comes_to_the_front() {
        let notices = ApprovalNotices::default();
        let notifier = FakeNotifier::new(true);
        let attention = Cell::new(0);
        assert_eq!(
            announce(&notices, "project-1", false, 1, &notifier, &attention),
            Notice::Notified
        );
        let shown = notifier.shown.borrow().clone();
        assert_eq!(shown.len(), 1);
        assert_eq!(shown[0].1, "Pitwall approval request");
        assert_eq!(shown[0].2, "agent is waiting for approval of fake_write");
        // 누르면 그 창과 요청.
        assert_eq!(notices.tapped(shown[0].0), Some(("project-1".into(), 1)));
        // 앞에 오기 전의 다음 요청은 알리지 않는다. 다른 창은 따로 센다.
        assert_eq!(
            announce(&notices, "project-1", false, 2, &notifier, &attention),
            Notice::AlreadyAnnounced
        );
        assert_eq!(
            announce(&notices, "welcome", false, 3, &notifier, &attention),
            Notice::Notified
        );
        assert_eq!(notifier.shown.borrow().len(), 2);
        // 앞에 왔다 가면 다시 알린다.
        notices.focused("project-1");
        assert_eq!(
            notices.tapped(shown[0].0),
            None,
            "앞에 온 창의 알림은 잊는다"
        );
        assert_eq!(
            announce(&notices, "project-1", false, 4, &notifier, &attention),
            Notice::Notified
        );
        assert_eq!(notifier.shown.borrow().len(), 3);
        assert_eq!(attention.get(), 0);
    }

    #[test]
    fn without_permission_the_window_asks_for_attention() {
        let notices = ApprovalNotices::default();
        let notifier = FakeNotifier::new(false);
        let attention = Cell::new(0);
        assert_eq!(
            announce(&notices, "project-1", false, 1, &notifier, &attention),
            Notice::Attention
        );
        assert!(notifier.shown.borrow().is_empty());
        assert_eq!(attention.get(), 1);
        assert_eq!(
            announce(&notices, "project-1", false, 2, &notifier, &attention),
            Notice::AlreadyAnnounced
        );
        assert_eq!(attention.get(), 1);
    }

    #[test]
    fn a_failed_notification_falls_back_to_attention() {
        let notices = ApprovalNotices::default();
        let notifier = FakeNotifier {
            fails: true,
            ..FakeNotifier::new(true)
        };
        let attention = Cell::new(0);
        assert_eq!(
            announce(&notices, "project-1", false, 1, &notifier, &attention),
            Notice::Attention
        );
        assert_eq!(attention.get(), 1);
    }

    #[test]
    fn the_notice_text_loses_invisible_characters_and_is_bounded() {
        assert_eq!(clean("claude\u{200B}code\u{202E}\nx"), "claudecodex");
        let long = "a".repeat(NOTICE_CHARS + 5);
        let cleaned = clean(&long);
        assert_eq!(cleaned.chars().count(), NOTICE_CHARS + 1);
        assert!(cleaned.ends_with('…'));
        assert_eq!(clean(&"a".repeat(NOTICE_CHARS)), "a".repeat(NOTICE_CHARS));
    }
}
