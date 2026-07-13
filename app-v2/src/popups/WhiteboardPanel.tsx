import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from 'react';
import {
  CaptureUpdateAction,
  Excalidraw,
  exportToBlob,
  MainMenu,
  restore,
  serializeAsJSON,
} from '@excalidraw/excalidraw';
import type { BinaryFiles, ExcalidrawImperativeAPI } from '@excalidraw/excalidraw/types';
import '@excalidraw/excalidraw/index.css';
import type { ComposerAttachment } from '../chrome/InputBar';
import type {
  WhiteboardCanvasLoadResult,
  WhiteboardCanvasSnapshotResult,
} from '../pty-client';

type WhiteboardPanelProps = {
  projectRoot?: string | null;
  onClose: () => void;
  onInsertDrawing: (attachment: ComposerAttachment) => void;
  onLoadCanvas: (projectRoot?: string | null, pagePath?: string | null) => Promise<WhiteboardCanvasLoadResult>;
  onSaveCanvas: (request: {
    projectRoot?: string | null;
    pagePath: string;
    dataJson: string;
  }) => Promise<WhiteboardCanvasLoadResult>;
  onRenamePage: (request: {
    projectRoot?: string | null;
    pagePath: string;
    title: string;
  }) => Promise<WhiteboardCanvasLoadResult>;
  onSaveSnapshot: (request: {
    projectRoot?: string | null;
    pagePath: string;
    pngBytes: number[];
  }) => Promise<WhiteboardCanvasSnapshotResult>;
};

type WhiteboardPage = WhiteboardCanvasLoadResult['pages'][number];

const SAVE_DEBOUNCE_MS = 700;
const EXTERNAL_REFRESH_MS = 1600;

// Excalidraw tool types that expose stroke/fill/opacity controls. Selecting one of these is
// enough to reveal the style palette button, so colour/width/opacity can be pre-set before
// anything is drawn (not only after an element is selected).
const STYLEABLE_TOOLS = new Set(['freedraw', 'rectangle', 'diamond', 'ellipse', 'arrow', 'line', 'text']);

function hasExcalidrawEscapableState(api: ExcalidrawImperativeAPI | null): boolean {
  const appState = api?.getAppState();
  if (!appState) return false;
  return Boolean(
    appState.openDialog
    || appState.openMenu
    || appState.openPopup
    || appState.contextMenu
    || appState.editingTextElement
    || appState.editingLinearElement
    || appState.activeEmbeddable,
  );
}

function defaultExcalidrawJson(): string {
  return JSON.stringify({
    type: 'excalidraw',
    version: 2,
    source: 'https://kota.local',
    elements: [],
    appState: {
      viewBackgroundColor: '#f5f0e5',
      gridSize: null,
    },
    files: {},
  });
}

function restoreCanvasData(dataJson: string) {
  const raw = JSON.parse(dataJson || defaultExcalidrawJson());
  if (!Array.isArray(raw.elements)) throw new Error('Canvas file is missing elements[]');
  return restore({
    elements: raw.elements,
    appState: {
      viewBackgroundColor: '#f5f0e5',
      ...(raw.appState ?? {}),
    },
    files: raw.files ?? {},
  }, null, null, {
    refreshDimensions: false,
    repairBindings: true,
  });
}

function pathForPrompt(path: string): string {
  const marker = '/project-memory/';
  const index = path.indexOf(marker);
  if (index >= 0) return path.slice(index + 1);
  return path;
}

function canvasPrompt(snapshotPath: string, pagePath: string): string {
  return [
    '[canvas]',
    `snapshot: ${pathForPrompt(snapshotPath)}`,
    `editable: ${pathForPrompt(pagePath)}`,
    'You may edit the editable .excalidraw file directly. Keep it valid Excalidraw JSON.',
    '[/canvas]',
  ].join('\n');
}

function simpleHash(text: string): string {
  let hash = 0;
  for (let index = 0; index < text.length; index += 1) {
    hash = ((hash << 5) - hash + text.charCodeAt(index)) | 0;
  }
  return `${text.length}:${hash}`;
}

function pagePathForIndex(canvasDir: string, index: number): string {
  return `${canvasDir.replace(/\/+$/, '')}/pages/page-${String(index).padStart(3, '0')}.excalidraw`;
}

