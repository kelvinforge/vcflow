import { useEffect, useState } from "react"
import { FolderOpen, RefreshCw } from "lucide-react"
import { openWorkingDirectory, type RepoStatus } from "@/lib/tauri"
import { loadRecentRepos, rememberRepo } from "@/lib/repo"
import { Badge, Button, Input } from "@/components/ui"
import { TokenButton } from "@/components/TokenButton"

function ago(ts: number | null): string {
  if (!ts) return "never"
  const s = Math.round((Date.now() - ts) / 1000)
  if (s < 5) return "just now"
  if (s < 60) return `${s}s ago`
  return `${Math.round(s / 60)}m ago`
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
  const [input, setInput] = useState(repoPath)
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

  return (
    <div className="flex flex-col gap-3 rounded-lg border border-border bg-card p-4">
      <p className="text-xs font-medium text-foreground">Repository</p>

      <div className="flex gap-2">
        <Input
          className="flex-1"
          value={input}
          onChange={(e) => setInput(e.target.value)}
          onKeyDown={(e) => e.key === "Enter" && open(input)}
          placeholder="/path/to/repo"
        />
        <Button variant="primary" disabled={!input.trim()} onClick={() => open(input)}>
          Open
        </Button>
        <Button onClick={() => openWorkingDirectory(repoPath)} title="Open in file manager">
          <FolderOpen size={14} />
        </Button>
      </div>

      {!status && (
        <p className="text-xs text-muted-foreground">
          Enter a local repository path or choose a recent repository.
        </p>
      )}

      {recents.length > 0 && (
        <label className="flex items-center gap-2 text-xs text-muted-foreground">
          Recent repositories:
          <select
            className="flex-1 rounded border border-border bg-transparent px-2 py-1"
            value=""
            onChange={(e) => e.target.value && open(e.target.value)}
          >
            <option value="">Choose…</option>
            {recents.map((r) => (
              <option key={r} value={r}>
                {r.split("/").filter(Boolean).pop() || r}
              </option>
            ))}
          </select>
        </label>
      )}

      {status && (
        <>
          <p className="text-xs font-medium text-foreground">Status</p>
          <div className="flex flex-wrap items-center gap-2 text-xs">
            <Badge>{status.provider}</Badge>
            <Badge tone="ok">{status.branch}</Badge>
            {status.version && <Badge>v{status.version}</Badge>}
          </div>
          <div className="flex flex-wrap items-center gap-2 text-xs">
            <Badge tone={status.ssh_ok ? "ok" : "bad"}>
              SSH {status.ssh_ok ? "connected" : "unreachable"}
            </Badge>
            <Badge tone={status.gitlab_ok ? "ok" : "warn"}>
              GitLab {status.gitlab_ok ? "authenticated" : "unauthenticated"}
            </Badge>
            <Badge tone={status.role === "Owner" ? "ok" : "muted"}>{status.role}</Badge>
          </div>

          <div className="border-t border-border pt-2">
            <p className="mb-1 text-xs font-medium text-foreground">Connection</p>
            <TokenButton repoPath={repoPath} status={status} onChanged={onRefresh} />
          </div>

          <div className="flex items-center justify-between text-xs text-muted-foreground">
            <span>Refreshed {ago(lastRefreshed)}</span>
            <button
              className="inline-flex items-center gap-1 hover:text-foreground"
              onClick={onRefresh}
              disabled={refreshing}
            >
              <RefreshCw size={12} className={refreshing ? "animate-spin" : ""} /> Refresh
            </button>
          </div>
        </>
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
