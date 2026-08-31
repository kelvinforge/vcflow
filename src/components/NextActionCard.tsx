import { useState } from "react"
import { ArrowRight, Lock, TriangleAlert } from "lucide-react"
import { ACTIONS, type FieldName } from "@/lib/actions"
import type { NextActionDto, RepoStatus, WorkItemKind } from "@/lib/tauri"
import { Button, Card, ErrorLine, Input, Textarea } from "@/components/ui"
import { cn, slugify } from "@/lib/utils"

const FIELD_PLACEHOLDER: Record<FieldName, string> = {
  message: "commit message",
  title: "merge request title",
  kind: "",
  slug: "branch name — any text, we clean it up",
}

export function NextActionCard({
  repoPath,
  nextAction,
  status,
  loading,
  onRun,
}: {
  repoPath: string
  nextAction: NextActionDto | null
  status: RepoStatus | null
  /** true until the first repo status has loaded. */
  loading: boolean
  /** Runs the action's command, then refreshes. Returns error string or null. */
  onRun: (run: () => Promise<unknown>) => Promise<string | null>
}) {
  const [values, setValues] = useState<Partial<Record<FieldName, string>>>({})
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  if (!nextAction) {
    return (
      <Card>
        <p className="text-sm text-muted-foreground">
          {loading
            ? "Loading next action…"
            : "No next action from the backend. Use the panels below."}
        </p>
      </Card>
    )
  }

  // `start_work_item` has a dedicated always-visible entry point ("New work
  // item" in StartPanel). Don't duplicate its form here -- just point to it.
  const pointToStartPanel = nextAction.primary === "start_work_item"
  const def = nextAction.primary && !pointToStartPanel ? ACTIONS[nextAction.primary] : null
  const set = (f: FieldName, v: string) => setValues((prev) => ({ ...prev, [f]: v }))
  const missing = def?.fields.some((f) => !(values[f] ?? (f === "kind" ? "feature" : "")).trim())

  const trigger = async () => {
    if (!def?.run) return
    setBusy(true)
    const err = await onRun(() =>
      def.run!({ repoPath, values: { kind: "feature", ...values } }),
    )
    setBusy(false)
    setError(err)
    if (!err) setValues({})
  }

  // Workflow Guard card: uncommitted changes on a protected branch. `block`
  // (main/master) is louder than `warn` (develop). The theme only ships a
  // destructive colour, so severity is expressed by intensity, not hue.
  const guard = nextAction.primary === "move_to_new_branch"
  const block = status?.branch_guard === "block"
  const guardBorder = block
    ? "border-destructive bg-destructive/10"
    : "border-destructive/40 bg-destructive/5"

  return (
    <Card className={cn(guard ? guardBorder : "border-primary/30")}>
      <div className="flex flex-col gap-3">
        <div className="flex gap-2">
          {guard && (
            <TriangleAlert
              size={16}
              className={cn("mt-0.5 shrink-0", block ? "text-destructive" : "text-destructive/70")}
            />
          )}
          <div className="flex flex-col gap-1">
            <p className="text-base font-semibold text-foreground">{nextAction.title}</p>
            <p className="text-sm text-muted-foreground">{nextAction.description}</p>
            {nextAction.helper && (
              <p className="text-xs text-muted-foreground/80">{nextAction.helper}</p>
            )}
          </div>
        </div>

        {guard && (
          <GuardFlow
            here={status?.branch ?? "this branch"}
            fileCount={status?.dirty_count ?? 0}
            slug={(values.slug ?? "").trim()}
            kind={(values.kind as WorkItemKind) ?? "feature"}
          />
        )}

        {nextAction.primary === null && (
          <p className="text-xs text-muted-foreground">Nothing for you to do right now.</p>
        )}

        {pointToStartPanel && (
          <p className="text-xs text-muted-foreground">Use “New work item” below to start.</p>
        )}

        {def?.kind === "guidance" && (
          <p className="text-xs text-muted-foreground">
            Handle this in your working directory — the backend does not do it for you.
          </p>
        )}

        {def?.kind === "trigger" && (
          <div className="flex flex-col gap-2">
            {def.fields.map((f) =>
              f === "message" ? (
                <Textarea
                  key={f}
                  rows={3}
                  value={values[f] ?? ""}
                  onChange={(e) => set(f, e.target.value)}
                  placeholder={FIELD_PLACEHOLDER[f]}
                />
              ) : f === "kind" ? (
                <select
                  key={f}
                  className="rounded border border-border bg-transparent px-2 py-1 text-sm"
                  value={(values.kind as WorkItemKind) ?? "feature"}
                  onChange={(e) => set("kind", e.target.value)}
                >
                  <option value="feature">feature</option>
                  <option value="bug">bug</option>
                  <option value="chore">chore</option>
                </select>
              ) : (
                <Input
                  key={f}
                  value={values[f] ?? ""}
                  onChange={(e) =>
                    set(f, f === "slug" ? slugify(e.target.value) : e.target.value)
                  }
                  placeholder={FIELD_PLACEHOLDER[f]}
                />
              ),
            )}
            <Button variant="primary" disabled={busy || missing} onClick={trigger}>
              <span className="inline-flex items-center gap-1.5">
                <def.icon size={14} />
                {busy ? "Working…" : def.label}
              </span>
            </Button>
          </div>
        )}

        {error && <ErrorLine error={error} onRetry={def?.run ? trigger : undefined} />}
      </div>
    </Card>
  )
}

/** Responsive "from -> to" flow. Chips wrap under each other on a narrow
 *  window; the arrow rotates to point down when they stack. */
function GuardFlow({
  here,
  fileCount,
  slug,
  kind,
}: {
  here: string
  fileCount: number
  slug: string
  kind: WorkItemKind
}) {
  const target = `${kind}/${slug || "…"}`
  const chip = "rounded border border-border bg-background px-2 py-1"
  return (
    <div className="flex flex-wrap items-center gap-2 text-xs">
      <span className={cn(chip, "inline-flex items-center gap-1")}>
        <Lock size={11} className="text-destructive/70" />
        <span className="font-medium text-foreground">{here}</span>
        <span className="text-muted-foreground">
          locked{fileCount > 0 ? ` · ${fileCount} file${fileCount === 1 ? "" : "s"}` : ""}
        </span>
      </span>
      <ArrowRight size={14} className="shrink-0 text-muted-foreground" />
      <span className={cn(chip, "inline-flex items-center gap-1")}>
        <span className="font-medium text-foreground">{target}</span>
        <span className="text-muted-foreground">changes kept</span>
      </span>
    </div>
  )
}