function lastCanvasPageStorageKey(projectRoot?: string | null): string {
  return `kota:whiteboard:last-page:${projectRoot ?? 'default'}`;
}

function readLastCanvasPage(projectRoot?: string | null): string | null {
  try {
    return window.localStorage.getItem(lastCanvasPageStorageKey(projectRoot));
  } catch {
    return null;
  }
}

function writeLastCanvasPage(projectRoot: string | null | undefined, pagePath: string) {
  try {
    window.localStorage.setItem(lastCanvasPageStorageKey(projectRoot), pagePath);
  } catch {
    // Ignore storage failures; the page still switches normally.
  }
}

function syncRangeValueBubbles(root: HTMLElement | null) {
  if (!root) return;
  root.querySelectorAll<HTMLElement>('.range-wrapper').forEach((wrapper) => {
    const input = wrapper.querySelector<HTMLInputElement>('.range-input');
    const bubble = wrapper.querySelector<HTMLElement>('.value-bubble');
    if (!input || !bubble) return;
    const inputWidth = input.offsetWidth;
    if (!inputWidth) return;
    const min = Number(input.min || 0);
    const max = Number(input.max || 100);
    const value = Number(input.value);
    if (!Number.isFinite(value) || max <= min) return;
    const thumbWidth = 16;
    const ratio = Math.max(0, Math.min(1, (value - min) / (max - min)));
    const position = ratio * (inputWidth - thumbWidth) + thumbWidth / 2;
    const nextLeft = `${position}px`;
    if (bubble.style.left !== nextLeft) {
      bubble.style.left = nextLeft;
    }
  });
}

async function blobToBytes(blob: Blob): Promise<number[]> {
  return Array.from(new Uint8Array(await blob.arrayBuffer()));
}

