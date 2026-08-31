import { useEffect, useState, type ReactNode } from "react"
import { FolderOpen, RefreshCw } from "lucide-react"
import { pickRepo, type RepoStatus } from "@/lib/tauri"
import { loadRecentRepos, rememberRepo } from "@/lib/repo"
import { Button } from "@/components/ui"
import { TokenButton } from "@/components/TokenButton"
import { cn } from "@/lib/utils"

function ago(ts: number | null): string {
  if (!ts) return "never"
  const s = Math.round((Date.now() - ts) / 1000)
  if (s < 5) return "just now"
  if (s < 60) return `${s}s ago`
  return `${Math.round(s / 60)}m ago`
}

function health(status: RepoStatus): { dot: string; label: string } {
  if (!status.ssh_ok) return { dot: "bg-destructive", label: "SSH unreachable" }
  if (!status.gitlab_ok)
    return { dot: "bg-destructive/60", label: `${status.provider} not authenticated` }
  return { dot: "bg-primary", label: "connected" }
}

/** Connection health dot + label. Lives in the footer. */
export function RepoHealth({ status }: { status: RepoStatus | null }) {
  if (!status) return null
  const h = health(status)
  return (
    <span className="inline-flex items-center gap-1">
      <span className={cn("h-1.5 w-1.5 rounded-full", h.dot)} />
      {h.label}
    </span>
  )
}

function Dot({ children }: { children: ReactNode }) {
  return <span className="text-border">{children}</span>
}

export function RepoHeader({
  repoPath,
  status,
  lastRefreshed,
  refreshing,
  onOpenRepo,
  onRefresh,
}: {
  repoPath: string
  status: RepoStatus | null
  lastRefreshed: number | null
  refreshing: boolean
  onOpenRepo: (path: string) => void
  onRefresh: () => void
}) {
  // Parent passes key={repoPath}, so this mounts fresh per repo -- no prop sync.
  const [recents, setRecents] = useState<string[]>(loadRecentRepos)
  const [, tick] = useState(0)

  useEffect(() => {
    const id = setInterval(() => tick((n) => n + 1), 10_000)
    return () => clearInterval(id)
  }, [])

  const open = (p: string) => {
    const t = p.trim()
    if (!t) return
    setRecents(rememberRepo(t, recents))
    onOpenRepo(t)
  }

  const browse = async () => {
    const picked = await pickRepo()
    if (picked) open(picked)
  }

  return (
    <div className="flex flex-col gap-2">
      {/* Row 1: current repo + controls */}
      <div className="flex items-center gap-2">
        <FolderOpen size={16} className="shrink-0 text-muted-foreground" />
        <span
          className="min-w-0 flex-1 truncate text-base font-semibold text-foreground"
          title={repoPath}
        >
          {repoPath.split("/").filter(Boolean).pop() || repoPath || "No repository"}
        </span>
        <select
          className="w-40 shrink-0 rounded border border-border bg-transparent px-2 py-1 text-xs"
          value=""
          onChange={(e) => {
            const v = e.target.value
            if (v === "__browse__") void browse()
            else if (v) open(v)
          }}
        >
          <option value="">Open / switch repo…</option>
          <option value="__browse__">Browse folder…</option>
          {recents.map((r) => (
            <option key={r} value={r}>
              {r.split("/").filter(Boolean).pop() || r}
            </option>
          ))}
        </select>
        <Button onClick={onRefresh} disabled={refreshing} title="Refresh status">
          <RefreshCw size={14} className={refreshing ? "animate-spin" : ""} />
        </Button>
      </div>

      {/* Row 2: status (left) + health (right) */}
      {!status ? (
        <p className="text-xs text-muted-foreground">
          Choose a repository folder, or pick a recent one.
        </p>
      ) : (
        <div className="flex items-center justify-between gap-4 text-xs text-muted-foreground">
          <div className="flex min-w-0 items-center gap-2 truncate">
            <span>{status.provider}</span>
            <Dot>·</Dot>
            <span className="font-medium text-foreground">{status.branch}</span>
            {status.version && (
              <>
                <Dot>·</Dot>
                <span>v{status.version}</span>
              </>
            )}
          </div>
          <div className="flex shrink-0 items-center gap-2">
            <TokenButton repoPath={repoPath} status={status} onChanged={onRefresh} />
            <Dot>·</Dot>
            <span title="Last refreshed">{ago(lastRefreshed)}</span>
          </div>
        </div>
      )}
    </div>
  )
}

export function RepoStateStrip({ status }: { status: RepoStatus }) {
  const items: Array<[string, string, boolean]> = [
    ["dirty", status.dirty ? `yes (${status.dirty_count})` : "clean", status.dirty],
    ["ahead/behind", `${status.ahead}/${status.behind}`, status.ahead > 0 || status.behind > 0],
    ["diverged", status.diverged ? "yes" : "no", status.diverged],
    ["in-progress op", status.in_progress_op ?? "none", status.in_progress_op != null],
  ]
  return (
    <div className="grid grid-cols-2 gap-x-4 gap-y-1 rounded-lg border border-border bg-card p-3 text-xs">
      {items.map(([k, v, warn]) => (
        <div key={k} className="flex justify-between gap-2">
          <span className="text-muted-foreground">{k}</span>
          <span className={warn ? "text-destructive" : "text-foreground"}>{v}</span>
        </div>
      ))}
    </div>
  )
}
