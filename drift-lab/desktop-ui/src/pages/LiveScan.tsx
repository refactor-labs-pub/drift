import { startTransition, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useNavigate } from "react-router-dom";

import IcicleChart from "../components/IcicleChart";
import CallGraphPanel from "../components/scan-views/CallGraphPanel";
import CallTreePanel from "../components/scan-views/CallTreePanel";
import StatisticsPanel from "../components/scan-views/StatisticsPanel";
import SearchBox from "../components/SearchBox";
import Tabs, { type TabItem } from "../components/Tabs";
import { useRunStore } from "../store/runStore";
import { matchFrameFilter, parseFrameFilter } from "../lib/frame_filter";
import {
  aggregateEventLog,
  cancelRealtimeTest,
  downloadEventLog,
  folderHasStaticScan,
  listEventLogs,
  listRealtimeProfiles,
  listScannedFolders,
  registerFolder,
  ScannedFolder,
  selectProjectPath,
  onLiveEventAgg,
  onLiveEventErr,
  onTestRealtimeProgress,
  realtimeApiKeyName,
  RealtimeProfile,
  secretStatus,
  selectEventLogFile,
  startLiveEventScan,
  startRealtimeEventStream,
  stopLiveEventScan,
  stopRealtimeEventStream,
  testRealtimeConnection,
  type EventLogFunctionStat,
  type EventLogMeta,
  type EventLogReport,
  type EventLogTreeNode,
} from "../lib/tauri";

/** Default observability-server URL the "Download" button hits. The
 *  user can override via the prompt that opens on click; we keep the
 *  default here so the common case (local dev / Tilt) is a single click. */
const DEFAULT_OBS_URL = "http://localhost:8080/events/log";

/**
 * `events.log` profiling viewer — snakeviz-style icicle chart over the
 * call graph plus a per-function table. Two entry points:
 *
 *   - **scan list**: every `.log` / `.jsonl` file in `~/.drift/event_logs/`
 *     is listed in the left rail. Click a row to load a one-shot
 *     aggregation of the file.
 *   - **live_scan**: pick a file via the system dialog; the backend
 *     re-aggregates every ~1s and pushes a fresh report over
 *     `event_log://aggregate`. The UI just listens — the aggregation lives
 *     server-side so the JSON over the wire stays small.
 *
 * View shape (top → bottom):
 *
 *   header  : breadcrumb + run button + live status
 *   summary : total time / events / services
 *   icicle  : flamegraph of the tree (click a bar to zoom; reset to home)
 *   table   : per-qualname rollup, sortable
 *
 * The page picks its mode from the active `scan` state — there is no
 * top-level "live or static" tab. A live scan that errors falls back to
 * the last successful aggregate so the chart doesn't flash empty.
 */
/** Top-level source selection. Persisted in component state only — the
 *  user's last choice doesn't survive an app restart for now (deferred). */
type SourceMode = "live-listen" | "load-from-file";

type Mode =
  | { kind: "idle" }
  | { kind: "loading"; path: string }
  | { kind: "static"; path: string; report: EventLogReport }
  | {
      // Existing file-tail live mode. Polls a JSONL file at ~1Hz.
      kind: "live";
      path: string;
      liveScanId: string;
      report: EventLogReport | null;
      lastError: string | null;
    }
  | {
      // Supabase Realtime live mode (Phase C). The Rust side runs the WSS
      // task + writes broadcasts to a JSONL file; the file-tail aggregator
      // emits aggregates on the SAME `event_log://aggregate` topic as the
      // file-poll path, keyed by `liveScanId`. The chart and table render
      // identically regardless of source.
      kind: "live-realtime";
      streamId: string;
      liveScanId: string;
      logPath: string;
      channel: string;
      report: EventLogReport | null;
      lastError: string | null;
    }
  | { kind: "error"; message: string };

