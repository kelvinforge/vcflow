import { useEffect, useState } from "react"
import type { WorkflowState } from "@/hooks/useWorkflow"
import { RepoStateStrip } from "@/components/RepoHeader"
import { NextActionCard } from "@/components/NextActionCard"
import { BranchInspector } from "@/components/BranchInspector"
import { StartPanel } from "@/components/StartPanel"
import { MrPanel } from "@/components/MrPanel"
import { ConflictPanel } from "@/components/ConflictPanel"
import { SavedWorkPanel } from "@/components/SavedWorkPanel"
import { WorkPanel } from "@/components/WorkPanel"
import { CommandLogPanel } from "@/components/LogPanel"
import { ErrorLine, Section } from "@/components/ui"

/**
 * The existing normal workflow UI, unchanged. Extracted verbatim from App.tsx
 * so the setup gate can wrap it: rendered only once `setup.phase === "ready"`.
 * Nothing in here was modified for Preflight / Initial Setup.
 */
export function ExistingWorkflow({ wf }: { wf: WorkflowState }) {
  // Backend defers merge-conflict resolution to the working dir; when it is the
  // next action, surface the Owner-only resolution panel open.
  const inConflict = wf.nextAction?.primary === "resolve_mr_conflict"

  // Top slot shows EITHER the next step OR the pick-up-work list, never both.
  // Default: work list when there's nothing to do (idle), the action card
  // otherwise. `pickOverride` lets the user flip it by hand -- picking up work
  // by mistake, or wanting to browse other branches mid-task. Reset whenever
  // the branch or the next step changes, so it re-follows the default.
  const canPickWork = !wf.inspection
  const idle = wf.status?.branch === "develop" || wf.nextAction?.primary == null
  const [pickOverride, setPickOverride] = useState<boolean | null>(null)
  useEffect(() => setPickOverride(null), [wf.status?.branch, wf.nextAction?.primary])
  const showWork = canPickWork && (pickOverride ?? idle)

  // Triggered-command wrapper: pause auto-refresh, run, resume + refresh.
  // Returns the raw backend error string, or null on success.
  const onRun = async (run: () => Promise<unknown>): Promise<string | null> => {
    wf.setBusy(true)
    try {
      await run()
      return null
    } catch (e) {
      return String(e)
    } finally {
      wf.setBusy(false)
      wf.refreshNow()
    }
  }

  return (
    <>
      {wf.error && <ErrorLine error={wf.error} onRetry={wf.refreshNow} />}

      <StartPanel
        repoPath={wf.repoPath}
        currentBranch={wf.status?.branch ?? null}
        onChanged={wf.refreshNow}
        extra={
          <BranchInspector
            currentBranch={wf.status?.branch ?? null}
            productionBranch={wf.status?.production_branch ?? null}
            inspection={wf.inspection}
            onInspect={wf.inspectBranch}
            onReturn={wf.endInspection}
          />
        }
      />

      {showWork ? (
        <Section title="Your work" defaultOpen>
          <WorkPanel repoPath={wf.repoPath} onChanged={wf.refreshNow} reloadSignal={wf.syncTick} />
        </Section>
      ) : (
        <NextActionCard
          repoPath={wf.repoPath}
          nextAction={wf.nextAction}
          loading={wf.status === null}
          onRun={onRun}
        />
      )}

      {canPickWork && (
        <button
          className="self-start text-xs text-muted-foreground hover:text-foreground"
          onClick={() => setPickOverride(!showWork)}
        >
          {showWork ? "← Back to next step" : "Pick up other work"}
        </button>
      )}

      {wf.status && <RepoStateStrip status={wf.status} />}

      <Section title={wf.release ? "Active Release" : "Review Handoff"} defaultOpen>
        <MrPanel
          mr={wf.mr}
          hotfix={wf.hotfix}
          release={wf.release}
          repoPath={wf.repoPath}
          onChanged={wf.refreshNow}
        />
      </Section>

      {/* Only surfaces when the backend actually routes a merge conflict here. */}
      {inConflict && (
        <Section title="Resolve conflicts" defaultOpen>
          <ConflictPanel repoPath={wf.repoPath} onChanged={wf.refreshNow} />
        </Section>
      )}

      <Section title="Saved work (Work Safe)">
        <SavedWorkPanel repoPath={wf.repoPath} onChanged={wf.refreshNow} />
      </Section>

      <Section title="Command log">
        <CommandLogPanel repoPath={wf.repoPath} />
      </Section>
    </>
  )
}
