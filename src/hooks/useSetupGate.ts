import { useCallback, useEffect, useRef, useState } from "react"
import {
  getSetupState,
  initializeWorkflow,
  onWorkflowInitStep,
  onWorkflowStateChanged,
  type SetupStateDto,
} from "@/lib/tauri"

/**
 * The setup gate that sits IN FRONT of the existing workflow. Owns only setup
 * state -- `useWorkflow` is untouched. When `state.phase === "ready"` the app
 * hands off to the existing workflow UI and this hook does nothing more.
 */
export interface SetupGate {
  state: SetupStateDto | null
  loading: boolean
  initializing: boolean
  /** Live step text while `initializing` ("Saving your work…", …). */
  step: string | null
  /** Verbatim backend error from a failed `initialize`. */
  initError: string | null
  refresh: () => void
  initialize: () => Promise<void>
}

export function useSetupGate(repoPath: string): SetupGate {
  const [state, setState] = useState<SetupStateDto | null>(null)
  const [loading, setLoading] = useState(true)
  const [initializing, setInitializing] = useState(false)
  const [step, setStep] = useState<string | null>(null)
  const [initError, setInitError] = useState<string | null>(null)

  const repoRef = useRef(repoPath)
  const gen = useRef(0)

  const refresh = useCallback(() => {
    const repo = repoRef.current
    const g = ++gen.current
    setLoading(true)
    getSetupState(repo)
      .then((s) => {
        if (g === gen.current && repoRef.current === repo) setState(s)
      })
      .catch(() => {
        if (g === gen.current && repoRef.current === repo) setState(null)
      })
      .finally(() => {
        if (g === gen.current && repoRef.current === repo) setLoading(false)
      })
  }, [])

  // Directory change: drop the previous repo's setup state immediately so it
  // never shows, then refetch for the new one.
  useEffect(() => {
    repoRef.current = repoPath
    setState(null)
    setInitError(null)
    setStep(null)
    refresh()
  }, [repoPath, refresh])

  // A real mutation elsewhere (e.g. resuming saved work) may have changed
  // `develop` / the saved-work table -- re-derive. Not on the 3s/15s timers.
  useEffect(() => {
    const un = onWorkflowStateChanged(() => {
      if (!initializing) refresh()
    })
    return () => {
      void un.then((f) => f())
    }
  }, [refresh, initializing])

  const initialize = useCallback(async () => {
    setInitializing(true)
    setInitError(null)
    setStep(null)
    const un = await onWorkflowInitStep(setStep)
    try {
      await initializeWorkflow(repoRef.current)
    } catch (e) {
      setInitError(String(e))
    } finally {
      un()
      setInitializing(false)
      setStep(null)
      refresh()
    }
  }, [refresh])

  return { state, loading, initializing, step, initError, refresh, initialize }
}