export default function LiveScanPage() {
  const navigate = useNavigate();
  const [logs, setLogs] = useState<EventLogMeta[]>([]);
  const [logsLoading, setLogsLoading] = useState(true);
  const [mode, setMode] = useState<Mode>({ kind: "idle" });
  const liveIdRef = useRef<string | null>(null);
  const realtimeStreamIdRef = useRef<string | null>(null);

  // Top-level source selector. Defaults to "live-listen" because that's
  // the new headline feature; users with existing JSONL files flip to
  // "load-from-file" once and stay there.
  const [sourceMode, setSourceMode] = useState<SourceMode>("live-listen");

  // Loaded profile list + selected profile id (defaults to the active
  // one). The picker is the source of truth for "which Supabase project
  // is this scan against"; the channel/event inputs below are per-scan
  // overrides on top of the selected profile.
  const [profiles, setProfiles] = useState<RealtimeProfile[]>([]);
  const [selectedProfileId, setSelectedProfileId] = useState<string | null>(
    null,
  );

  // Folder pick — mirrors the Home page's static-scan UX (`SearchBox`
  // + system picker + history). Every active run is anchored to a
  // folder so live samples can be joined back to a prior static scan.
  //
  // The state is split intentionally:
  //   * `folderPath` is the raw input (debounced on each keystroke).
  //   * `resolvedFolder` is the *backend-confirmed* identity for the
  //     current path — `null` while we're resolving, an object once
  //     the backend has computed the fingerprint and the static-scan
  //     presence check returns.
  // The split prevents "Start" from flashing enabled against a stale
  // resolution while the user is still typing.
  const [folderPath, setFolderPath] = useState<string>("");
  const [resolvedFolder, setResolvedFolder] = useState<{
    path: string;
    fingerprint: string;
    hasStaticScan: boolean;
  } | null>(null);
  const [folderResolveError, setFolderResolveError] = useState<string | null>(null);
  // Race protection: each call to `resolveFolder` bumps this counter;
  // its async result only applies if it still matches.
  const folderResolveTokenRef = useRef(0);
  // The browsed-folder history (other folders the user has previously
  // scanned). Shown as a quick-pick list below the input.
  const [scannedFolders, setScannedFolders] = useState<ScannedFolder[]>([]);

  // Per-scan channel + event filter overrides. Pre-filled from the
  // selected profile; editable freely. Empty string falls back to the
  // profile's saved value at command time (`channel: null` to Rust).
  const [channelInput, setChannelInput] = useState<string>("");
  const [eventFilterInput, setEventFilterInput] = useState<string>("");

  // Search query for the active scan's profile views. Plain typing
  // matches any word against name OR file ("type any word, find it"
  // semantics, same as the entries picker on Home). Power users can also
  // reach into the mini-DSL — `name:foo`, `file:/app/`,
  // `!file:/site-packages/` — but it's never required.
  //
  // Pre-filled from `AppConfig.realtime.defaultFrameFilter`. The config
  // field keeps the legacy name (it's persisted to disk and renaming
  // would break existing user configs); the UI just calls the concept
  // "Search" to match the rest of the desktop UI.
  const [searchQuery, setSearchQuery] = useState<string>("");

  // Search mode toggle. Double-clicking the search input flips between:
  //   * "frames"     — search within the currently-loaded scan (default).
  //                    Pre-filled from the active profile's frame filter.
  //   * "past-scans" — search the left rail by file path/name. Useful
  //                    when ~/.drift/event_logs has many old runs and
  //                    the user wants to grep by service / date.
  // The state lives at the page level so the search box and the rail
  // both observe it.
  const [searchMode, setSearchMode] = useState<"frames" | "past-scans">(
    "frames",
  );

  // Whether the Supabase API key is configured in SecretStore. Drives the
  // "Live Listen → Start" button's disabled state.
  const [keyConfigured, setKeyConfigured] = useState<boolean>(false);

  // Whether a connection attempt is in flight. Prevents double-clicks.
  const [starting, setStarting] = useState<boolean>(false);

  const refreshLogs = useCallback(async () => {
    setLogsLoading(true);
    try {
      const ls = await listEventLogs();
      setLogs(ls);
    } catch {
      setLogs([]);
    } finally {
      setLogsLoading(false);
    }
  }, []);

  useEffect(() => {
    refreshLogs();
  }, [refreshLogs]);

  // Hydrate the scanned-folder list + seed `folderPath` from either a
  // `?folder=<fp>` query string or the most-recently-touched folder.
  // The picker UI shows the history as a quick-pick list; the user can
  // also paste a brand-new path the same way they would on Home.
  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const folders = await listScannedFolders();
        if (cancelled) return;
        setScannedFolders(folders);
        const fpFromUrl = new URLSearchParams(location.search).get("folder");
        const fromUrl = fpFromUrl
          ? folders.find((f) => f.fingerprint === fpFromUrl)
          : undefined;
        const seed = fromUrl ?? folders[0];
        if (seed) setFolderPath(seed.path);
      } catch {
        // Backend not ready / scans dir missing — leave folders empty;
        // the UI shows a "no folders yet" hint.
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  // Resolve `folderPath` → { fingerprint, hasStaticScan } via the
  // backend. Runs whenever `folderPath` changes, debounced 250 ms so
  // typing into the SearchBox doesn't spam IPC. The token-ref pattern
  // makes a stale resolution from a previous keystroke a no-op against
  // the latest state, matching the test-id race protection elsewhere.
  useEffect(() => {
    const path = folderPath.trim();
    if (!path) {
      setResolvedFolder(null);
      setFolderResolveError(null);
      return;
    }
    folderResolveTokenRef.current += 1;
    const myToken = folderResolveTokenRef.current;
    const timer = window.setTimeout(async () => {
      try {
        const fp = await registerFolder(path);
        const has = await folderHasStaticScan(fp);
        if (folderResolveTokenRef.current !== myToken) return;
        setResolvedFolder({ path, fingerprint: fp, hasStaticScan: has });
        setFolderResolveError(null);
      } catch (e) {
        if (folderResolveTokenRef.current !== myToken) return;
        setResolvedFolder(null);
        setFolderResolveError(e instanceof Error ? e.message : String(e));
      }
    }, 250);
    return () => window.clearTimeout(timer);
  }, [folderPath]);

  // Hydrate the profile list + select the active profile + presence-
  // check that profile's namespaced SecretStore slot. Done once on
  // mount; the picker can re-trigger via `selectProfile`.
  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const s = await listRealtimeProfiles();
        if (cancelled) return;
        setProfiles(s.profiles);
        // Default selection: the active profile, or the first profile,
        // or none. The Start button is disabled until a profile is
        // selected AND that profile has a key configured.
        const initial =
          s.activeProfileId ?? s.profiles[0]?.id ?? null;
        setSelectedProfileId(initial);
        const initialProfile =
          s.profiles.find((p) => p.id === initial) ?? null;
        if (initialProfile) {
          setChannelInput(initialProfile.channel);
          setEventFilterInput(initialProfile.eventName);
          setSearchQuery(initialProfile.frameFilter);
          try {
            const present = await secretStatus(
              realtimeApiKeyName(initialProfile.id),
            );
            if (!cancelled) setKeyConfigured(present);
          } catch {
            /* badge stays grey */
          }
        } else {
          setKeyConfigured(false);
        }
      } catch {
        // Non-fatal — leave inputs empty; the user can pick a profile
        // once one is created in Settings → Realtime.
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  // Stop any live tail / realtime stream on unmount. The Rust side also
  // drops the registry entry when the cancel token fires; doing it here
  // covers UI-only teardowns (navigation, error path).
  useEffect(() => {
    return () => {
      if (liveIdRef.current) {
        stopLiveEventScan(liveIdRef.current).catch(() => undefined);
        liveIdRef.current = null;
      }
      if (realtimeStreamIdRef.current) {
        stopRealtimeEventStream(realtimeStreamIdRef.current).catch(
          () => undefined,
        );
        realtimeStreamIdRef.current = null;
      }
    };
  }, []);

  // Subscribe to the live aggregator's events. We register unconditionally
  // and filter by liveScanId in the callback — keeps the listener identity
  // stable across mode flips so we never miss the first frame. The same
  // topic carries aggregates for BOTH `live` (file-tail) and
  // `live-realtime` (Supabase WSS) modes; we match by liveScanId.
  useEffect(() => {
    const cleanup: Array<() => void> = [];
    (async () => {
      cleanup.push(
        await onLiveEventAgg((p) => {
          // Wrap the live-aggregate state update in `startTransition` so
          // React treats it as low-priority work — the user's typing in
          // the search box, table sort clicks, and other interactions
          // get to interrupt a 10k-row table re-render instead of
          // jank-blocking behind it.
          startTransition(() => {
            setMode((cur) => {
              if (cur.kind === "live" && cur.liveScanId === p.liveScanId) {
                return { ...cur, report: p.report, lastError: null };
              }
              if (
                cur.kind === "live-realtime" &&
                cur.liveScanId === p.liveScanId
              ) {
                return { ...cur, report: p.report, lastError: null };
              }
              return cur;
            });
          });
        }),
      );
      cleanup.push(
        await onLiveEventErr((p) => {
          setMode((cur) => {
            if (cur.kind === "live" && cur.liveScanId === p.liveScanId) {
              return { ...cur, lastError: p.message };
            }
            if (
              cur.kind === "live-realtime" &&
              cur.liveScanId === p.liveScanId
            ) {
              return { ...cur, lastError: p.message };
            }
            return cur;
          });
        }),
      );
    })();
    return () => {
      cleanup.forEach((fn) => fn());
    };
  }, []);

  // Helper: cancel whatever's currently running (file-tail OR realtime).
  // Called before every mode switch so we never have two sources writing
  // into the chart concurrently. Quiet on errors — double-stop is fine.
  const stopAnyActive = useCallback(async () => {
    if (liveIdRef.current) {
      await stopLiveEventScan(liveIdRef.current).catch(() => undefined);
      liveIdRef.current = null;
    }
    if (realtimeStreamIdRef.current) {
      await stopRealtimeEventStream(realtimeStreamIdRef.current).catch(
        () => undefined,
      );
      realtimeStreamIdRef.current = null;
    }
  }, []);

  const loadStatic = useCallback(
    async (path: string) => {
      // Drop any active source — we only support one active view at a time.
      await stopAnyActive();
      setMode({ kind: "loading", path });
      try {
        const report = await aggregateEventLog(path);
        setMode({ kind: "static", path, report });
      } catch (e) {
        setMode({
          kind: "error",
          message: e instanceof Error ? e.message : String(e),
        });
      }
    },
    [stopAnyActive],
  );

  const startLive = useCallback(async () => {
    // System dialog → start tail → mode flip. Any prior source is
    // cancelled first.
    const path = await selectEventLogFile();
    if (!path) return;
    await stopAnyActive();
    try {
      const id = await startLiveEventScan(path);
      liveIdRef.current = id;
      setMode({
        kind: "live",
        path,
        liveScanId: id,
        report: null,
        lastError: null,
      });
    } catch (e) {
      setMode({
        kind: "error",
        message: e instanceof Error ? e.message : String(e),
      });
    }
  }, [stopAnyActive]);

  /** Start a Supabase Realtime subscription. Reads the JWT from
   *  SecretStore server-side; the channel + event-filter overrides come
   *  from the inline inputs on this page. Empty inputs → server-side
   *  defaults from AppConfig.realtime → publisher defaults. */
  const startRealtime = useCallback(async () => {
    if (!keyConfigured) {
      setMode({
        kind: "error",
        message:
          "Supabase API key isn't configured. Open Settings → Realtime and paste your key.",
      });
      return;
    }
    if (starting) return; // double-click guard

    // Folder resolution can lag the click — the debounced effect runs
    // 250 ms after the last keystroke / browse pick, so a fast user
    // clicks Start before `resolvedFolder` is filled in. Resolve
    // synchronously here so Start always uses the freshest path.
    let folder = resolvedFolder;
    const pendingPath = folderPath.trim();
    if (!folder && pendingPath) {
      try {
        const fp = await registerFolder(pendingPath);
        const has = await folderHasStaticScan(fp);
        folder = { path: pendingPath, fingerprint: fp, hasStaticScan: has };
        setResolvedFolder(folder);
      } catch (e) {
        setMode({
          kind: "error",
          message: `Folder problem: ${e instanceof Error ? e.message : String(e)}`,
        });
        return;
      }
    }
    if (!folder) {
      setMode({
        kind: "error",
        message:
          "Pick a folder first. Type a path or click Browse — same flow as Static Scan on Home.",
      });
      return;
    }
    if (!folder.hasStaticScan) {
      setMode({
        kind: "error",
        message:
          "This folder hasn't been statically scanned yet. Run Static Scan against it first so live samples can be joined to code references.",
      });
      return;
    }
    setStarting(true);
    await stopAnyActive();
    try {
      const handle = await startRealtimeEventStream(
        folder.fingerprint,
        channelInput.trim() || null,
        eventFilterInput.trim() || null,
      );
      realtimeStreamIdRef.current = handle.streamId;
      // The file-tail aggregator that drives `event_log://aggregate` for
      // this stream is auto-registered server-side; cancelling via
      // stopRealtimeEventStream cleans both up.
      setMode({
        kind: "live-realtime",
        streamId: handle.streamId,
        liveScanId: handle.liveScanId,
        logPath: handle.logPath,
        channel: channelInput.trim() || "drift-profiler-events",
        report: null,
        lastError: null,
      });
      // The realtime log shows up under ~/.drift/event_logs as a new
      // file — refresh the rail so the user can find it later.
      refreshLogs();
    } catch (e) {
      setMode({
        kind: "error",
        message: e instanceof Error ? e.message : String(e),
      });
    } finally {
      setStarting(false);
    }
  }, [
    channelInput,
    eventFilterInput,
    folderPath,
    folderResolveError,
    keyConfigured,
    refreshLogs,
    resolvedFolder,
    starting,
    stopAnyActive,
  ]);

  /** Download `events.log` from a running observability-server and load
   *  it as a static report. Same downstream code path as picking a
   *  local file — the Rust side saves the bytes to
   *  `~/.drift/event_logs/downloaded-<stamp>.jsonl` and we then
   *  `aggregateEventLog` that path.
   *
   *  Errors surface in the existing `mode = error` view so we don't
   *  need a new toast surface. */
  const downloadFromUrl = useCallback(async () => {
    // window.prompt is intentionally minimal — anything fancier (modal,
    // history dropdown) is design polish, not a blocker. The default
    // URL covers the Tilt / docker-compose setups documented in
    // drift-observability/.
    const url = window.prompt(
      "Observability-server URL (/events/log):",
      DEFAULT_OBS_URL,
    );
    if (!url) return;
    setMode({ kind: "loading", path: url });
    try {
      const dl = await downloadEventLog(url);
      // Refresh the rail so the new file shows up under "Past scans".
      refreshLogs();
      // Load the freshly-downloaded file as a static report.
      const report = await aggregateEventLog(dl.path);
      setMode({ kind: "static", path: dl.path, report });
    } catch (e) {
      setMode({
        kind: "error",
        message: e instanceof Error ? e.message : String(e),
      });
    }
  }, [refreshLogs]);

  const stopLive = useCallback(async () => {
    // Stops whichever live source is active — file-tail OR realtime.
    // Preserves the last report as a static view so the chart doesn't
    // flash empty after Stop.
    if (liveIdRef.current) {
      await stopLiveEventScan(liveIdRef.current).catch(() => undefined);
      liveIdRef.current = null;
    }
    if (realtimeStreamIdRef.current) {
      await stopRealtimeEventStream(realtimeStreamIdRef.current).catch(
        () => undefined,
      );
      realtimeStreamIdRef.current = null;
    }
    setMode((cur) => {
      if (cur.kind === "live" && cur.report) {
        return { kind: "static", path: cur.path, report: cur.report };
      }
      if (cur.kind === "live-realtime" && cur.report) {
        return { kind: "static", path: cur.logPath, report: cur.report };
      }
      if (cur.kind === "live" || cur.kind === "live-realtime") {
        return { kind: "idle" };
      }
      return cur;
    });
    // Make the saved realtime log discoverable in the rail.
    refreshLogs();
  }, [refreshLogs]);

  const activeReport: EventLogReport | null =
    mode.kind === "static"
      ? mode.report
      : mode.kind === "live"
      ? mode.report
      : mode.kind === "live-realtime"
      ? mode.report
      : null;

  const isLive = mode.kind === "live" || mode.kind === "live-realtime";

  // ---- past-scans rail content -------------------------------------------
  // Computed OUTSIDE the JSX block because the `new Set<string>()` generic
  // inside JSX confuses Babel's TSX parser (it tries to read `<string>` as
  // a JSX tag). Lifting the logic up keeps the JSX clean.
  const railContent = (() => {
    if (logsLoading) {
      return <div className="muted live-scan-rail-empty">Loading…</div>;
    }
    // De-dup against full paths so a re-opened scan doesn't double up.
    const seen = new Set<string>();
    const uniqueLogs = logs.filter((l) => {
      if (seen.has(l.path)) return false;
      seen.add(l.path);
      return true;
    });
    if (uniqueLogs.length === 0) {
      return (
        <div className="muted live-scan-rail-empty">
          No event logs in <code>~/.drift/event_logs/</code>. Drop a{" "}
          <code>events.log</code> there or click <strong>live_scan</strong>{" "}
          to pick one anywhere.
        </div>
      );
    }
    const q = searchQuery.trim().toLowerCase();
    const visibleLogs =
      searchMode === "past-scans" && q
        ? uniqueLogs.filter(
            (l) =>
              l.path.toLowerCase().includes(q) ||
              l.displayName.toLowerCase().includes(q),
          )
        : uniqueLogs;
    if (visibleLogs.length === 0) {
      return (
        <div className="muted live-scan-rail-empty">
          No past scans match "{searchQuery}". Click <strong>Frames</strong>{" "}
          above to search the loaded scan instead.
        </div>
      );
    }
    return (
      <ul className="live-scan-rail-list">
        {visibleLogs.map((l) => {
          const activePath =
            mode.kind === "static" || mode.kind === "live" || mode.kind === "loading"
              ? mode.path
              : mode.kind === "live-realtime"
                ? mode.logPath
                : null;
          const active = activePath === l.path;
          return (
            <li
              key={l.path}
              className={
                active
                  ? "live-scan-rail-row live-scan-rail-row--active"
                  : "live-scan-rail-row"
              }
            >
              <button
                type="button"
                onClick={() => loadStatic(l.path)}
                title={l.path}
              >
                <div className="live-scan-rail-row-name">{l.displayName}</div>
                <div className="muted live-scan-rail-row-meta">
                  {formatBytes(l.sizeBytes)}
                  {l.modifiedIso && <> · {formatRelative(l.modifiedIso)}</>}
                </div>
              </button>
            </li>
          );
        })}
      </ul>
    );
  })();

  return (
    <div className="stage stage--running live-scan-page">
      <div className="live-scan-head">
        <div>
          <h1>Active Scan</h1>
          <div className="muted">
            {sourceMode === "live-listen"
              ? "Stream profiler events from a Supabase Realtime channel — same wire format as your Python services emit."
              : "Aggregate a saved events.log into a snakeviz-style icicle chart."}
          </div>
        </div>
        <div className="live-scan-actions">
          {isLive ? (
            <button type="button" className="ghost-btn" onClick={stopLive}>
              ■ Stop
            </button>
          ) : null}
          <button
            type="button"
            className="ghost-btn"
            onClick={() => navigate("/")}
          >
            ← Home
          </button>
        </div>
      </div>

      {/* Two-mode source selector. Sits above the per-mode controls and
          the chart. Disabled while a scan is starting to avoid mid-flight
          mode flips. */}
      <div className="active-scan-modebar">
        <div
          className="settings-tabs"
          style={{ marginTop: 0, marginBottom: 12 }}
        >
          <button
            type="button"
            className={`settings-tab ${
              sourceMode === "live-listen" ? "is-active" : ""
            }`}
            onClick={() => setSourceMode("live-listen")}
            disabled={starting}
          >
            ● Live Listen
          </button>
          <button
            type="button"
            className={`settings-tab ${
              sourceMode === "load-from-file" ? "is-active" : ""
            }`}
            onClick={() => setSourceMode("load-from-file")}
            disabled={starting}
          >
            📂 Load from File
          </button>
        </div>

        {sourceMode === "live-listen" && (
          <LiveListenPanel
            folderPath={folderPath}
            setFolderPath={setFolderPath}
            resolvedFolder={resolvedFolder}
            folderResolveError={folderResolveError}
            folderHistory={scannedFolders}
            onBrowseFolder={async () => {
              const picked = await selectProjectPath();
              if (!picked) return;
              setFolderPath(picked);
              // Resolve immediately — the debounce-on-typing branch is
              // for keystroke spam; an explicit pick is intentional and
              // should reflect in the status badge without waiting
              // 250 ms.
              folderResolveTokenRef.current += 1;
              const myToken = folderResolveTokenRef.current;
              try {
                const fp = await registerFolder(picked);
                const has = await folderHasStaticScan(fp);
                if (folderResolveTokenRef.current !== myToken) return;
                setResolvedFolder({ path: picked, fingerprint: fp, hasStaticScan: has });
                setFolderResolveError(null);
              } catch (e) {
                if (folderResolveTokenRef.current !== myToken) return;
                setResolvedFolder(null);
                setFolderResolveError(e instanceof Error ? e.message : String(e));
              }
            }}
            onRunStaticScan={() => {
              // Deep-link into Home pre-filled with the current path so
              // the user can immediately kick off "make static magic".
              useRunStore.getState().setProjectPath(folderPath);
              navigate("/");
            }}
            profiles={profiles}
            selectedProfileId={selectedProfileId}
            onSelectProfile={async (id) => {
              setSelectedProfileId(id);
              const p = profiles.find((pp) => pp.id === id) ?? null;
              if (p) {
                setChannelInput(p.channel);
                setEventFilterInput(p.eventName);
                setSearchQuery(p.frameFilter);
                try {
                  const present = await secretStatus(realtimeApiKeyName(p.id));
                  setKeyConfigured(present);
                } catch {
                  setKeyConfigured(false);
                }
              } else {
                setKeyConfigured(false);
              }
            }}
            channelInput={channelInput}
            setChannelInput={setChannelInput}
            eventFilterInput={eventFilterInput}
            setEventFilterInput={setEventFilterInput}
            keyConfigured={keyConfigured}
            starting={starting}
            isStreaming={mode.kind === "live-realtime"}
            currentStatus={
              mode.kind === "live-realtime"
                ? mode.report
                  ? "🟢 connected · streaming"
                  : mode.lastError
                  ? `🔴 ${mode.lastError}`
                  : "⏳ waiting for first event"
                : null
            }
            onStart={startRealtime}
            onStop={stopLive}
            onOpenSettings={() => navigate("/settings?tab=realtime")}
          />
        )}

        {sourceMode === "load-from-file" && (
          <LoadFromFilePanel
            onPickFile={startLive}
            onDownloadFromUrl={downloadFromUrl}
            isStreaming={mode.kind === "live"}
          />
        )}

        {/* Two-mode search bar:
              "frames"     — filter the icicle/table by name/file in
                             the currently-loaded scan (default;
                             mini-DSL supported).
              "past-scans" — filter the left rail of saved scans by
                             file path / display name. Useful when
                             ~/.drift/event_logs has many runs.
            Mode is picked via two visible toggle buttons next to the
            input — affordance is explicit, no hidden gestures. */}
        <div
          className="scan-picker-search-wrap live-scan-search"
          role="search"
          style={{ display: "flex", alignItems: "center", gap: 8 }}
        >
          <div
            role="tablist"
            aria-label="Search target"
            style={{ display: "flex", gap: 4, flexShrink: 0 }}
          >
            <button
              type="button"
              role="tab"
              aria-selected={searchMode === "frames"}
              className={`settings-tab ${searchMode === "frames" ? "is-active" : ""}`}
              style={{ padding: "4px 10px", fontSize: 12 }}
              onClick={() => {
                if (searchMode !== "frames") {
                  setSearchMode("frames");
                  // Clear the query when switching — the two modes use
                  // different input grammars (frame DSL vs path
                  // substring), leaving stale text would surface
                  // confusing matches.
                  setSearchQuery("");
                }
              }}
              title="Search within the currently-loaded scan"
            >
              Frames
            </button>
            <button
              type="button"
              role="tab"
              aria-selected={searchMode === "past-scans"}
              className={`settings-tab ${searchMode === "past-scans" ? "is-active" : ""}`}
              style={{ padding: "4px 10px", fontSize: 12 }}
              onClick={() => {
                if (searchMode !== "past-scans") {
                  setSearchMode("past-scans");
                  setSearchQuery("");
                }
              }}
              title="Search past scans on disk by path / name"
            >
              Past scans
            </button>
          </div>
          <div style={{ position: "relative", flex: 1, display: "flex", alignItems: "center" }}>
            <svg
              className="scan-picker-search-icon"
              viewBox="0 0 24 24"
              aria-hidden="true"
              focusable="false"
            >
              <circle cx="11" cy="11" r="7" fill="none" stroke="currentColor" strokeWidth="2" />
              <path d="M20 20l-3.5-3.5" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" />
            </svg>
            <input
              type="search"
              className="scan-picker-search"
              placeholder={
                searchMode === "frames"
                  ? "Search functions or files in the active scan…"
                  : "Filter past scans by path / name…"
              }
              value={searchQuery}
              onChange={(e) => setSearchQuery(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Escape" && searchQuery) {
                  e.preventDefault();
                  setSearchQuery("");
                }
              }}
              spellCheck={false}
              autoComplete="off"
              autoCorrect="off"
              aria-label={
                searchMode === "frames"
                  ? "Search frames in the active scan"
                  : "Search past scans by path"
              }
              style={{ flex: 1 }}
            />
            {searchQuery && (
              <button
                type="button"
                className="scan-picker-search-clear"
                onClick={() => setSearchQuery("")}
                aria-label="Clear search"
                title="Clear (Esc)"
              >
                ×
              </button>
            )}
          </div>
        </div>
      </div>

      <div className="live-scan-body">
        <aside className="live-scan-rail">
          <div className="live-scan-rail-head">
            <span>
              Past scans
              {searchMode === "past-scans" && searchQuery && (
                <span
                  className="muted"
                  style={{ fontSize: 11, marginLeft: 6 }}
                >
                  filtered: "{searchQuery}"
                </span>
              )}
            </span>
            <button
              type="button"
              className="ghost-btn live-scan-refresh"
              onClick={refreshLogs}
              disabled={logsLoading}
              title="Re-list ~/.drift/event_logs/"
            >
              ↻
            </button>
          </div>
          {railContent}
        </aside>

        <section className="live-scan-main">
          {mode.kind === "idle" && (
            <div className="live-scan-empty">
              <div className="live-scan-empty-title">No scan loaded</div>
              <div className="muted">
                Pick a past scan on the left or click <strong>live_scan</strong>
                {" "}to start a 1-second poll over any{" "}
                <code>events.log</code>.
              </div>
            </div>
          )}
          {mode.kind === "loading" && (
            <div className="live-scan-empty">
              <div className="live-scan-empty-title">Aggregating…</div>
              <div className="muted">{mode.path}</div>
            </div>
          )}
          {mode.kind === "error" && (
            <div className="report-error">{mode.message}</div>
          )}
          {(mode.kind === "static" ||
            mode.kind === "live" ||
            mode.kind === "live-realtime") &&
            activeReport && (
              <ReportView
                report={activeReport}
                live={
                  mode.kind === "live" || mode.kind === "live-realtime"
                }
                liveError={
                  mode.kind === "live"
                    ? mode.lastError
                    : mode.kind === "live-realtime"
                    ? mode.lastError
                    : null
                }
                path={
                  mode.kind === "live-realtime" ? mode.logPath : mode.path
                }
                searchQuery={searchQuery}
              />
            )}
          {mode.kind === "live" && !activeReport && (
            <div className="live-scan-empty">
              <div className="live-scan-empty-title">Waiting for first sample…</div>
              <div className="muted">
                Tailing <code>{mode.path}</code> at ~1Hz.
                {mode.lastError && (
                  <>
                    {" "}
                    <span className="report-error-inline">
                      Last error: {mode.lastError}
                    </span>
                  </>
                )}
              </div>
            </div>
          )}
          {mode.kind === "live-realtime" && !activeReport && (
            <div className="live-scan-empty">
              <div className="live-scan-empty-title">
                Listening on <code>{mode.channel}</code>…
              </div>
              <div className="muted">
                Connected to Supabase Realtime. Waiting for the first
                broadcast — start your Python service to see frames here.
                Events are also saved to <code>{mode.logPath}</code>.
                {mode.lastError && (
                  <>
                    {" "}
                    <span className="report-error-inline">
                      Last status: {mode.lastError}
                    </span>
                  </>
                )}
              </div>
            </div>
          )}
        </section>
      </div>
    </div>
  );
}