export function WhiteboardPanel({
  projectRoot,
  onClose,
  onInsertDrawing,
  onLoadCanvas,
  onSaveCanvas,
  onRenamePage,
  onSaveSnapshot,
}: WhiteboardPanelProps) {
  const apiRef = useRef<ExcalidrawImperativeAPI | null>(null);
  const saveTimerRef = useRef<number | null>(null);
  const applyingExternalRef = useRef(false);
  const lastHashRef = useRef('');
  const lastModifiedRef = useRef(0);
  const latestSceneRef = useRef<{ args: Parameters<typeof serializeAsJSON>; pagePath: string } | null>(null);
  const titleButtonRef = useRef<HTMLButtonElement | null>(null);
  const pagesMenuRef = useRef<HTMLDivElement | null>(null);
  const editorRootRef = useRef<HTMLDivElement | null>(null);
  const overlayRef = useRef<HTMLDivElement | null>(null);
  const [canvas, setCanvas] = useState<WhiteboardCanvasLoadResult | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [status, setStatus] = useState('Loading canvas...');
  const [saving, setSaving] = useState(false);
  const [pagesMenuOpen, setPagesMenuOpen] = useState(false);
  const [renamingPagePath, setRenamingPagePath] = useState<string | null>(null);
  const [renameDraft, setRenameDraft] = useState('');
  const [styleControlsAvailable, setStyleControlsAvailable] = useState(false);
  const [shapePaletteOpen, setShapePaletteOpen] = useState(false);

  useEffect(() => {
    // The whiteboard is modal: pull focus off the chat composer when it opens, so pasted
    // screenshots and tool shortcuts (1/2/3/4…) land on the canvas instead of leaking into
    // the composer that sits behind the overlay.
    const active = document.activeElement;
    if (active instanceof HTMLElement && !overlayRef.current?.contains(active)) {
      active.blur();
    }
    overlayRef.current?.focus({ preventScroll: true });
  }, []);

  useEffect(() => {
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key !== 'Escape') return;
      if (pagesMenuOpen) {
        event.preventDefault();
        setPagesMenuOpen(false);
        return;
      }
      if (shapePaletteOpen) {
        event.preventDefault();
        setShapePaletteOpen(false);
        return;
      }
      if (hasExcalidrawEscapableState(apiRef.current)) return;
      event.preventDefault();
      onClose();
    };
    document.addEventListener('keydown', handleKeyDown, true);
    return () => document.removeEventListener('keydown', handleKeyDown, true);
  }, [onClose, pagesMenuOpen, shapePaletteOpen]);

  useEffect(() => {
    if (!pagesMenuOpen) return undefined;
    const handlePointerDown = (event: PointerEvent) => {
      const target = event.target;
      if (!(target instanceof Node)) return;
      if (pagesMenuRef.current?.contains(target) || titleButtonRef.current?.contains(target)) return;
      setPagesMenuOpen(false);
    };
    document.addEventListener('pointerdown', handlePointerDown, true);
    return () => document.removeEventListener('pointerdown', handlePointerDown, true);
  }, [pagesMenuOpen]);

  useEffect(() => {
    if (pagesMenuOpen) return;
    setRenamingPagePath(null);
    setRenameDraft('');
  }, [pagesMenuOpen]);

  useEffect(() => {
    if (styleControlsAvailable) return;
    setShapePaletteOpen(false);
  }, [styleControlsAvailable]);

  const initialData = useMemo(() => {
    if (!canvas) return null;
    try {
      return restoreCanvasData(canvas.dataJson);
    } catch {
      return restoreCanvasData(defaultExcalidrawJson());
    }
  }, [canvas?.pagePath]);

  const activePage = useMemo(() => {
    if (!canvas) return null;
    return canvas.pages.find((page) => page.path === canvas.pagePath) ?? canvas.pages[0] ?? null;
  }, [canvas]);

  const applyCanvasResult = useCallback((next: WhiteboardCanvasLoadResult, options: { updateScene: boolean }) => {
    setCanvas(next);
    setLoadError(null);
    const nextPage = next.pages.find((page) => page.path === next.pagePath);
    setStatus(`Editing ${nextPage?.title ?? next.pageId}`);
    lastHashRef.current = simpleHash(next.dataJson);
    lastModifiedRef.current = next.modifiedMs;
    writeLastCanvasPage(projectRoot, next.pagePath);
    if (options.updateScene && apiRef.current) {
      try {
        const restored = restoreCanvasData(next.dataJson);
        applyingExternalRef.current = true;
        apiRef.current.updateScene({
          elements: restored.elements,
          appState: restored.appState as never,
          captureUpdate: CaptureUpdateAction.NEVER,
        });
        if (restored.files) {
          apiRef.current.addFiles(Object.values(restored.files));
        }
        window.setTimeout(() => {
          applyingExternalRef.current = false;
        }, 0);
      } catch (err) {
        setLoadError(err instanceof Error ? err.message : String(err));
      }
    }
  }, [projectRoot]);

  useEffect(() => {
    let cancelled = false;
    void onLoadCanvas(projectRoot, readLastCanvasPage(projectRoot)).then((next) => {
      if (cancelled) return;
      applyCanvasResult(next, { updateScene: false });
    }).catch((err) => {
      if (cancelled) return;
      setLoadError(err instanceof Error ? err.message : String(err));
      setStatus('Could not load canvas');
    });
    return () => {
      cancelled = true;
    };
  }, [applyCanvasResult, onLoadCanvas, projectRoot]);

  useEffect(() => {
    const root = editorRootRef.current;
    // Only sync the slider value bubbles while the style palette is actually visible.
    // Watching the whole Excalidraw subtree for class/style mutations (which fire on every
    // pointer move and re-render) forced a layout-reading rAF on every frame — the main
    // source of whiteboard jank. Scope it to when the palette is open and to `value` changes.
    if (!root || !shapePaletteOpen) return undefined;
    let frame = 0;
    const sync = () => {
      if (frame) window.cancelAnimationFrame(frame);
      frame = window.requestAnimationFrame(() => {
        frame = 0;
        syncRangeValueBubbles(root);
      });
    };
    sync();
    const observer = typeof MutationObserver === 'undefined' ? null : new MutationObserver(sync);
    observer?.observe(root, {
      childList: true,
      subtree: true,
      attributes: true,
      attributeFilter: ['value'],
    });
    const resizeObserver = typeof ResizeObserver === 'undefined' ? null : new ResizeObserver(sync);
    resizeObserver?.observe(root);
    root.addEventListener('input', sync, true);
    return () => {
      if (frame) window.cancelAnimationFrame(frame);
      observer?.disconnect();
      resizeObserver?.disconnect();
      root.removeEventListener('input', sync, true);
    };
  }, [canvas?.pagePath, shapePaletteOpen]);

  const saveData = useCallback(async (dataJson: string, pagePath: string) => {
    const hash = simpleHash(dataJson);
    if (hash === lastHashRef.current) return;
    lastHashRef.current = hash;
    setStatus('Saving canvas...');
    try {
      const next = await onSaveCanvas({ projectRoot, pagePath, dataJson });
      lastModifiedRef.current = next.modifiedMs;
      setCanvas(next);
      const nextPage = next.pages.find((page) => page.path === next.pagePath);
      setStatus(`Saved ${nextPage?.title ?? next.pageId}`);
    } catch (err) {
      setLoadError(err instanceof Error ? err.message : String(err));
      setStatus('Save failed');
    }
  }, [onSaveCanvas, projectRoot]);

  const scheduleSave = useCallback(() => {
    if (saveTimerRef.current) window.clearTimeout(saveTimerRef.current);
    saveTimerRef.current = window.setTimeout(() => {
      // Serialize once, when the debounce settles — not on every onChange. serializeAsJSON walks
      // the whole scene, so doing it per change made large canvases stutter while drawing. The
      // snapshot carries its own pagePath so a page switch mid-debounce still saves to the
      // page the edit happened on.
      const scene = latestSceneRef.current;
      if (!scene) return;
      const dataJson = serializeAsJSON(...scene.args);
      void saveData(dataJson, scene.pagePath);
    }, SAVE_DEBOUNCE_MS);
  }, [saveData]);

  useEffect(() => () => {
    if (saveTimerRef.current) window.clearTimeout(saveTimerRef.current);
  }, []);

  useEffect(() => {
    if (!canvas?.pagePath) return undefined;
    const timer = window.setInterval(() => {
      void onLoadCanvas(projectRoot, canvas.pagePath).then((next) => {
        if (next.pagePath !== canvas.pagePath) return;
        // Cheap mtime guard first: only hash the (possibly large) canvas JSON when the file
        // actually changed on disk, instead of re-hashing the whole document every poll tick.
        if (next.modifiedMs === lastModifiedRef.current) return;
        if (simpleHash(next.dataJson) === lastHashRef.current) return;
        applyCanvasResult(next, { updateScene: true });
        setStatus('Reloaded external canvas edit');
      }).catch((err) => {
        setLoadError(err instanceof Error ? err.message : String(err));
      });
    }, EXTERNAL_REFRESH_MS);
    return () => window.clearInterval(timer);
  }, [applyCanvasResult, canvas?.pagePath, onLoadCanvas, projectRoot]);

  const switchPage = useCallback((pagePath: string) => {
    setStatus('Loading page...');
    setPagesMenuOpen(false);
    setRenamingPagePath(null);
    void onLoadCanvas(projectRoot, pagePath)
      .then((next) => applyCanvasResult(next, { updateScene: false }))
      .catch((err) => {
        setLoadError(err instanceof Error ? err.message : String(err));
        setStatus('Could not load page');
      });
  }, [applyCanvasResult, onLoadCanvas, projectRoot]);

  const renamePage = useCallback((page: WhiteboardPage, title: string) => {
    const nextTitle = title.trim().replace(/\s+/g, ' ');
    if (!nextTitle || nextTitle === page.title) return;
    setStatus('Renaming page...');
    void onRenamePage({
      projectRoot,
      pagePath: page.path,
      title: nextTitle,
    }).then((next) => {
      applyCanvasResult(next, { updateScene: false });
      setStatus(`Renamed ${nextTitle}`);
    }).catch((err) => {
      setLoadError(err instanceof Error ? err.message : String(err));
      setStatus('Rename failed');
    });
  }, [applyCanvasResult, onRenamePage, projectRoot]);

  const finishRenamePage = useCallback((page: WhiteboardPage, title: string) => {
    setRenamingPagePath(null);
    setRenameDraft('');
    renamePage(page, title);
  }, [renamePage]);

  const addPage = useCallback(() => {
    if (!canvas) return;
    const used = new Set(canvas.pages.map((page) => page.path));
    let index = canvas.pages.length + 1;
    let pagePath = pagePathForIndex(canvas.canvasDir, index);
    while (used.has(pagePath)) {
      index += 1;
      pagePath = pagePathForIndex(canvas.canvasDir, index);
    }
    setStatus('Creating page...');
    void onSaveCanvas({
      projectRoot,
      pagePath,
      dataJson: defaultExcalidrawJson(),
    }).then((next) => {
      applyCanvasResult(next, { updateScene: false });
      setPagesMenuOpen(false);
    })
      .catch((err) => {
        setLoadError(err instanceof Error ? err.message : String(err));
        setStatus('Could not create page');
      });
  }, [applyCanvasResult, canvas, onSaveCanvas, projectRoot]);

  const insertToChat = useCallback(async () => {
    const api = apiRef.current;
    if (!api || !canvas) return;
    setSaving(true);
    setStatus('Creating canvas snapshot...');
    try {
      const elements = api.getSceneElements();
      const appState = api.getAppState();
      const files = api.getFiles();
      const dataJson = serializeAsJSON(elements, appState, files, 'local');
      await saveData(dataJson, canvas.pagePath);
      const blob = await exportToBlob({
        elements,
        appState: {
          ...appState,
          exportBackground: true,
        },
        files,
        mimeType: 'image/png',
        maxWidthOrHeight: 1400,
        exportPadding: 16,
      });
      const snapshot = await onSaveSnapshot({
        projectRoot,
        pagePath: canvas.pagePath,
        pngBytes: await blobToBytes(blob),
      });
      onInsertDrawing({
        path: snapshot.snapshotPath,
        name: activePage?.title ?? 'drawing',
        kind: 'drawing',
        prompt: canvasPrompt(snapshot.snapshotPath, snapshot.pagePath),
      });
      setStatus('Inserted canvas into composer');
      onClose();
    } catch (err) {
      setLoadError(err instanceof Error ? err.message : String(err));
      setStatus('Insert failed');
    } finally {
      setSaving(false);
    }
  }, [activePage?.title, canvas, onClose, onInsertDrawing, onSaveSnapshot, projectRoot, saveData]);

  return (
    <div ref={overlayRef} className="wb-overlay" role="dialog" aria-label="Whiteboard" tabIndex={-1}>
      <div className="wb-window">
        <div className="wb-topbar">
          <div className="wb-heading">
            <div className="wb-title-row">
              <div className="wb-title" title={activePage?.title}>{activePage?.title ?? 'Canvas'}</div>
              <button
                ref={titleButtonRef}
                type="button"
                className={`wb-title-menu-button ${pagesMenuOpen ? 'open' : ''}`}
                title="Open page menu"
                aria-haspopup="menu"
                aria-expanded={pagesMenuOpen}
                onClick={() => setPagesMenuOpen((open) => !open)}
              >
                <svg viewBox="0 0 24 24" aria-hidden="true">
                  <path d="M8 10L12 14L16 10" />
                </svg>
              </button>
            </div>
            <div className="wb-subtitle">{status}</div>
            {pagesMenuOpen && (
              <div ref={pagesMenuRef} className="wb-page-menu" role="menu" aria-label="Whiteboard pages">
                <button type="button" className="wb-page-menu-add" onClick={addPage} disabled={!canvas}>
                  + New page
                </button>
                <div className="wb-page-menu-list">
                  {(canvas?.pages ?? []).map((page) => (
                    <div
                      key={page.path}
                      className={`wb-page-menu-row ${page.path === canvas?.pagePath ? 'active' : ''}`}
                    >
                      {renamingPagePath === page.path ? (
                        <div className="wb-page-menu-rename">
                          <input
                            key={`${page.path}:${page.title}`}
                            className="wb-page-menu-title"
                            value={renameDraft}
                            aria-label={`Rename ${page.title}`}
                            autoFocus
                            onChange={(event) => setRenameDraft(event.currentTarget.value)}
                            onClick={(event) => event.stopPropagation()}
                            onFocus={(event) => event.currentTarget.select()}
                            onBlur={(event) => finishRenamePage(page, event.currentTarget.value)}
                            onKeyDown={(event) => {
                              event.stopPropagation();
                              if (event.key === 'Enter') {
                                event.preventDefault();
                                event.currentTarget.blur();
                              }
                              if (event.key === 'Escape') {
                                event.preventDefault();
                                setRenamingPagePath(null);
                                setRenameDraft('');
                              }
                            }}
                          />
                          <button
                            type="button"
                            className="wb-page-menu-confirm"
                            aria-label={`Confirm rename ${page.title}`}
                            onMouseDown={(event) => event.preventDefault()}
                            onClick={() => finishRenamePage(page, renameDraft)}
                          >
                            <svg viewBox="0 0 24 24" aria-hidden="true">
                              <path d="M5 12.5L10 17L19 7" />
                            </svg>
                          </button>
                        </div>
                      ) : (
                        <>
                          <button
                            type="button"
                            className="wb-page-menu-select"
                            onClick={() => switchPage(page.path)}
                          >
                            <span>{page.title}</span>
                          </button>
                          <button
                            type="button"
                            className="wb-page-menu-edit"
                            onClick={() => {
                              setRenamingPagePath(page.path);
                              setRenameDraft(page.title);
                            }}
                            aria-label={`Rename ${page.title}`}
                          >
                            Rename
                          </button>
                        </>
                      )}
                    </div>
                  ))}
                </div>
              </div>
            )}
          </div>
          <div className="wb-actions">
            <button type="button" className="wb-action primary" onClick={insertToChat} disabled={!canvas || saving}>
              {saving ? 'Inserting...' : 'Insert to chat'}
            </button>
            <button type="button" className="wb-close" onClick={onClose} aria-label="Collapse whiteboard">
              <svg className="wb-close-icon" viewBox="0 0 24 24" aria-hidden="true">
                <path d="M8 10L12 14L16 10" />
              </svg>
            </button>
          </div>
        </div>

        <div className="wb-main">
          <div ref={editorRootRef} className={`wb-editor ${shapePaletteOpen ? 'shape-palette-open' : ''}`}>
            {styleControlsAvailable && (
              <button
                type="button"
                className={`wb-shape-palette-button ${shapePaletteOpen ? 'active' : ''}`}
                onClick={() => setShapePaletteOpen((open) => !open)}
                title={shapePaletteOpen ? 'Hide style palette' : 'Show style palette'}
                aria-label={shapePaletteOpen ? 'Hide style palette' : 'Show style palette'}
              >
                <span className="wb-shape-palette-icon" aria-hidden="true">
                  <span />
                  <span />
                  <span />
                  <span />
                </span>
              </button>
            )}
            {canvas && initialData ? (
              <Excalidraw
                key={canvas.pagePath}
                initialData={initialData}
                excalidrawAPI={(api) => {
                  apiRef.current = api;
                }}
                theme="light"
                autoFocus
                gridModeEnabled={false}
                handleKeyboardGlobally={false}
                name={canvas.pageId}
                onChange={(elements, appState, files: BinaryFiles) => {
                  if (applyingExternalRef.current) return;
                  const hasSelection = Object.keys(appState.selectedElementIds ?? {}).length > 0;
                  const toolType = appState.activeTool?.type;
                  const styleable = hasSelection || (toolType !== undefined && STYLEABLE_TOOLS.has(toolType));
                  setStyleControlsAvailable((current) => (current === styleable ? current : styleable));
                  if (canvas?.pagePath) {
                    latestSceneRef.current = { args: [elements, appState, files, 'local'], pagePath: canvas.pagePath };
                    scheduleSave();
                  }
                }}
              >
                <MainMenu>
                  <MainMenu.DefaultItems.ChangeCanvasBackground />
                  <MainMenu.Separator />
                  <MainMenu.DefaultItems.LoadScene />
                  <MainMenu.DefaultItems.SaveToActiveFile />
                  <MainMenu.DefaultItems.Export />
                  <MainMenu.DefaultItems.SaveAsImage />
                  <MainMenu.DefaultItems.SearchMenu />
                  <MainMenu.DefaultItems.Help />
                  <MainMenu.Separator />
                  <MainMenu.DefaultItems.ToggleTheme />
                  <MainMenu.DefaultItems.ClearCanvas />
                </MainMenu>
              </Excalidraw>
            ) : (
              <div className="wb-loading">Loading canvas...</div>
            )}
            {loadError && (
              <div className="wb-error" role="status">
                {loadError}
              </div>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}
