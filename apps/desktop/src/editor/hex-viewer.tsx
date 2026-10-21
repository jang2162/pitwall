// hex 뷰어(12-m1-plan M1-23, 결정 M1-D10): 읽기 전용, 한 줄 = 오프셋 · 16바이트 · ASCII. 바이트는 화면에 보이는 줄이 걸친 64 KiB
// 페이지만 `fs.readBytes` 로 읽는다(preview-host.tsx 의 `readPage`) — 1 GB 파일의 끝으로 가면 그 페이지만 읽는다. 줄은 보이는
// 것만 그린다(가상 스크롤). 계산은 hex-model.ts.
//
// - 이동: 스크롤 · ↑↓ · PageUp/PageDown · Home/End(⌘↑ · ⌘↓). 오프셋으로 이동(⌘L, Windows · Linux Ctrl+G)은 이 뷰어에 포커스가
//   있을 때만이다 — 뿌리 요소의 키맵 컨텍스트가 `hexFocus` 라 텍스트 에디터의 줄로 이동(`editorFocus`)과 겹치지 않는다. 액션
//   (`hex.gotoOffset`, editor-actions.ts)은 활성 탭의 hex 뷰어(hex-controls.ts)를 부른다.
// - 고른 오프셋의 바이트는 hex · ASCII 두 칸 모두 강조한다.
// - 읽은 페이지는 `KEEP_PAGES` 장까지 두고 지금 화면에서 먼 것부터 버린다. 스크롤하는 동안의 중간 위치는 읽지 않도록 위치가
//   `READ_DELAY_MS` 동안 멈춘 뒤에 읽는다.

import * as Dialog from "@radix-ui/react-dialog";
import {
  type KeyboardEvent,
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
} from "react";

import { useKeymapPopup } from "../keymap/keymap-provider";
import { useProjectStrings } from "../layout/strings";
import { isComposingKey, keepOpenWhileComposing } from "../ui/ime";
import {
  BYTES_PER_ROW,
  firstRowAt,
  formatAscii,
  formatOffset,
  lastFirstRow,
  offsetDigits,
  pagesFor,
  parseOffset,
  rowCount,
  scrollHeight,
  scrollTopFor,
} from "./hex-model";
import { hexControls } from "./hex-controls";
import { formatBytes } from "./image-viewer";
import { HEX_PAGE_BYTES } from "./limits";
import { EditorNotice } from "./notice";
import { usePreviewHost } from "./preview-host";
import type { SelfReadProps } from "./viewers";

/** 줄 높이(px). 글꼴 크기와 함께 고정한다 — 가상 스크롤이 이 값으로 줄 번호를 센다. */
export const ROW_PX = 18;
const KEEP_PAGES = 16;
const READ_DELAY_MS = 16;

const BUTTON = "rounded-sm border border-ui-border px-3 py-1 hover:bg-ui-hover";