// ---------- Live Listen panel (Phase D) ----------
// Sits above the chart when source-mode = "live-listen". The channel
// input is pre-filled from AppConfig.realtime.defaultChannel; users can
// override per scan. The API-key check drives the Start button's
// disabled state — we never reveal the key, just whether it's saved.
function LiveListenPanel({
  folderPath,
  setFolderPath,
  resolvedFolder,
  folderResolveError,
  folderHistory,
  onBrowseFolder,
  onRunStaticScan,
  profiles,
  selectedProfileId,
  onSelectProfile,
  channelInput,
  setChannelInput,
  eventFilterInput,
  setEventFilterInput,
  keyConfigured,
  starting,
  isStreaming,
  currentStatus,
  onStart,
  onStop,
  onOpenSettings,
}: {
  folderPath: string;
  setFolderPath: (path: string) => void;
  resolvedFolder: { path: string; fingerprint: string; hasStaticScan: boolean } | null;
  folderResolveError: string | null;
  folderHistory: ScannedFolder[];
  onBrowseFolder: () => void;
  onRunStaticScan: () => void;
  profiles: RealtimeProfile[];
  selectedProfileId: string | null;
  onSelectProfile: (id: string | null) => void;
  channelInput: string;
  setChannelInput: (v: string) => void;
  eventFilterInput: string;
  setEventFilterInput: (v: string) => void;
  keyConfigured: boolean;
  starting: boolean;
  isStreaming: boolean;
  currentStatus: string | null;
  onStart: () => void;
  onStop: () => void;
  onOpenSettings: () => void;
}) {
  const [showAdvanced, setShowAdvanced] = useState(false);
  // Start requires the same gates the backend enforces:
  //   1. folder resolved (backend confirmed the path exists)
  //   2. that folder has at least one static scan
  //   3. realtime profile picked + key configured
  //   4. channel name set
  // Each failing gate surfaces a precise tooltip on the button so the
  // user knows what to fix.
  const canStart =
    !!resolvedFolder &&
    resolvedFolder.hasStaticScan &&
    !!selectedProfileId &&
    keyConfigured &&
    !!channelInput.trim() &&
    !starting;

  // "Test Connection" debug button — connects, joins the channel, waits
  // for phx_reply, closes. No broadcast emitted. Uses the same unified
  // Tauri command as Settings; here every override is null so the Rust
  // side falls through to the saved URL + the SecretStore JWT.
  //
  // `testStage` is the label the Rust side emits at each step. Showing
  // it makes a slow phase visible instead of a single opaque spinner.
  //
  // `liveTestIdRef` is the race-protection ref: if the user Stops and
  // restarts before the first test's promise resolves, the first
  // promise's late `setTestResult` is gated by this ref so it can't
  // clobber the second test's state. Mirror of the same pattern in
  // Settings → RealtimeTab.
  const [testing, setTesting] = useState(false);
  const [testStage, setTestStage] = useState<string>("Testing…");
  const [activeTestId, setActiveTestId] = useState<string | null>(null);
  const liveTestIdRef = useRef<string | null>(null);
  const [testResult, setTestResult] = useState<{
    ok: boolean;
    message: string;
  } | null>(null);
  const canTest = keyConfigured && !testing && !starting && !isStreaming;
  const runTest = useCallback(async () => {
    const id = crypto.randomUUID();
    liveTestIdRef.current = id;
    setActiveTestId(id);
    setTesting(true);
    setTestStage("Testing…");
    setTestResult(null);
    const unlisten = await onTestRealtimeProgress((p) => {
      if (liveTestIdRef.current === id) setTestStage(p.label);
    });
    try {
      // All overrides null → use saved URL + saved JWT from SecretStore.
      // The channel input on this page is the only user-typed value.
      const result = await testRealtimeConnection(id, {
        channel: channelInput.trim() || null,
      });
      if (liveTestIdRef.current === id) setTestResult(result);
    } catch (e) {
      if (liveTestIdRef.current === id) {
        setTestResult({
          ok: false,
          message: e instanceof Error ? e.message : String(e),
        });
      }
    } finally {
      unlisten();
      if (liveTestIdRef.current === id) {
        liveTestIdRef.current = null;
        setActiveTestId(null);
        setTesting(false);
        setTestStage("Testing…");
      }
    }
  }, [channelInput]);

  /** Cancel an in-flight test. Optimistic local reset so the UI reflects
   *  the click immediately, IPC cancel fires in the background. */
  const stopTest = useCallback(async () => {
    if (!activeTestId) return;
    const idToCancel = activeTestId;
    liveTestIdRef.current = null;
    setActiveTestId(null);
    setTesting(false);
    setTestStage("Testing…");
    setTestResult({ ok: false, message: "Test cancelled." });
    try {
      await cancelRealtimeTest(idToCancel);
    } catch {
      // Best-effort. The Rust 5 s budget will reap the test if cancel fails.
    }
  }, [activeTestId]);

  return (
    <div
      className="active-scan-panel"
      style={{
        display: "flex",
        flexDirection: "column",
        gap: 10,
        padding: "12px 14px",
        borderRadius: 8,
        background: "rgba(255, 255, 255, 0.03)",
        border: "1px solid rgba(255, 255, 255, 0.08)",
      }}
    >
      {/* Folder picker — SAME UX as the Static Scan launcher on Home.
          Every active run is anchored to a folder; the folder must
          already have a static scan so live samples can be joined to
          code references. The SearchBox lets the user type a path or
          browse via the system dialog (`onBrowseFolder`); a debounced
          resolver computes the fingerprint and checks the prereq. */}
      <label style={{ display: "flex", flexDirection: "column" }}>
        <span style={{ fontSize: 12, marginBottom: 4 }}>
          Folder{" "}
          <span className="muted" style={{ fontSize: 11 }}>
            (same browse flow as Static Scan — paste a path or click Browse)
          </span>
        </span>
        <SearchBox
          value={folderPath}
          onChange={setFolderPath}
          onPick={onBrowseFolder}
          onSubmit={() => {
            /* Enter in SearchBox isn't the primary Start trigger here
               — the panel's ▶ Start button below is. The submit
               handler is a no-op so Enter doesn't accidentally fire
               half-resolved state. */
          }}
          disabled={isStreaming || starting}
        />

        {/* Folder status: empty / resolving / error / no-static / ready */}
        {folderPath.trim() === "" ? (
          <span className="muted" style={{ fontSize: 11, marginTop: 6 }}>
            Pick a folder to start. Same flow as the "make static magic" button.
          </span>
        ) : folderResolveError ? (
          <span style={{ fontSize: 11, marginTop: 6, color: "#f87171" }}>
            ✗ {folderResolveError}
          </span>
        ) : !resolvedFolder ? (
          <span className="muted" style={{ fontSize: 11, marginTop: 6 }}>
            Resolving folder…
          </span>
        ) : !resolvedFolder.hasStaticScan ? (
          <div
            style={{
              marginTop: 6,
              padding: "6px 10px",
              borderRadius: 6,
              fontSize: 12,
              background: "rgba(251, 191, 36, 0.08)",
              border: "1px solid rgba(251, 191, 36, 0.25)",
              color: "#fbbf24",
              display: "flex",
              alignItems: "center",
              gap: 10,
              flexWrap: "wrap",
            }}
          >
            <span>
              ⚠ This folder has no static scan yet — live samples can't
              be joined to code without one.
            </span>
            <button
              type="button"
              className="ghost-btn"
              onClick={onRunStaticScan}
              style={{ marginLeft: "auto", fontSize: 12 }}
              title="Open Home pre-filled with this path to run the static scan"
            >
              ▸ Run Static Scan
            </button>
          </div>
        ) : (
          <span style={{ fontSize: 11, marginTop: 6, color: "#34d399" }}>
            ✓ Statically scanned · ready to listen
          </span>
        )}

        {/* History — quick-pick of folders the user has previously
            scanned. Same idea as Home's PreviousScansDropdown but
            inline since this panel already nests inputs. */}
        {folderHistory.length > 0 && (
          <details style={{ marginTop: 8 }}>
            <summary
              className="muted"
              style={{ fontSize: 11, cursor: "pointer" }}
            >
              Previously scanned ({folderHistory.length})
            </summary>
            <ul style={{ listStyle: "none", padding: 0, margin: "6px 0 0" }}>
              {folderHistory.map((f) => (
                <li key={f.fingerprint} style={{ marginBottom: 4 }}>
                  <button
                    type="button"
                    className="ghost-btn"
                    style={{
                      width: "100%",
                      textAlign: "left",
                      fontSize: 12,
                      padding: "4px 8px",
                    }}
                    onClick={() => setFolderPath(f.path)}
                    disabled={isStreaming || starting}
                    title={f.path}
                  >
                    {f.path}
                    {f.staticScanCount === 0 && (
                      <span
                        className="muted"
                        style={{ fontSize: 11, marginLeft: 6 }}
                      >
                        (no static scan yet)
                      </span>
                    )}
                  </button>
                </li>
              ))}
            </ul>
          </details>
        )}
      </label>

      {/* Profile picker — drives which Supabase project this scan
          subscribes to. Empty option shown only when no profiles exist
          yet; the Settings button below opens the management surface. */}
      <label style={{ display: "flex", flexDirection: "column" }}>
        <span style={{ fontSize: 12, marginBottom: 4 }}>Profile</span>
        <select
          value={selectedProfileId ?? ""}
          onChange={(e) => onSelectProfile(e.target.value || null)}
          disabled={isStreaming || starting}
        >
          {profiles.length === 0 && (
            <option value="">— no profiles configured —</option>
          )}
          {profiles.map((p) => (
            <option key={p.id} value={p.id}>
              {p.name} ({p.url})
            </option>
          ))}
        </select>
      </label>

      <div style={{ display: "flex", gap: 12, flexWrap: "wrap" }}>
        <label style={{ display: "flex", flexDirection: "column", flex: 1, minWidth: 200 }}>
          <span style={{ fontSize: 12, marginBottom: 4 }}>
            Channel{" "}
            <span className="muted" style={{ fontSize: 11 }}>
              (overrides profile)
            </span>
          </span>
          <input
            type="text"
            placeholder="drift-profiler-events"
            value={channelInput}
            disabled={isStreaming || starting}
            onChange={(e) => setChannelInput(e.target.value)}
            spellCheck={false}
            autoComplete="off"
          />
        </label>

        <div style={{ display: "flex", flexDirection: "column", gap: 4 }}>
          <span style={{ fontSize: 12 }}>&nbsp;</span>
          <div style={{ display: "flex", gap: 8 }}>
            {!isStreaming && (
              testing ? (
                <button
                  type="button"
                  className="ghost-btn"
                  onClick={stopTest}
                  title="Cancel the in-flight test"
                >
                  ⏹ Stop ({testStage})
                </button>
              ) : (
                <button
                  type="button"
                  className="ghost-btn"
                  onClick={runTest}
                  disabled={!canTest}
                  title={
                    !keyConfigured
                      ? "Set your API key in Settings → Realtime first"
                      : "Connect, join the channel, and disconnect — no events are broadcast"
                  }
                >
                  🔌 Test Connection
                </button>
              )
            )}
            {isStreaming ? (
              <button
                type="button"
                className="ghost-btn"
                onClick={onStop}
                title="Disconnect and save the captured events to ~/.drift/event_logs/"
              >
                ⏹ Stop
              </button>
            ) : (
              <button
                type="button"
                className="primary-btn"
                onClick={onStart}
                disabled={!canStart}
                title={
                  !resolvedFolder
                    ? "Pick a folder above — type a path or click Browse"
                    : !resolvedFolder.hasStaticScan
                    ? "This folder has no static scan yet — click Run Static Scan above"
                    : !selectedProfileId
                    ? "Pick a realtime profile above"
                    : !keyConfigured
                    ? "Set this profile's API key in Settings → Realtime first"
                    : !channelInput.trim()
                    ? "Enter a channel name"
                    : "Subscribe to the channel"
                }
              >
                {starting ? "Connecting…" : "▶ Start"}
              </button>
            )}
          </div>
        </div>
      </div>

      {testResult && (
        <div
          style={{
            fontSize: 12,
            padding: "8px 10px",
            borderRadius: 6,
            background: testResult.ok
              ? "rgba(52, 211, 153, 0.08)"
              : "rgba(248, 113, 113, 0.08)",
            border: `1px solid ${
              testResult.ok
                ? "rgba(52, 211, 153, 0.25)"
                : "rgba(248, 113, 113, 0.25)"
            }`,
            color: testResult.ok ? "#34d399" : "#f87171",
            display: "flex",
            alignItems: "center",
            gap: 8,
          }}
        >
          <span>{testResult.ok ? "✓" : "✗"}</span>
          <span style={{ flex: 1 }}>{testResult.message}</span>
          <button
            type="button"
            className="ghost-btn"
            style={{ fontSize: 11, padding: "2px 8px" }}
            onClick={() => setTestResult(null)}
            title="Dismiss"
          >
            ✕
          </button>
        </div>
      )}

      <button
        type="button"
        className="ghost-btn"
        style={{ alignSelf: "flex-start", fontSize: 12 }}
        onClick={() => setShowAdvanced((v) => !v)}
      >
        {showAdvanced ? "▾" : "▸"} Advanced
      </button>

      {showAdvanced && (
        <label style={{ display: "flex", flexDirection: "column" }}>
          <span style={{ fontSize: 12, marginBottom: 4 }}>
            Inner event filter (<code>payload.event</code>)
          </span>
          <input
            type="text"
            placeholder="profiler-event"
            value={eventFilterInput}
            disabled={isStreaming || starting}
            onChange={(e) => setEventFilterInput(e.target.value)}
            spellCheck={false}
            autoComplete="off"
          />
          <span className="muted" style={{ fontSize: 11, marginTop: 4 }}>
            Drop broadcasts whose inner <code>payload.event</code> field
            doesn't match. Empty = accept all.
          </span>
        </label>
      )}

      {!keyConfigured && (
        <div
          className="muted"
          style={{
            fontSize: 12,
            padding: "8px 10px",
            borderRadius: 6,
            background: "rgba(251, 191, 36, 0.08)",
            border: "1px solid rgba(251, 191, 36, 0.2)",
            color: "#fbbf24",
            display: "flex",
            alignItems: "center",
            gap: 10,
          }}
        >
          <span>⚠ No Supabase API key configured.</span>
          <button
            type="button"
            className="ghost-btn"
            style={{ marginLeft: "auto", fontSize: 12 }}
            onClick={onOpenSettings}
          >
            ⚙ Open Realtime Settings
          </button>
        </div>
      )}

      {currentStatus && (
        <div className="muted" style={{ fontSize: 12 }}>
          {currentStatus}
        </div>
      )}
    </div>
  );
}

