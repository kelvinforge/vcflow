import { useState } from "react"
import { ExternalLink } from "lucide-react"
import {
  openUrl,
  syncDevelopAfterRelease,
  type HotfixStatus,
  type MrStatus,
  type ReleaseStatusDto,
} from "@/lib/tauri"
import { Badge, Button, ErrorLine } from "@/components/ui"

function MrRow({ label, mr }: { label: string; mr: MrStatus | null }) {
  if (!mr) {
    return (
      <div className="flex justify-between text-xs">
        <span className="text-muted-foreground">{label}</span>
        <span className="text-muted-foreground">not opened</span>
      </div>
    )
  }
  // Mergeability only has meaning while the MR is open -- a merged/closed MR
  // reports "Unknown" from the provider. Show that badge for open MRs only.
  const open = mr.status === "Open"
  const conflicted = open && mr.mergeability === "Conflicted"
  return (
    <div className="flex items-center justify-between gap-2 text-xs">
      <span className="text-muted-foreground">{label}</span>
      <span className="flex items-center gap-2">
        <Badge tone={conflicted ? "bad" : "muted"}>{mr.status}</Badge>
        {open && (
          <Badge tone={conflicted ? "bad" : "ok"}>
            {conflicted ? "conflict (Owner resolves)" : mr.mergeability}
          </Badge>
        )}
        <button
          className="text-muted-foreground hover:text-foreground"
          onClick={() => openUrl(mr.web_url)}
          title={mr.web_url}
        >
          <ExternalLink size={12} />
        </button>
      </span>
    </div>
  )
}

/**
 * MR / Handoff view. "Handoff Complete" is shown ONLY when the backend
 * returned a real MR (mr != null) -- a successful finish command alone never
 * means the MR exists.
 */
export function MrPanel({
  mr,
  hotfix,
  release,
  repoPath,
  onChanged,
}: {
  mr: MrStatus | null
  hotfix: HotfixStatus | null
  release?: ReleaseStatusDto | null
  repoPath?: string
  onChanged?: () => void
}) {
  if (release) {
    return <ReleaseRows release={release} repoPath={repoPath} onChanged={onChanged} />
  }

  const isHotfix = hotfix != null
  const handoffMr = isHotfix ? hotfix.master : mr
  const handoffComplete = handoffMr != null

  return (
    <div className="flex flex-col gap-2">
      {isHotfix ? (
        <>
          <MrRow label="hotfix → master" mr={hotfix.master} />
          <MrRow label="master → develop (sync)" mr={hotfix.develop} />
        </>
      ) : (
        <MrRow label="branch → develop" mr={mr} />
      )}
      <div className="border-t border-border pt-2">
        {handoffComplete ? (
          <Badge tone="ok">✓ Handoff complete — MR exists</Badge>
        ) : (
          <Badge tone="muted">No MR yet — handoff not established</Badge>
        )}
      </div>
    </div>
  )
}

function ReleaseRows({
  release,
  repoPath,
  onChanged,
}: {
  release: ReleaseStatusDto
  repoPath?: string
  onChanged?: () => void
}) {
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const sync = async () => {
    if (!repoPath) return
    setBusy(true)
    setError(null)
    try {
      await syncDevelopAfterRelease(repoPath, release.candidate_branch, `Release ${release.version}`)
      onChanged?.()
    } catch (e) {
      setError(String(e))
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="flex flex-col gap-2">
      <p className="text-xs font-medium text-foreground">Release v{release.version}</p>
      <MrRow label={`${release.candidate_branch} → production`} mr={release.production} />
      <MrRow label="production → develop (sync)" mr={release.sync} />

      <div className="border-t border-border pt-2">
        {release.complete ? (
          <Badge tone="ok">✓ Release v{release.version} shipped — production live, develop synced</Badge>
        ) : release.sync_required ? (
          <div className="flex flex-col gap-1">
            <Badge tone="warn">Production merged — develop sync owed</Badge>
            {repoPath && (
              <Button variant="primary" disabled={busy} onClick={sync}>
                {busy ? "Working…" : "Sync Develop"}
              </Button>
            )}
          </div>
        ) : release.production ? (
          <Badge tone="muted">Release candidate submitted — awaiting review</Badge>
        ) : (
          <Badge tone="muted">Candidate prepared — not yet submitted</Badge>
        )}
      </div>

      {release.superseded.length > 0 && (
        <div className="border-t border-border pt-2">
          <p className="text-xs text-muted-foreground">Superseded candidates</p>
          {release.superseded.map((c) => (
            <div key={c.branch} className="flex items-center justify-between gap-2 text-xs">
              <span className="text-muted-foreground">{c.branch}</span>
              <span className="flex items-center gap-2">
                <span className="text-muted-foreground">close on the provider</span>
                {c.web_url && (
                  <button
                    className="text-muted-foreground hover:text-foreground"
                    onClick={() => openUrl(c.web_url!)}
                    title={c.web_url}
                  >
                    <ExternalLink size={12} />
                  </button>
                )}
              </span>
            </div>
          ))}
        </div>
      )}

      {error && <ErrorLine error={error} />}
    </div>
  )
}
