import { useCallback, useEffect, useRef, useState } from "react"
import {
  endBranchInspection,
  getHotfixStatus,
  getMrStatus,
  getNextAction,
  getRepoStatus,
  inspectBranch,
  onWorkflowStateChanged,
  refreshRepoStatus,
  type HotfixStatus,
  type InspectTarget,
  type MrStatus,
  type NextActionDto,
  type RepoStatus,
} from "@/lib/tauri"

/** `git fetch origin` + recompute. Network-bound. */
const FETCH_INTERVAL_MS = 15_000
/** Local-only status read (dirty/branch). No network. Keeps the UI responsive
 * to file saves and terminal checkouts. */
const LOCAL_POLL_MS = 3_000

export interface WorkflowState {
  repoPath: string
  setRepoPath: (p: string) => void
  status: RepoStatus | null
  nextAction: NextActionDto | null
  mr: MrStatus | null
  hotfix: HotfixStatus | null
  lastRefreshed: number | null
  refreshing: boolean
  error: string | null
  /** Increments on every full refresh. Panels that own their data watch this. */
  syncTick: number
  /** Fetch + re-read the workflow snapshot now. Call after any triggered command. */
  refreshNow: () => void
  /** Pause auto-refresh while a mutating command is in flight. */
  setBusy: (busy: boolean) => void
  /** Non-null while temporarily parked on develop/master for inspection. */
  inspection: InspectionMode | null
  /** Enter branch inspection: Work Safe guard -> checkout target -> refresh. */
  inspectBranch: (target: InspectTarget) => Promise<string | null>
  /** Leave inspection: checkout original branch -> Work Safe restore -> refresh. */
  endInspection: () => Promise<string | null>
}

export interface InspectionMode {
  target: InspectTarget
  originalBranch: string
  savedWorkId: number | null
}

/**
 * Single owner of workflow state, split into two flows:
 *
 *  - readLocal (3s timer): local-only status read, no network. Keeps dirty/branch
 *    responsive to file saves and terminal checkouts.
 *  - syncRepo (15s timer): `git fetch origin` + rebuild RepoStatus, for
 *    ahead/behind/divergence. Skipped-over by readLocal while in flight.
 *  - refreshSnapshot (mount + real `workflow:state:changed` + refreshNow):
 *    re-reads the backend workflow contract -- next action, MR, hotfix.
 *
 * A generation counter drops stale snapshot results so a slow older read can't
 * overwrite a newer one. Backend `next_action` stays the sole authority; this
 * hook only decides *when* to ask and *which answer to keep*.
 */
