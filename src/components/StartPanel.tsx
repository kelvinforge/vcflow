import { useEffect, useState, type ReactNode } from "react"
import { GitBranchPlus, Rocket, Siren } from "lucide-react"
import {
  createHotfix,
  createReleaseCandidate,
  createWorkItem,
  getHotfixVersionPreview,
  getReleasePreview,
  type ReleasePreview,
  type VersionPreview,
  type WorkItemKind,
} from "@/lib/tauri"
import { Button, Card, ErrorLine, Input, Textarea } from "@/components/ui"

type Mode = "choose" | "work" | "hotfix" | "release"

/**
 * Live "still working" note. The create commands are one blocking backend call
 * that does several network steps (fetch develop, fast-forward, create branch,
 * re-probe SSH + GitLab). No per-step events from the backend, so we just show
 * the step list and a running clock so it never looks hung.
 */
function BusyNote({ steps }: { steps: string }) {
  const [secs, setSecs] = useState(0)
  useEffect(() => {
    const id = setInterval(() => setSecs((n) => n + 1), 1000)
    return () => clearInterval(id)
  }, [])
  return (
    <p className="text-xs text-muted-foreground">
      Working… {secs}s — {steps}. First run can take ~30s.
    </p>
  )
}

/**
 * Entry points for new branches. Two cards to choose from; picking one asks
 * for the branch name. Both commands are guarded backend-side (Work Safe +
 * fast-forward + role gate) -- these are UI entry points, not workflow logic.
 */
export function StartPanel({
  repoPath,
  currentBranch,
  onChanged,
  extra,
}: {
  repoPath: string
  /** Used only to gate [Prepare Release] to `develop`. */
  currentBranch?: string | null
  onChanged: () => void
  /** Extra card rendered alongside the entry-point cards (e.g. branch inspector). */
  extra?: ReactNode
}) {
  const [mode, setMode] = useState<Mode>("choose")

  if (mode === "work") {
    return <WorkForm repoPath={repoPath} onChanged={onChanged} onBack={() => setMode("choose")} />
  }
  if (mode === "hotfix") {
    return <HotfixForm repoPath={repoPath} onChanged={onChanged} onBack={() => setMode("choose")} />
  }
  if (mode === "release") {
    return <ReleaseForm repoPath={repoPath} onChanged={onChanged} onBack={() => setMode("choose")} />
  }

  return (
    <div className="flex flex-wrap items-center gap-x-4 gap-y-2">
      <Button variant="primary" onClick={() => setMode("work")}>
        <span className="inline-flex items-center gap-1.5">
          <GitBranchPlus size={14} /> New work
        </span>
      </Button>

      <Button variant="destructive" onClick={() => setMode("hotfix")}>
        <span className="inline-flex items-center gap-1.5">
          <Siren size={14} /> Hotfix
        </span>
      </Button>

      {currentBranch === "develop" && (
        <Button onClick={() => setMode("release")}>
          <span className="inline-flex items-center gap-1.5">
            <Rocket size={14} /> Prepare Release
          </span>
        </Button>
      )}

      {extra && <div className="ml-auto">{extra}</div>}
    </div>
  )
}

