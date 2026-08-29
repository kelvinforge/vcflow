import { ExternalLink } from "lucide-react"
import { openUrl, type HotfixStatus, type MrStatus } from "@/lib/tauri"
import { Badge } from "@/components/ui"

function MrRow({ label, mr }: { label: string; mr: MrStatus | null }) {
  if (!mr) {
    return (
      <div className="flex justify-between text-xs">
        <span className="text-muted-foreground">{label}</span>
        <span className="text-muted-foreground">not opened</span>
      </div>
    )
  }
  const conflicted = mr.mergeability === "Conflicted"
  return (
    <div className="flex items-center justify-between gap-2 text-xs">
      <span className="text-muted-foreground">{label}</span>
      <span className="flex items-center gap-2">
        <Badge tone={conflicted ? "bad" : "muted"}>{mr.status}</Badge>
        <Badge tone={conflicted ? "bad" : "ok"}>
          {conflicted ? "conflict (Owner resolves)" : mr.mergeability}
        </Badge>
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
export function MrPanel({ mr, hotfix }: { mr: MrStatus | null; hotfix: HotfixStatus | null }) {
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
