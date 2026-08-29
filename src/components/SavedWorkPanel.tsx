import { useCallback, useEffect, useState } from "react"
import {
  discardWork,
  listSavedWork,
  onWorkflowStateChanged,
  resumeWork,
  type ResumeOutcome,
  type SavedWorkDto,
} from "@/lib/tauri"
import { Badge, Button, ErrorLine } from "@/components/ui"

const STATUS_TONE = {
  saved: "muted",
  conflict: "bad",
  restored: "ok",
  discarded: "muted",
} as const

export function SavedWorkPanel({
  repoPath,
  onChanged,
}: {
  repoPath: string
  onChanged: () => void
}) {
  const [rows, setRows] = useState<SavedWorkDto[]>([])
  const [error, setError] = useState<string | null>(null)
  const [busyId, setBusyId] = useState<number | null>(null)
  const [conflict, setConflict] = useState<ResumeOutcome | null>(null)

  const load = useCallback(() => {
    listSavedWork(repoPath)
      .then(setRows)
      .catch((e) => setError(String(e)))
  }, [repoPath])

  useEffect(load, [load])

  // Other commands (e.g. creating a branch) auto-save work as a side effect.
  // Re-list on the same signal the workflow hook uses.
  useEffect(() => {
    const unlisten = onWorkflowStateChanged(load)
    return () => {
      void unlisten.then((fn) => fn())
    }
  }, [load])

  const guard = async (id: number, fn: () => Promise<unknown>) => {
    setBusyId(id)
    setError(null)
    try {
      await fn()
    } catch (e) {
      setError(String(e))
    } finally {
      setBusyId(null)
      load()
      onChanged()
    }
  }

  const resume = (id: number) =>
    guard(id, async () => {
      const outcome = await resumeWork(repoPath, id)
      // Structured result -- never parse an error string for this.
      setConflict(outcome.outcome === "conflict" ? outcome : null)
    })

  return (
    <div className="flex flex-col gap-2 text-xs">
      {/* Manual save disabled during testing: entries are meant to be created
          automatically by the Work Safe guard, not hand-managed by the user. */}
      <div className="flex flex-col gap-1">
        <Button className="self-start" disabled title="Disabled during testing">
          Save current work (stash)
        </Button>
        <p className="text-muted-foreground">
          Saved work is created automatically when a workflow step needs a clean tree.
        </p>
      </div>

      {rows.length === 0 && <p className="text-muted-foreground">No saved work.</p>}

      {rows.map((r) => (
        <div key={r.id} className="flex flex-col gap-1 rounded border border-border p-2">
          <div className="flex items-center justify-between gap-2">
            <span className="text-foreground">{r.label || "(no label)"}</span>
            <Badge tone={STATUS_TONE[r.status]}>{r.status}</Badge>
          </div>
          <div className="text-muted-foreground">
            {r.original_branch} @ {r.original_commit.slice(0, 8) || "?"} · {r.created_at}
          </div>
          {r.status === "restored" ? (
            <span className="text-muted-foreground">Restored — re-applied to your working tree.</span>
          ) : (
            <div className="flex gap-2">
              {r.status === "saved" && (
                <button
                  className="text-primary hover:underline disabled:opacity-50"
                  disabled={busyId === r.id}
                  onClick={() => resume(r.id)}
                >
                  Resume
                </button>
              )}
              <button
                className="text-destructive hover:underline disabled:opacity-50"
                disabled={busyId === r.id}
                onClick={() => guard(r.id, () => discardWork(repoPath, r.id))}
              >
                Discard
              </button>
            </div>
          )}
        </div>
      ))}

      {conflict && (
        <div className="flex flex-col gap-1 rounded border border-destructive/40 bg-destructive/5 p-2">
          <p className="font-medium text-destructive">
            Resume hit a conflict — saved work was kept, not discarded.
          </p>
          <p className="text-muted-foreground">Conflict markers are in these files:</p>
          <ul className="text-muted-foreground">
            {conflict.conflicting_files.map((f) => (
              <li key={f}>· {f}</li>
            ))}
          </ul>
          <p className="text-muted-foreground">
            Resolve them in your working directory, then commit. The entry stays listed as
            <span className="text-destructive"> conflict</span> until you discard it.
          </p>
        </div>
      )}

      {error && <ErrorLine error={error} />}
    </div>
  )
}
