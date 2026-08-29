import { useState } from "react"
import type { InspectionMode } from "@/hooks/useWorkflow"
import type { InspectTarget } from "@/lib/tauri"
import { Button, ErrorLine, Help } from "@/components/ui"

// Auxiliary action, not a workflow surface. Temporarily parks on develop or the
// production branch via Work Safe so client feedback can be checked without a
// manual checkout, then returns to the original branch.

export function BranchInspector({
  currentBranch,
  productionBranch,
  inspection,
  onInspect,
  onReturn,
}: {
  currentBranch: string | null
  productionBranch: string | null
  inspection: InspectionMode | null
  onInspect: (target: InspectTarget) => Promise<string | null>
  onReturn: () => Promise<string | null>
}) {
  const targets = ["develop", productionBranch ?? "main"]
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const run = async (fn: () => Promise<string | null>) => {
    setBusy(true)
    setError(await fn())
    setBusy(false)
  }

  return (
    <div className="flex flex-wrap items-center gap-2">
      {inspection ? (
        <>
          <Button disabled={busy} onClick={() => run(onReturn)}>
            {busy ? "Returning…" : `Return to ${inspection.originalBranch}`}
          </Button>
          <span className="text-xs text-muted-foreground">
            Inspecting <span className="font-medium text-foreground">{inspection.target}</span>
          </span>
        </>
      ) : (
        <span className="inline-flex items-center gap-1">
          <select
            className="rounded border border-border bg-transparent px-2 py-1 text-xs disabled:opacity-50"
            value=""
            disabled={busy || !currentBranch}
            onChange={(e) => e.target.value && run(() => onInspect(e.target.value as InspectTarget))}
          >
            <option value="">{busy ? "Inspecting…" : "Inspect branch…"}</option>
            {targets
              .filter((t) => t !== currentBranch)
              .map((t) => (
                <option key={t} value={t}>
                  {t}
                </option>
              ))}
          </select>
          <Help text={`Inspect: peek at develop or ${productionBranch ?? "main"} from any branch. Current work is saved first; use Return to come back.`} />
        </span>
      )}
      {error && <ErrorLine error={error} />}
    </div>
  )
}
