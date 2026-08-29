import { useCallback, useEffect, useState } from "react"
import { ArrowRight, Play } from "lucide-react"
import {
  continueWork,
  dropWork,
  listWorkItems,
  type ContinueOutcome,
  type WipItemDto,
  type WorkList,
} from "@/lib/tauri"
import { Badge, Button, ErrorLine, Input } from "@/components/ui"

const EMPTY: WorkList = { current: null, other: [], waiting: [] }

function startedAgo(iso: string): string {
  const then = new Date(iso).getTime()
  if (Number.isNaN(then)) return ""
  const days = Math.floor((Date.now() - then) / 86_400_000)
  if (days <= 0) return "Started today"
  if (days === 1) return "Started 1 day ago"
  return `Started ${days} days ago`
}

/**
 * The user's work-in-progress. Backend returns current / other / waiting
 * already partitioned -- this component only renders and triggers commands.
 * It never runs git, never inspects branch names, never decides lifecycle.
 * `develop` / `master` are not work items and never appear here.
 */
export function WorkPanel({
  repoPath,
  onChanged,
  reloadSignal,
}: {
  repoPath: string
  onChanged: () => void
  /** Bumped by useWorkflow on every full refresh (timer / focus / mutation). */
  reloadSignal: number
}) {
  const [list, setList] = useState<WorkList>(EMPTY)
  const [error, setError] = useState<string | null>(null)
  const [busyId, setBusyId] = useState<number | null>(null)
  const [dropId, setDropId] = useState<number | null>(null)
  const [resumeHint, setResumeHint] = useState<ContinueOutcome | null>(null)

  const load = useCallback(() => {
    listWorkItems(repoPath)
      .then(setList)
      .catch((e) => setError(String(e)))
  }, [repoPath])

  // Mount + every full refresh in useWorkflow (15s timer, window focus,
  // backend mutation event, inspection). The timer tick is what retires a
  // branch whose MR was merged on the web -- no backend event fires for that.
  useEffect(load, [load, reloadSignal])

  const doContinue = async (id: number) => {
    setBusyId(id)
    setError(null)
    setResumeHint(null)
    try {
      const outcome = await continueWork(repoPath, id)
      setResumeHint(outcome)
    } catch (e) {
      setError(String(e))
    } finally {
      setBusyId(null)
      load()
      onChanged()
    }
  }

  const nothing = !list.current && list.other.length === 0 && list.waiting.length === 0

  return (
    <div className="flex flex-col gap-3 text-xs">
      {nothing && <p className="text-muted-foreground">No tracked work in this repo.</p>}

      {list.current && (
        <div className="flex flex-col gap-1">
          <p className="font-medium text-foreground">Current Work</p>
          <Row item={list.current} />
        </div>
      )}

      {list.other.length > 0 && (
        <div className="flex flex-col gap-1">
          <p className="font-medium text-foreground">Other Work</p>
          {list.other.map((it) => (
            <Row
              key={it.id}
              item={it}
              busy={busyId === it.id}
              onContinue={() => doContinue(it.id)}
              onDrop={() => setDropId(it.id)}
            />
          ))}
        </div>
      )}

      {list.waiting.length > 0 && (
        <div className="flex flex-col gap-1">
          <p className="font-medium text-foreground">Waiting Work</p>
          {list.waiting.map((it) => (
            <Row
              key={it.id}
              item={it}
              busy={busyId === it.id}
              onContinue={() => doContinue(it.id)}
              onDrop={() => setDropId(it.id)}
            />
          ))}
        </div>
      )}

      {dropId !== null && (
        <DropConfirm
          item={[list.current, ...list.other, ...list.waiting].find((i) => i?.id === dropId) ?? null}
          repoPath={repoPath}
          onClose={() => setDropId(null)}
          onDropped={(next) => {
            setList(next)
            setDropId(null)
            onChanged()
          }}
        />
      )}

      {resumeHint && (
        <div className="rounded border border-border p-2 text-muted-foreground">
          Now on <span className="text-foreground">{resumeHint.status.branch}</span>.{" "}
          {resumeHint.restore_outcome === "restored" &&
            "Saved work re-applied to your working tree."}
          {resumeHint.restore_outcome === "conflict" && (
            <>
              Saved work re-applied but collided in:{" "}
              <span className="text-destructive">
                {resumeHint.conflicting_files.join(", ")}
              </span>
              . Resolve in your working directory, then commit.
            </>
          )}
          {resumeHint.restore_outcome === "error" &&
            "Couldn't auto-apply saved work — use Resume in the Saved work panel."}
          {resumeHint.restore_outcome === "none" && "No saved work for this branch."}
        </div>
      )}

      {error && <ErrorLine error={error} />}
    </div>
  )
}

function Row({
  item,
  busy,
  onContinue,
  onDrop,
}: {
  item: WipItemDto
  busy?: boolean
  onContinue?: () => void
  onDrop?: () => void
}) {
  return (
    <div className="flex flex-col gap-1 rounded border border-border p-2">
      <div className="flex items-center justify-between gap-2">
        <span className="text-foreground">{item.branch}</span>
        <div className="flex items-center gap-1">
          <Badge tone="muted">{item.work_type}</Badge>
          {item.status === "waiting" && <Badge tone="warn">waiting</Badge>}
          {item.has_saved_work && <Badge tone="ok">saved work</Badge>}
        </div>
      </div>
      <span className="text-muted-foreground">{startedAgo(item.created_at)}</span>
      {(onContinue || onDrop) && (
        <div className="flex gap-3">
          {onContinue && (
            <button
              className="inline-flex items-center gap-1 text-primary hover:underline disabled:opacity-50"
              disabled={busy}
              onClick={onContinue}
            >
              <Play size={12} /> Continue
            </button>
          )}
          {onDrop && (
            <button
              className="text-destructive hover:underline disabled:opacity-50"
              disabled={busy}
              onClick={onDrop}
            >
              Drop
            </button>
          )}
        </div>
      )}
    </div>
  )
}

function DropConfirm({
  item,
  repoPath,
  onClose,
  onDropped,
}: {
  item: WipItemDto | null
  repoPath: string
  onClose: () => void
  onDropped: (next: WorkList) => void
}) {
  const [text, setText] = useState("")
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  if (!item) return null

  const run = async () => {
    setBusy(true)
    setError(null)
    try {
      onDropped(await dropWork(repoPath, item.id, text))
    } catch (e) {
      setError(String(e))
      setBusy(false)
    }
  }

  return (
    <div className="flex flex-col gap-2 rounded border border-destructive/40 bg-destructive/5 p-2">
      <p className="font-medium text-destructive">Drop Work</p>
      <p className="text-muted-foreground">
        This only removes it from your work list. The git branch and any saved work are left
        untouched. Type the branch name to confirm:
      </p>
      <code className="text-foreground">{item.branch}</code>
      <Input
        autoFocus
        value={text}
        onChange={(e) => setText(e.target.value)}
        onKeyDown={(e) => e.key === "Enter" && text === item.branch && run()}
        placeholder={item.branch}
      />
      <div className="flex gap-2">
        <Button disabled={busy} onClick={onClose}>
          Cancel
        </Button>
        <Button
          variant="destructive"
          disabled={busy || text !== item.branch}
          onClick={run}
        >
          <span className="inline-flex items-center gap-1">
            <ArrowRight size={12} /> Drop Work
          </span>
        </Button>
      </div>
      {error && <ErrorLine error={error} />}
    </div>
  )
}