// ---------- Load from File panel (Phase D) ----------
// Surfaces the three existing file-load flows as first-class buttons.
// No new functionality — the picker, recents rail, and URL download
// already exist; this is just UI hoisting so the user sees them at the
// top level instead of scattered around the page.
function LoadFromFilePanel({
  onPickFile,
  onDownloadFromUrl,
  isStreaming,
}: {
  onPickFile: () => void;
  onDownloadFromUrl: () => void;
  isStreaming: boolean;
}) {
  return (
    <div
      className="active-scan-panel"
      style={{
        display: "flex",
        gap: 10,
        padding: "12px 14px",
        borderRadius: 8,
        background: "rgba(255, 255, 255, 0.03)",
        border: "1px solid rgba(255, 255, 255, 0.08)",
        flexWrap: "wrap",
        alignItems: "center",
      }}
    >
      <button
        type="button"
        className="primary-btn"
        onClick={onPickFile}
        disabled={isStreaming}
        title="Open a JSONL events.log via the system file picker"
      >
        📂 Pick file…
      </button>
      <button
        type="button"
        className="ghost-btn"
        onClick={onDownloadFromUrl}
        disabled={isStreaming}
        title="Fetch events.log from a running observability-server"
      >
        ⬇ Download from URL
      </button>
      <span className="muted" style={{ fontSize: 12 }}>
        Or pick a saved scan from the rail on the left.
      </span>
    </div>
  );
}