function WorkForm({
  repoPath,
  onChanged,
  onBack,
}: {
  repoPath: string
  onChanged: () => void
  onBack: () => void
}) {
  const [kind, setKind] = useState<WorkItemKind>("feature")
  const [slug, setSlug] = useState("")
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const create = async () => {
    setBusy(true)
    setError(null)
    try {
      await createWorkItem(repoPath, kind, slug.trim())
      onChanged()
      onBack()
    } catch (e) {
      setError(String(e))
    } finally {
      setBusy(false)
    }
  }

  return (
    <Card>
      <div className="flex flex-col gap-2">
        <p className="text-sm font-semibold text-foreground">New work item</p>
        <p className="text-xs text-muted-foreground">
          Start a feature, bug, or chore. Branches off the latest <code>develop</code> and merges
          back with an MR. Any uncommitted work is saved first.
        </p>
        <div className="flex gap-2">
          <select
            className="rounded border border-border bg-transparent px-2 py-1 text-sm"
            value={kind}
            onChange={(e) => setKind(e.target.value as WorkItemKind)}
          >
            <option value="feature">feature</option>
            <option value="bug">bug</option>
            <option value="chore">chore</option>
          </select>
          <Input
            className="flex-1"
            autoFocus
            value={slug}
            onChange={(e) => setSlug(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && slug.trim() && create()}
            placeholder="branch name (lowercase-hyphens)"
          />
        </div>
        <p className="text-xs text-muted-foreground">
          Branch: <code>{kind}/{slug.trim() || "…"}</code>
        </p>
        <div className="flex gap-2">
          <Button variant="primary" disabled={busy || !slug.trim()} onClick={create}>
            {busy ? "Working…" : "Create branch"}
          </Button>
          <Button disabled={busy} onClick={onBack}>
            Cancel
          </Button>
        </div>
        {busy && <BusyNote steps="saving any local work, fetching develop, creating the branch" />}
        {error && <ErrorLine error={error} onRetry={create} />}
      </div>
    </Card>
  )
}

function HotfixForm({
  repoPath,
  onChanged,
  onBack,
}: {
  repoPath: string
  onChanged: () => void
  onBack: () => void
}) {
  const [preview, setPreview] = useState<VersionPreview | null>(null)
  const [slug, setSlug] = useState("")
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    getHotfixVersionPreview(repoPath)
      .then(setPreview)
      .catch(() => setPreview(null))
  }, [repoPath])

  const create = async () => {
    setBusy(true)
    setError(null)
    try {
      await createHotfix(repoPath, slug.trim())
      onChanged()
      onBack()
    } catch (e) {
      setError(String(e))
    } finally {
      setBusy(false)
    }
  }

  return (
    <Card>
      <div className="flex flex-col gap-2">
        <p className="text-sm font-semibold text-foreground">New hotfix</p>
        <p className="text-xs text-muted-foreground">
          Urgent fix straight to production. Branches off <code>master</code>, auto-bumps the patch
          version, and opens MRs to <code>master</code> and back to <code>develop</code>.
        </p>
        {preview && (
          <p className="text-xs text-muted-foreground">
            Version bump: <span className="text-foreground">v{preview.current_version}</span> →{" "}
            <span className="text-primary">v{preview.next_version}</span>
          </p>
        )}
        <Input
          autoFocus
          value={slug}
          onChange={(e) => setSlug(e.target.value)}
          onKeyDown={(e) => e.key === "Enter" && slug.trim() && create()}
          placeholder="branch name (lowercase-hyphens)"
        />
        <p className="text-xs text-muted-foreground">
          Branch: <code>hotfix/{slug.trim() || "…"}</code>
        </p>
        <div className="flex gap-2">
          <Button variant="destructive" disabled={busy || !slug.trim()} onClick={create}>
            {busy ? "Working…" : "Create hotfix"}
          </Button>
          <Button disabled={busy} onClick={onBack}>
            Cancel
          </Button>
        </div>
        {busy && <BusyNote steps="fetching master, bumping the version, creating the branch, opening MRs" />}
        {error && <ErrorLine error={error} onRetry={create} />}
      </div>
    </Card>
  )
}