export function useWorkflow(initialRepoPath: string): WorkflowState {
  const [repoPath, setRepoPathState] = useState(initialRepoPath)
  const [status, setStatus] = useState<RepoStatus | null>(null)
  const [nextAction, setNextAction] = useState<NextActionDto | null>(null)
  const [mr, setMr] = useState<MrStatus | null>(null)
  const [hotfix, setHotfix] = useState<HotfixStatus | null>(null)
  const [lastRefreshed, setLastRefreshed] = useState<number | null>(null)
  const [refreshing, setRefreshing] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [inspection, setInspection] = useState<InspectionMode | null>(null)
  // Bumped after every full refresh (timer, focus, mutation, inspection).
  // Panels that own their own data (WorkPanel) reload when it changes, so they
  // track MR-merge / branch-delete facts that arrive with no backend event.
  const [syncTick, setSyncTick] = useState(0)
  const bumpTick = useCallback(() => setSyncTick((n) => n + 1), [])

  const busyRef = useRef(false)
  // While set, the 15s fetch loop and mutation-driven snapshot re-reads are
  // suspended so nothing races the manual checkout that inspection just did.
  const inspectingRef = useRef(false)
  const repoRef = useRef(repoPath)
  const statusRef = useRef<RepoStatus | null>(null)
  const snapshotGen = useRef(0)
  // Wall-clock of the last successful snapshot read. Lets the focus-driven
  // catch-up skip itself when a fetch just ran, so it can't stack on the timer.
  const lastRefreshedRef = useRef<number | null>(null)
  // True while a network fetch is in flight: the local poll skips its write so
  // it can't clobber fresher ahead/behind numbers with a stale local view.
  const fetchingRef = useRef(false)

  // Re-read the workflow contract. Never runs on the plain fetch tick.
  const refreshSnapshot = useCallback(async () => {
    const repo = repoRef.current
    const gen = ++snapshotGen.current
    try {
      // Branch-prefix check only skips a pointless call -- it is not a workflow
      // decision (that is get_next_action).
      const isHotfix = statusRef.current?.branch.startsWith("hotfix/") ?? false
      const [na, mrs, hfs] = await Promise.all([
        getNextAction(repo).catch((): NextActionDto | null => null),
        getMrStatus(repo).catch((): MrStatus | null => null),
        isHotfix
          ? getHotfixStatus(repo).catch((): HotfixStatus | null => null)
          : Promise.resolve<HotfixStatus | null>(null),
      ])
      // Drop stale results: a newer snapshot read already started.
      if (gen !== snapshotGen.current || repoRef.current !== repo) return
      setNextAction(na)
      setMr(mrs)
      setHotfix(hfs)
      lastRefreshedRef.current = Date.now()
      setLastRefreshed(lastRefreshedRef.current)
      bumpTick()
    } catch (e) {
      if (gen === snapshotGen.current && repoRef.current === repo) setError(String(e))
    }
  }, [bumpTick])

  // Store a freshly read status. An external change (file save, terminal
  // commit/checkout) that moves dirty/branch also triggers a snapshot re-read,
  // since neither poll re-reads the workflow contract on its own.
  const applyStatus = useCallback(
    (repo: string, s: RepoStatus) => {
      if (repoRef.current !== repo) return
      const prev = statusRef.current
      statusRef.current = s
      setStatus(s)
      setError(null)
      if (prev && (prev.dirty !== s.dirty || prev.branch !== s.branch)) {
        void refreshSnapshot()
      }
    },
    [refreshSnapshot],
  )

  // Cheap local read (no network): keeps dirty/branch responsive between fetches.
  const readLocal = useCallback(async () => {
    if (fetchingRef.current) return
    const repo = repoRef.current
    try {
      const s = await getRepoStatus(repo)
      if (!fetchingRef.current) applyStatus(repo, s)
    } catch (e) {
      if (repoRef.current === repo) setError(String(e))
    }
  }, [applyStatus])

  // 15s timer path: fetch origin, rebuild RepoStatus. Nothing else.
  const syncRepo = useCallback(async () => {
    const repo = repoRef.current
    setRefreshing(true)
    fetchingRef.current = true
    try {
      const s = await refreshRepoStatus(repo)
      applyStatus(repo, s)
      // Fallback path: nudge self-owned panels (WorkPanel) so the 15s loop
      // still retires merged/deleted branches even with no backend event.
      bumpTick()
    } catch (e) {
      if (repoRef.current === repo) setError(String(e))
    } finally {
      fetchingRef.current = false
      if (repoRef.current === repo) setRefreshing(false)
    }
  }, [applyStatus, bumpTick])

  const refreshNow = useCallback(() => {
    void syncRepo().then(refreshSnapshot)
  }, [syncRepo, refreshSnapshot])

  // Enter inspection. setBusy pauses the timers for the checkout; inspectingRef
  // keeps the fetch loop paused for the whole inspection session. On any error
  // the backend has already left the original branch + tree untouched.
  const enterInspection = useCallback(
    async (target: InspectTarget): Promise<string | null> => {
      busyRef.current = true
      try {
        const out = await inspectBranch(repoRef.current, target)
        inspectingRef.current = true
        setInspection({
          target,
          originalBranch: out.original_branch,
          savedWorkId: out.saved_work_id,
        })
        applyStatus(repoRef.current, out.status)
        await refreshSnapshot()
        // Parked on develop/master now -- fetch so ahead/behind reflects origin.
        // syncRepo only fetches + re-reads, never checks out, so it can't
        // disturb the inspection checkout.
        void syncRepo()
        return null
      } catch (e) {
        return String(e)
      } finally {
        busyRef.current = false
      }
    },
    [applyStatus, refreshSnapshot, syncRepo],
  )

  const endInspection = useCallback(async (): Promise<string | null> => {
    const mode = inspection
    if (!mode) return null
    busyRef.current = true
    try {
      const out = await endBranchInspection(
        repoRef.current,
        mode.originalBranch,
        mode.savedWorkId,
      )
      inspectingRef.current = false
      setInspection(null)
      applyStatus(repoRef.current, out.status)
      await refreshSnapshot()
      // Back on the work branch -- fetch so ahead/behind is current again.
      void syncRepo()
      return out.outcome === "conflict"
        ? `Back on ${mode.originalBranch}. Saved work re-apply hit conflicts in: ${out.conflicting_files.join(", ")}`
        : null
    } catch (e) {
      return String(e)
    } finally {
      busyRef.current = false
    }
  }, [inspection, applyStatus, refreshSnapshot, syncRepo])

  const setRepoPath = useCallback((p: string) => {
    const trimmed = p.trim()
    if (!trimmed) return
    setStatus(null)
    setNextAction(null)
    setMr(null)
    setHotfix(null)
    setError(null)
    setLastRefreshed(null)
    setRepoPathState(trimmed)
  }, [])

  // On repo change: reset, do one full refresh, then fetch on an interval.
  useEffect(() => {
    repoRef.current = repoPath
    statusRef.current = null
    void syncRepo().then(refreshSnapshot)
    const local = setInterval(() => {
      if (!busyRef.current) void readLocal()
    }, LOCAL_POLL_MS)
    const fetch = setInterval(() => {
      if (!busyRef.current && !inspectingRef.current) void syncRepo()
    }, FETCH_INTERVAL_MS)
    return () => {
      clearInterval(local)
      clearInterval(fetch)
    }
  }, [repoPath, syncRepo, refreshSnapshot, readLocal])

  // App regained focus -> the user may have merged an MR or deleted a branch on
  // GitLab while away. Do one immediate fetch + rebuild so Review Handoff / Your
  // Work catch up now instead of on the next 15s tick. Event-driven, not a poll:
  // the 15s loop stays untouched as the fallback. Skipped while busy/inspecting,
  // and when a fetch already ran in the last few seconds so it can't stack on
  // the timer.
  useEffect(() => {
    const onFocus = () => {
      if (busyRef.current || inspectingRef.current) return
      const last = lastRefreshedRef.current
      if (last && Date.now() - last < 3_000) return
      void syncRepo().then(refreshSnapshot)
    }
    window.addEventListener("focus", onFocus)
    return () => window.removeEventListener("focus", onFocus)
  }, [syncRepo, refreshSnapshot])

  // Real mutation happened -> re-read the snapshot (no fetch needed).
  useEffect(() => {
    const unlisten = onWorkflowStateChanged(() => {
      if (!busyRef.current && !inspectingRef.current) void refreshSnapshot()
    })
    return () => {
      void unlisten.then((fn) => fn())
    }
  }, [refreshSnapshot])

  return {
    repoPath,
    setRepoPath,
    status,
    nextAction,
    mr,
    hotfix,
    lastRefreshed,
    refreshing,
    error,
    syncTick,
    refreshNow,
    setBusy: (b: boolean) => {
      busyRef.current = b
    },
    inspection,
    inspectBranch: enterInspection,
    endInspection,
  }
}
