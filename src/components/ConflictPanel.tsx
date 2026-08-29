import { useState } from "react"
import {
  openInExternalTool,
  startConflictResolution,
  verifyAndCommitResolution,
  type ConflictInfo,
} from "@/lib/tauri"
import { Button, ErrorLine, Input } from "@/components/ui"

/**
 * Owner-only conflict resolution flow: start (merge target into HEAD in the
 * real working dir) -> open external tool -> verify + commit + push. The
 * backend is the authority on every step; this panel only sequences the
 * calls and shows their results.
 */
export function ConflictPanel({
  repoPath,
  onChanged,
}: {
  repoPath: string
  onChanged: () => void
}) {
  const [target, setTarget] = useState("develop")
  const [info, setInfo] = useState<ConflictInfo | null>(null)
  const [note, setNote] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)

  const step = async (fn: () => Promise<void>) => {
    setBusy(true)
    setError(null)
    try {
      await fn()
    } catch (e) {
      setError(String(e))
    } finally {
      setBusy(false)
      onChanged()
    }
  }

  return (
    <div className="flex flex-col gap-2 text-xs">
      {!info && (
        <div className="flex gap-2">
          <Input value={target} onChange={(e) => setTarget(e.target.value)} placeholder="target branch" />
          <Button
            variant="destructive"
            disabled={busy || !target.trim()}
            onClick={() =>
              step(async () => {
                setInfo(await startConflictResolution(repoPath, target.trim()))
                setNote("Merged target in. Resolve markers, then verify.")
              })
            }
          >
            Start
          </Button>
        </div>
      )}

      {info && (
        <>
          <p className="text-muted-foreground">
            {info.branch} ← {info.target_branch}. Conflicting files:
          </p>
          <ul className="text-muted-foreground">
            {info.conflicting_files.map((f) => (
              <li key={f}>· {f}</li>
            ))}
          </ul>
          <div className="flex gap-2">
            <Button disabled={busy} onClick={() => step(() => openInExternalTool(repoPath))}>
              Open mergetool / editor
            </Button>
            <Button
              variant="primary"
              disabled={busy}
              onClick={() =>
                step(async () => {
                  await verifyAndCommitResolution(repoPath)
                  setInfo(null)
                  setNote(`Resolved, committed, pushed to ${info.branch}.`)
                })
              }
            >
              Verify &amp; commit + push
            </Button>
          </div>
        </>
      )}

      {note && <p className="text-muted-foreground">{note}</p>}
      {error && <ErrorLine error={error} />}
    </div>
  )
}