function ReleaseForm({
  repoPath,
  onChanged,
  onBack,
}: {
  repoPath: string
  onChanged: () => void
  onBack: () => void
}) {
  const [preview, setPreview] = useState<ReleasePreview | null>(null)
  const [version, setVersion] = useState("")
  const [changelog, setChangelog] = useState("")
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  // Set once the backend asks to confirm supersede; the next submit passes true.
  const [supersedePrompt, setSupersedePrompt] = useState<string | null>(null)

  useEffect(() => {
    getReleasePreview(repoPath)
      .then((p) => {
        setPreview(p)
        setVersion(p.suggested_version)
        setChangelog(p.changelog_seed.join("\n"))
      })
      .catch((e) => setError(String(e)))
  }, [repoPath])

  const submit = async (supersedeConfirmed: boolean) => {
    setBusy(true)
    setError(null)
    try {
      await createReleaseCandidate(repoPath, version.trim(), changelog, supersedeConfirmed)
      onChanged()
      onBack()
    } catch (e) {
      const msg = String(e)
      if (msg.includes("SUPERSEDE_REQUIRED")) {
        setSupersedePrompt(msg.replace(/^.*SUPERSEDE_REQUIRED:\s*/, ""))
      } else {
        setError(msg)
      }
    } finally {
      setBusy(false)
    }
  }

  const pending = preview?.pending_candidates ?? []

  return (
    <Card>
      <div className="flex flex-col gap-2">
        <p className="text-sm font-semibold text-foreground">Prepare release candidate</p>
        <p className="text-xs text-muted-foreground">
          Snapshots <code>develop</code> onto a short-lived <code>release/x.y.z</code> branch with
          only <code>VERSION</code> + <code>CHANGELOG.md</code>. Review the diff, then Submit opens
          the MR to production. It does not block develop.
        </p>

        {preview && (
          <div className="text-xs text-muted-foreground">
            <p>
              Current production: <span className="text-foreground">v{preview.current_version}</span>
            </p>
            <p>
              Commits to release: <span className="text-foreground">{preview.commit_count}</span> ·
              impact <span className="text-foreground">{preview.impact}</span>
            </p>
          </div>
        )}

        {pending.length > 0 && (
          <p className="text-xs text-amber-600 dark:text-amber-500">
            ⚠ {pending.length} candidate(s) already in flight:{" "}
            {pending.map((c) => `${c.branch}${c.merged ? " (merged!)" : ""}`).join(", ")}
          </p>
        )}

        <label className="text-xs text-muted-foreground">
          Version (SemVer, must exceed current)
          <Input
            className="mt-1"
            value={version}
            onChange={(e) => setVersion(e.target.value)}
            placeholder="1.4.0"
          />
        </label>

        <label className="text-xs text-muted-foreground">
          CHANGELOG section
          <Textarea
            className="mt-1"
            rows={8}
            value={changelog}
            onChange={(e) => setChangelog(e.target.value)}
          />
        </label>

        {supersedePrompt ? (
          <div className="flex flex-col gap-2 rounded border border-amber-500/40 bg-amber-500/5 p-2">
            <p className="text-xs text-foreground">{supersedePrompt}</p>
            <div className="flex gap-2">
              <Button variant="destructive" disabled={busy} onClick={() => submit(true)}>
                {busy ? "Working…" : "Supersede & prepare"}
              </Button>
              <Button disabled={busy} onClick={() => setSupersedePrompt(null)}>
                Cancel
              </Button>
            </div>
          </div>
        ) : (
          <div className="flex gap-2">
            <Button
              variant="primary"
              disabled={busy || !version.trim() || !preview}
              onClick={() => submit(false)}
            >
              {busy ? "Working…" : "Prepare release candidate"}
            </Button>
            <Button disabled={busy} onClick={onBack}>
              Cancel
            </Button>
          </div>
        )}

        {busy && <BusyNote steps="fetching develop, creating the release branch, writing VERSION + CHANGELOG" />}
        {error && (
          <>
            <ErrorLine error={error} />
            {error.includes("SYNC_REQUIRED") && (
              <p className="text-xs text-muted-foreground">
                A prior candidate already merged to production. Use “Sync Develop” in the Active
                Release panel below, then prepare this release again.
              </p>
            )}
          </>
        )}
      </div>
    </Card>
  )
}
