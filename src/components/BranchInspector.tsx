import { useState } from "react"
import { Eye } from "lucide-react"
import type { InspectionMode } from "@/hooks/useWorkflow"
import type { InspectTarget } from "@/lib/tauri"
import { Button, Card, ErrorLine } from "@/components/ui"

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
    <Card>
      <div className="flex h-full flex-col gap-2">
        <p className="flex items-center gap-1.5 text-sm font-semibold text-foreground">
          <Eye size={15} /> Inspect
        </p>
        <p className="flex-1 text-xs text-muted-foreground">
          Peek at <code>develop</code> or <code>{productionBranch ?? "main"}</code> from any branch.
          Current work is saved first; use <span className="font-medium">Return</span> to come back.
        </p>
        {inspection ? (
          <Button disabled={busy} className="w-full" onClick={() => run(onReturn)}>
            {busy ? "Returning…" : `Return to ${inspection.originalBranch}`}
          </Button>
        ) : (
          <div className="flex flex-col gap-2">
            {targets.filter((t) => t !== currentBranch).map((t) => (
              <Button
                key={t}
                className="w-full"
                disabled={busy || !currentBranch}
                onClick={() => run(() => onInspect(t))}
              >
                {busy ? "…" : t}
              </Button>
            ))}
          </div>
        )}
        {inspection && (
          <p className="text-xs text-muted-foreground">
            Inspecting <span className="font-medium text-foreground">{inspection.target}</span>.
          </p>
        )}
        {error && <ErrorLine error={error} />}
      </div>
    </Card>
  )
}