function GotoOffsetDialog({
  size,
  current,
  onGo,
  onClose,
}: {
  size: number;
  current: number;
  onGo(offset: number): void;
  onClose(): void;
}) {
  const T = useProjectStrings().editor.hex;
  const G = useProjectStrings().editor.gotoLine;
  useKeymapPopup(true);
  const digits = offsetDigits(size);
  const [text, setText] = useState(`0x${formatOffset(current, 1)}`);
  const [invalid, setInvalid] = useState(false);
  const go = () => {
    const offset = parseOffset(text, size);
    if (offset === undefined) return setInvalid(true);
    onGo(offset);
  };
  return (
    <Dialog.Root open onOpenChange={(open) => !open && onClose()}>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 bg-black/20" />
        <Dialog.Content
          data-testid="goto-offset-dialog"
          onEscapeKeyDown={keepOpenWhileComposing}
          className="sheet-top fixed left-1/2 flex w-[360px] max-w-[94vw] -translate-x-1/2 flex-col gap-2 rounded-b-md border border-t-0 border-ui-border bg-ui-panel px-4 py-3 text-ui-foreground shadow-lg"
        >
          <Dialog.Title className="text-sm font-semibold">{T.gotoTitle}</Dialog.Title>
          <Dialog.Description className="text-[12px] text-ui-muted-foreground">
            {T.gotoHint(
              `0x${formatOffset(current, digits)}`,
              `0x${formatOffset(Math.max(0, size - 1), digits)}`,
            )}
          </Dialog.Description>
          <input
            autoFocus
            aria-label={T.gotoInput}
            placeholder={T.gotoInput}
            value={text}
            spellCheck={false}
            onFocus={(e) => e.currentTarget.select()}
            onChange={(e) => {
              setText(e.target.value);
              setInvalid(false);
            }}
            onKeyDown={(e) => {
              if (e.key !== "Enter" || isComposingKey(e.nativeEvent)) return;
              e.preventDefault();
              go();
            }}
            aria-invalid={invalid}
            className="rounded-sm border border-ui-border bg-editor-background px-2 py-1 font-mono outline-none focus:border-ui-accent"
          />
          {invalid && (
            <div role="alert" className="text-[12px] text-ui-error">
              {T.gotoInvalid}
            </div>
          )}
          <div className="flex justify-end gap-2">
            <button type="button" className={BUTTON} onClick={onClose}>
              {G.cancel}
            </button>
            <button type="button" className={`${BUTTON} border-ui-accent`} onClick={go}>
              {G.go}
            </button>
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}

/** 바이트 하나(hex 두 자리 또는 ASCII 한 글자). 고른 오프셋이면 강조한다. */
function Cell({ text, selected }: { text: string; selected: boolean }) {
  return selected ? (
    <span data-selected="" className="bg-ui-active text-ui-foreground">
      {text}
    </span>
  ) : (
    text
  );
}

/** 탭 문서의 파일 크기. */
function sizeOf(doc: SelfReadProps["doc"]): number {
  return doc.status === "external" ? doc.stat.size : doc.size;
}

export function HexViewer({ panel, params, doc }: SelfReadProps) {
  const T = useProjectStrings().editor.hex;
  const host = usePreviewHost();
  const { path, worktree } = params;
  const size = sizeOf(doc);
  const scroller = useRef<HTMLDivElement>(null);
  const [viewport, setViewport] = useState(0);
  const [first, setFirst] = useState(0);
  const [selected, setSelected] = useState<number>();
  const [goto, setGoto] = useState(false);
  const [failed, setFailed] = useState<string>();
  const pages = useRef(new Map<number, Uint8Array>());
  const [, setLoadedPages] = useState(0);
  /** 이 값으로 스크롤을 옮겼다 — 그 스크롤 이벤트로 첫 줄을 다시 셈하지 않는다(축척 반올림으로 한 줄씩 어긋난다). */
  const programmatic = useRef<number>(undefined);

  /** 그리는 줄 수: 화면을 채우는 줄 + 아래에 반쯤 걸친 한 줄. */
  const visible = Math.max(1, Math.ceil(viewport / ROW_PX)) + 1;
  const max = lastFirstRow(size, Math.floor(viewport / ROW_PX));
  const digits = offsetDigits(size);

  // 파일이 바뀌면 읽은 것을 버린다.
  useEffect(() => {
    pages.current = new Map();
    setFirst(0);
    setSelected(undefined);
    setFailed(undefined);
  }, [path, worktree, size]);

  useLayoutEffect(() => {
    const el = scroller.current;
    if (!el) return;
    const measure = () => setViewport(el.clientHeight);
    measure();
    if (typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(measure);
    observer.observe(el);
    return () => observer.disconnect();
  }, []);

  // 화면의 줄이 걸친 페이지 중 없는 것을 읽는다.
  useEffect(() => {
    const need = pagesFor(first, visible, size).filter((p) => !pages.current.has(p));
    if (need.length === 0) return;
    let current = true;
    const timer = setTimeout(() => {
      for (const page of need) {
        const offset = page * HEX_PAGE_BYTES;
        host.readPage(path, worktree, offset, Math.min(HEX_PAGE_BYTES, size - offset)).then(
          ({ bytes }) => {
            if (!current) return;
            const kept = pages.current;
            kept.set(page, bytes);
            const here = Math.floor((first * BYTES_PER_ROW) / HEX_PAGE_BYTES);
            for (const p of [...kept.keys()].sort(
              (a, b) => Math.abs(b - here) - Math.abs(a - here),
            ))
              if (kept.size > KEEP_PAGES) kept.delete(p);
            setLoadedPages((n) => n + 1);
          },
          (e: unknown) => current && setFailed(e instanceof Error ? e.message : String(e)),
        );
      }
    }, READ_DELAY_MS);
    return () => {
      current = false;
      clearTimeout(timer);
    };
  }, [host, path, worktree, size, first, visible]);

  const moveTo = useCallback(
    (row: number) => {
      const next = Math.max(0, Math.min(max, row));
      setFirst(next);
      const el = scroller.current;
      if (el) {
        const top = scrollTopFor(next, size, ROW_PX, viewport);
        programmatic.current = top;
        el.scrollTop = top;
      }
    },
    [max, size, viewport],
  );

  const gotoOffset = useCallback(
    (offset: number) => {
      setSelected(offset);
      const row = Math.floor(offset / BYTES_PER_ROW);
      // 화면 안이면 그대로 두고, 밖이면 그 줄을 위에서 세 번째 줄쯤에 둔다.
      if (row < first || row >= first + Math.floor(viewport / ROW_PX)) moveTo(row - 2);
    },
    [first, viewport, moveTo],
  );

  useEffect(() => {
    hexControls.set(panel.id, { gotoOffset: () => setGoto(true) });
    return () => {
      hexControls.delete(panel.id);
    };
  }, [panel.id]);

  const onScroll = () => {
    const el = scroller.current;
    if (!el) return;
    if (programmatic.current !== undefined && Math.abs(el.scrollTop - programmatic.current) < 1) {
      programmatic.current = undefined;
      return;
    }
    programmatic.current = undefined;
    setFirst(firstRowAt(el.scrollTop, size, ROW_PX, viewport));
  };

  const onKeyDown = (e: KeyboardEvent) => {
    const page = Math.max(1, Math.floor(viewport / ROW_PX) - 1);
    const command = e.metaKey || e.ctrlKey;
    let row: number | undefined;
    if (e.key === "ArrowDown") row = command ? max : first + 1;
    else if (e.key === "ArrowUp") row = command ? 0 : first - 1;
    else if (e.key === "PageDown") row = first + page;
    else if (e.key === "PageUp") row = first - page;
    else if (e.key === "Home") row = 0;
    else if (e.key === "End") row = max;
    if (row === undefined) return;
    e.preventDefault();
    moveTo(row);
  };

  if (failed) return <EditorNotice text={T.failed(failed)} />;

  const rows: number[] = [];
  for (let r = first; r < Math.min(rowCount(size), first + visible); r++) rows.push(r);
  const byteAt = (offset: number): number | undefined => {
    if (offset >= size) return undefined;
    const page = Math.floor(offset / HEX_PAGE_BYTES);
    return pages.current.get(page)?.[offset - page * HEX_PAGE_BYTES];
  };

  return (
    <div
      data-testid="hex-viewer"
      // 키맵 컨텍스트(src/keymap/context.ts): 가장 가까운 것을 보므로 에디터 탭의 `editorFocus` 를 가린다.
      data-keymap-context="hexFocus"
      className="flex h-full min-h-0 flex-col bg-editor-background"
    >
      <div className="flex h-[26px] flex-none items-center gap-2 border-b border-ui-border bg-ui-panel px-2 text-[12px] text-ui-muted-foreground">
        <span>{T.label}</span>
        <span>{formatBytes(size)}</span>
        <button
          type="button"
          data-hex-goto=""
          onClick={() => setGoto(true)}
          className="ml-auto rounded-sm px-2 py-px text-ui-foreground hover:bg-ui-hover"
        >
          {T.gotoTitle}
        </button>
      </div>
      <div
        ref={scroller}
        data-hex-scroll=""
        tabIndex={0}
        onScroll={onScroll}
        onKeyDown={onKeyDown}
        className="relative min-h-0 flex-1 overflow-auto font-mono text-[12px] outline-none select-text"
      >
        <div style={{ height: scrollHeight(size, ROW_PX) }} />
        <div
          className="absolute right-0 left-0"
          style={{ top: scrollTopFor(first, size, ROW_PX, viewport) }}
        >
          {rows.map((r) => {
            const start = r * BYTES_PER_ROW;
            const bytes = Array.from({ length: BYTES_PER_ROW }, (_, i) => byteAt(start + i));
            const ascii = formatAscii(bytes);
            return (
              <div
                key={r}
                data-hex-row={start}
                className="flex gap-4 px-2 whitespace-pre"
                style={{ height: ROW_PX, lineHeight: `${ROW_PX}px` }}
              >
                <span className="text-ui-muted-foreground">{formatOffset(start, digits)}</span>
                <span data-hex-bytes="">
                  {bytes.map((b, i) => (
                    <span key={i}>
                      {i === 0 ? "" : i === 8 ? "  " : " "}
                      <Cell
                        text={
                          b === undefined ? "  " : b.toString(16).toUpperCase().padStart(2, "0")
                        }
                        selected={start + i === selected}
                      />
                    </span>
                  ))}
                </span>
                <span data-hex-ascii="">
                  {[...ascii].map((c, i) => (
                    <Cell key={i} text={c} selected={start + i === selected} />
                  ))}
                </span>
              </div>
            );
          })}
        </div>
      </div>
      {goto && (
        <GotoOffsetDialog
          size={size}
          current={selected ?? first * BYTES_PER_ROW}
          onGo={(offset) => {
            setGoto(false);
            gotoOffset(offset);
            scroller.current?.focus();
          }}
          onClose={() => {
            setGoto(false);
            scroller.current?.focus();
          }}
        />
      )}
    </div>
  );
}
