// 액션 레지스트리(docs/design/10-ui.md §1 원칙 2 「모든 기능은 액션으로 등록」). 창마다 하나를 두고, 그 창에서 실행할 수
// 있는 액션만 등록한다 — 키맵에 키가 있어도 등록되지 않은 액션은 그 창에서 키를 가져가지 않는다(엔진).

export interface Action {
  /** 키맵 파일 · 메뉴가 가리키는 이름. `<영역>.<동작>`(예: `toolWindow.project`). */
  readonly id: string;
  /** 액션 찾기 · 메뉴에 보이는 이름. */
  readonly label: string;
  /** 액션 찾기에서 함께 찾는 낱말(영문 이름 등). */
  readonly keywords?: readonly string[];
  readonly run: () => void | Promise<void>;
  /** 키를 누르고 있어 반복되는 keydown 으로도 실행하는가. 없으면 거짓(엔진 머리 주석). */
  readonly repeat?: boolean;
  /** 지금 실행할 수 없으면 거짓. 없으면 늘 참. 거짓이면 키도 가져가지 않는다. */
  readonly enabled?: () => boolean;
}

export class ActionRegistry {
  private readonly actions = new Map<string, Action>();
  private readonly listeners = new Set<() => void>();
  private version = 0;

  /** 등록하고, 해제 함수를 돌려준다. 같은 id 를 다시 등록하면 앞의 것을 덮는다(해제는 자기 것만 지운다). */
  register(action: Action): () => void {
    this.actions.set(action.id, action);
    this.changed();
    return () => {
      if (this.actions.get(action.id) !== action) return;
      this.actions.delete(action.id);
      this.changed();
    };
  }

  get(id: string): Action | undefined {
    return this.actions.get(id);
  }

  /** 지금 실행할 수 있는가. 등록되지 않았으면 거짓. */
  isEnabled(id: string): boolean {
    const action = this.actions.get(id);
    return action !== undefined && (action.enabled?.() ?? true);
  }

  allowsRepeat(id: string): boolean {
    return this.actions.get(id)?.repeat === true;
  }

  list(): Action[] {
    return [...this.actions.values()];
  }

  /** 실행했으면 참. 등록되지 않았거나 실행할 수 없으면 거짓. 실행 중 오류는 로그로 남기고 삼킨다. */
  run(id: string): boolean {
    const action = this.actions.get(id);
    if (!action || !(action.enabled?.() ?? true)) return false;
    try {
      void Promise.resolve(action.run()).catch((e: unknown) =>
        console.error("[keymap] action failed", id, e),
      );
    } catch (e) {
      console.error("[keymap] action failed", id, e);
    }
    return true;
  }

  /** React `useSyncExternalStore` 용. */
  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  };

  getVersion = (): number => this.version;

  private changed() {
    this.version++;
    for (const l of this.listeners) l();
  }
}
