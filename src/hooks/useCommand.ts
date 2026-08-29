import { useCallback, useState } from "react"

/**
 * Wraps a single backend command call with transient UI state. `error` holds
 * the raw backend string -- never parse it. On success, callers typically
 * call the workflow refresh.
 */
export function useCommand<Args extends unknown[], T>(fn: (...args: Args) => Promise<T>) {
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const run = useCallback(
    async (...args: Args): Promise<T | undefined> => {
      setBusy(true)
      setError(null)
      try {
        return await fn(...args)
      } catch (e) {
        setError(String(e))
        return undefined
      } finally {
        setBusy(false)
      }
    },
    [fn],
  )

  return { run, busy, error, reset: () => setError(null) }
}