interface ReportViewProps {
  report: EventLogReport;
  live: boolean;
  liveError: string | null;
  path: string;
  /** Raw search string from the page-level input. Empty = no search.
   *  Applied uniformly to every view: the function-stats table filters
   *  rows (matches removed); the icicle chart dims non-matching frames
   *  (matches kept, structure preserved — see IcicleChart docstring for
   *  why we never prune). Parsing into a `FrameFilter` happens here once
   *  so every child view shares the same expression. */
  searchQuery: string;
}

/** Tabs we render inside `ReportView`. Order here is the display order.
 *  Flame is first because it's the canonical entry for visual scanning;
 *  Call Graph is next so the "who calls X?" workflow is one click away;
 *  the table-style views trail. */
type ReportTab = "flame" | "graph" | "tree" | "stats" | "functions";

const REPORT_TAB_ITEMS: readonly TabItem<ReportTab>[] = [
  { key: "flame", label: "Flame Graph" },
  { key: "graph", label: "Call Graph" },
  { key: "tree", label: "Call Tree" },
  { key: "stats", label: "Statistics" },
  { key: "functions", label: "Functions" },
];

function ReportView({ report, live, liveError, path, searchQuery }: ReportViewProps) {
  const [sortKey, setSortKey] = useState<keyof EventLogFunctionStat>(
    "cumulativeUs",
  );
  const [sortDir, setSortDir] = useState<"asc" | "desc">("desc");
  const [selected, setSelected] = useState<EventLogTreeNode | null>(null);
  // Tab selection lives next to the report data so switching tabs is a
  // pure render flip — no re-fetch. Each panel unmounts when its tab is
  // not active; that resets per-panel UI state (flame zoom path, table
  // sort) on round-trip, which matches the viewer's behavior.
  const [tab, setTab] = useState<ReportTab>("flame");

  // Parse the search expression once per input change. Cheap (a couple
  // of string splits) but pulling it out of the per-row hot loop matters
  // when `report.functions` is large.
  const parsedSearch = useMemo(
    () => parseFrameFilter(searchQuery),
    [searchQuery],
  );

  const sorted = useMemo(() => {
    // 1. Apply the search. Empty search → identity copy.
    const filtered = parsedSearch.empty
      ? report.functions.slice()
      : report.functions.filter((f) => matchFrameFilter(parsedSearch, f));
    // 2. Sort the survivors.
    filtered.sort((a, b) => {
      const av = a[sortKey];
      const bv = b[sortKey];
      const cmp =
        typeof av === "number" && typeof bv === "number"
          ? av - bv
          : String(av ?? "").localeCompare(String(bv ?? ""));
      return sortDir === "asc" ? cmp : -cmp;
    });
    return filtered;
  }, [report.functions, parsedSearch, sortKey, sortDir]);

  // Count for the "showing N of M" hint next to the table.
  const totalFunctions = report.functions.length;
  const hiddenBySearch = totalFunctions - sorted.length;

  const toggleSort = (key: keyof EventLogFunctionStat) => {
    if (key === sortKey) {
      setSortDir((d) => (d === "asc" ? "desc" : "asc"));
    } else {
      setSortKey(key);
      setSortDir("desc");
    }
  };

  return (
    <div className="live-scan-report">
      <div className="live-scan-summary">
        <div>
          <span className="live-scan-summary-label">file</span>
          <span className="live-scan-summary-value" title={path}>
            {basename(path)}
          </span>
        </div>
        <div>
          <span className="live-scan-summary-label">duration</span>
          <span className="live-scan-summary-value">
            {formatUs(report.durationUs)}
          </span>
        </div>
        <div>
          <span className="live-scan-summary-label">calls</span>
          <span className="live-scan-summary-value">
            {report.totalCalls.toLocaleString()}
          </span>
        </div>
        <div>
          <span className="live-scan-summary-label">events</span>
          <span className="live-scan-summary-value">
            {report.totalEvents.toLocaleString()}
          </span>
        </div>
        <div>
          <span className="live-scan-summary-label">services</span>
          <span className="live-scan-summary-value">
            {report.services.length === 0 ? "—" : report.services.join(", ")}
          </span>
        </div>
        {live && (
          <div className="live-scan-summary-live">
            <span className="live-pulse" />
            live · re-aggregating ~1Hz
            {liveError && (
              <span className="report-error-inline"> · {liveError}</span>
            )}
          </div>
        )}
      </div>

      <Tabs items={REPORT_TAB_ITEMS} value={tab} onChange={setTab} />

      {tab === "flame" && (
        <div className="live-scan-chart">
          <IcicleChart
            root={report.tree}
            search={parsedSearch}
            onNodeClick={(node) => setSelected(node)}
          />
        </div>
      )}

      {tab === "graph" && (
        <CallGraphPanel
          root={report.tree}
          functions={report.functions}
          search={parsedSearch}
          selected={selected}
          onSelect={setSelected}
        />
      )}

      {tab === "tree" && (
        <CallTreePanel
          root={report.tree}
          search={parsedSearch}
          selected={selected}
          onSelect={setSelected}
        />
      )}

      {tab === "stats" && (
        <StatisticsPanel functions={sorted} report={report} />
      )}

      {tab === "functions" && (
      <div className="live-scan-table-wrap">
        {hiddenBySearch > 0 && (
          <div
            className="muted"
            style={{ fontSize: 12, marginBottom: 6 }}
          >
            Showing {sorted.length} of {totalFunctions} functions
            {" "}({hiddenBySearch} hidden by search)
          </div>
        )}
        <table className="live-scan-table">
          <thead>
            <tr>
              <Th k="qualname" sortKey={sortKey} sortDir={sortDir} onClick={toggleSort}>
                qualname
              </Th>
              <Th k="ncalls" sortKey={sortKey} sortDir={sortDir} onClick={toggleSort} align="right">
                ncalls
              </Th>
              <Th k="totalUs" sortKey={sortKey} sortDir={sortDir} onClick={toggleSort} align="right">
                tottime (self)
              </Th>
              <Th k="cumulativeUs" sortKey={sortKey} sortDir={sortDir} onClick={toggleSort} align="right">
                cumtime
              </Th>
              <Th k="percallUs" sortKey={sortKey} sortDir={sortDir} onClick={toggleSort} align="right">
                percall
              </Th>
              <Th k="errors" sortKey={sortKey} sortDir={sortDir} onClick={toggleSort} align="right">
                errors
              </Th>
              <Th k="cpuAvg" sortKey={sortKey} sortDir={sortDir} onClick={toggleSort} align="right">
                cpu
              </Th>
            </tr>
          </thead>
          <tbody>
            {sorted.length === 0 && (
              <tr>
                <td colSpan={7} className="muted live-scan-table-empty">
                  No paired calls yet — waiting for end events.
                </td>
              </tr>
            )}
            {sorted.map((f) => {
              const highlighted = selected?.name === f.qualname;
              return (
                <tr
                  key={f.qualname}
                  className={highlighted ? "live-scan-row live-scan-row--hi" : "live-scan-row"}
                  title={f.file ? `${f.file}${f.line ? `:${f.line}` : ""}` : ""}
                >
                  <td>{f.qualname}</td>
                  <td className="num">{f.ncalls}</td>
                  <td className="num">{formatUs(f.totalUs)}</td>
                  <td className="num">{formatUs(f.cumulativeUs)}</td>
                  <td className="num">{formatUs(f.percallUs)}</td>
                  <td className="num">{f.errors > 0 ? f.errors : "—"}</td>
                  <td className="num">{f.cpuAvg === null ? "—" : f.cpuAvg.toFixed(2)}</td>
                </tr>
              );
            })}
          </tbody>
        </table>
      </div>
      )}
    </div>
  );
}

