import { useCallback, useEffect, useState } from "react"
import {
  getAuditLog,
  getCommandLog,
  type AuditEntryDto,
  type CommandLogDto,
} from "@/lib/tauri"
import { Button, ErrorLine } from "@/components/ui"

// ponytail: read-only log dumps. Load on mount + explicit Reload, no polling —
// the backend appends these, staleness between reloads is harmless.
const LIMIT = 50

/** RFC3339 -> local "HH:MM:SS" (24h). Date dropped — logs are same-session. */
const hms = (ts: string) => new Date(ts).toTimeString().slice(0, 8)
/** ms -> "992ms" / "2.7s". */
const dur = (ms: number) => (ms < 1000 ? `${ms}ms` : `${(ms / 1000).toFixed(1)}s`)

function useLog<T>(load: () => Promise<T[]>) {
  const [rows, setRows] = useState<T[] | null>(null)
  const [error, setError] = useState<string | null>(null)
  const reload = useCallback(() => {
    load()
      .then((r) => {
        setRows(r)
        setError(null)
      })
      .catch((e) => setError(String(e)))
  }, [load])
  useEffect(reload, [reload])
  return { rows, error, reload }
}

function LogShell({
  rows,
  error,
  reload,
  render,
}: {
  rows: unknown[] | null
  error: string | null
  reload: () => void
  render: (row: never, i: number) => string
}) {
  return (
    <div className="flex flex-col gap-2 text-xs">
      <Button className="self-start" onClick={reload}>
        Reload
      </Button>
      {error && <ErrorLine error={error} />}
      {rows?.length === 0 && <p className="text-muted-foreground">No entries.</p>}
      <ul className="flex flex-col gap-1 font-mono text-muted-foreground">
        {rows?.map((r, i) => (
          <li key={i} className="whitespace-pre-wrap break-words">
            {render(r as never, i)}
          </li>
        ))}
      </ul>
    </div>
  )
}

export function CommandLogPanel({ repoPath }: { repoPath: string }) {
  const load = useCallback(() => getCommandLog(repoPath, LIMIT), [repoPath])
  const { rows, error, reload } = useLog<CommandLogDto>(load)
  return (
    <LogShell
      rows={rows}
      error={error}
      reload={reload}
      render={(r: CommandLogDto) =>
        `${hms(r.timestamp)}  ${r.operation}  ${r.outcome}  ${dur(r.duration_ms)}` +
        (r.error ? `  ${r.error}` : "")
      }
    />
  )
}

export function AuditLogPanel() {
  const load = useCallback(() => getAuditLog(LIMIT), [])
  const { rows, error, reload } = useLog<AuditEntryDto>(load)
  return (
    <LogShell
      rows={rows}
      error={error}
      reload={reload}
      render={(r: AuditEntryDto) =>
        `${hms(r.timestamp)}  ${r.user}  ${r.branch ?? "-"}  ${r.action}  ${r.result}` +
        (r.error ? `  ${r.error}` : "")
      }
    />
  )
}