interface ThProps {
  k: keyof EventLogFunctionStat;
  sortKey: keyof EventLogFunctionStat;
  sortDir: "asc" | "desc";
  onClick: (k: keyof EventLogFunctionStat) => void;
  align?: "left" | "right";
  children: React.ReactNode;
}
function Th({ k, sortKey, sortDir, onClick, align, children }: ThProps) {
  const active = k === sortKey;
  return (
    <th
      onClick={() => onClick(k)}
      style={{ textAlign: align ?? "left", cursor: "pointer" }}
      className={active ? "live-scan-th live-scan-th--active" : "live-scan-th"}
    >
      {children}
      {active && <span className="live-scan-th-arrow">{sortDir === "asc" ? " ↑" : " ↓"}</span>}
    </th>
  );
}

// ---------- formatters ----------------------------------------------------

function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  if (n < 1024 * 1024 * 1024) return `${(n / 1024 / 1024).toFixed(1)} MB`;
  return `${(n / 1024 / 1024 / 1024).toFixed(2)} GB`;
}

function formatRelative(iso: string): string {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return iso;
  const ms = Date.now() - d.getTime();
  if (ms < 60_000) return "just now";
  if (ms < 3_600_000) return `${Math.floor(ms / 60_000)}m ago`;
  if (ms < 86_400_000) return `${Math.floor(ms / 3_600_000)}h ago`;
  return d.toLocaleDateString(undefined, {
    year: "numeric",
    month: "short",
    day: "2-digit",
  });
}

function formatUs(us: number): string {
  if (us <= 0) return "0";
  if (us < 1000) return `${us.toFixed(0)} μs`;
  if (us < 1_000_000) return `${(us / 1000).toFixed(2)} ms`;
  return `${(us / 1_000_000).toFixed(3)} s`;
}

function basename(p: string): string {
  const i = Math.max(p.lastIndexOf("/"), p.lastIndexOf("\\"));
  return i >= 0 ? p.slice(i + 1) : p;
}
