import { useState } from "react"
import { CheckCircle2, Circle, Loader2, XCircle } from "lucide-react"
import { resumeWork, type SetupStateDto } from "@/lib/tauri"
import { Badge, Button, Card, ErrorLine } from "@/components/ui"

/**
 * The setup gate's card. Shown whenever `setup.phase !== "ready"`; once the
 * gate is satisfied the app renders `<ExistingWorkflow/>` instead. This card is
 * additive -- it does not touch the existing workflow UI.
 */
export function SetupCard({
  state,
  loading,
  initializing,
  step,
  initError,
  repoPath,
  onInitialize,
  onRefresh,
  onRecovered,
}: {
  state: SetupStateDto | null
  loading: boolean
  initializing: boolean
  step: string | null
  initError: string | null
  repoPath: string
  onInitialize: () => void
  onRefresh: () => void
  onRecovered: () => void
}) {
  if (!state && loading) {
    return (
      <Card className="flex items-center gap-2 text-sm text-muted-foreground">
        <Loader2 size={14} className="animate-spin" /> Checking this directory…
      </Card>
    )
  }
  if (!state) {
    return (
      <Card className="text-sm">
        <ErrorLine error="Could not read this directory." onRetry={onRefresh} />
      </Card>
    )
  }

  return (
    <Card className="flex flex-col gap-3 text-sm">
      {state.phase === "not_a_repo" && (
        <Blocked
          title="Not a Git repository"
          body={
            <>
              This folder isn’t a Git repository. Run <code>git init</code> here (and add a
              GitHub or GitLab <code>origin</code>), then reopen it.
            </>
          }
          onRefresh={onRefresh}
        />
      )}

      {state.phase === "needs_first_commit" && (
        <Blocked
          title="No commits yet"
          body="This repository has no commits yet. Make your first commit, then reopen it."
          onRefresh={onRefresh}
        />
      )}

      {state.phase === "preflight_failed" && (
        <>
          <Header title="This directory isn’t ready" />
          <ul className="flex flex-col gap-1.5">
            {state.checks.map((c) => (
              <li key={c.id} className="flex items-start gap-2">
                <CheckIcon status={c.status} />
                <span className="flex flex-col">
                  <span className="text-foreground">{c.title}</span>
                  {c.status !== "pass" && (
                    <span className="text-xs text-muted-foreground">{c.message}</span>
                  )}
                </span>
              </li>
            ))}
          </ul>
          <Button className="self-start" onClick={onRefresh}>
            Re-check
          </Button>
        </>
      )}

      {state.phase === "needs_initial_workflow" && (
        <>
          <Header title="Set up the workflow for this repository" />
          <p className="text-muted-foreground">
            This repository has no <code>develop</code> branch.{" "}
            {state.dirty
              ? "Your uncommitted changes will be saved, carried onto a new feature/initial branch, and restored."
              : "A develop branch will be created and published; nothing else changes."}
          </p>
          {initializing ? (
            <div className="flex items-center gap-2 text-muted-foreground">
              <Loader2 size={14} className="animate-spin" />
              {step ?? "Working…"}
            </div>
          ) : (
            <Button variant="primary" className="self-start" onClick={onInitialize}>
              Set up workflow
            </Button>
          )}
          {initError && <ErrorLine error={initError} />}
        </>
      )}

      {state.phase === "recover" && (
        <RecoverList entries={state.recover_entries} repoPath={repoPath} onRecovered={onRecovered} />
      )}

      {state.notes.length > 0 && (
        <ul className="text-xs text-muted-foreground">
          {state.notes.map((n) => (
            <li key={n}>· {n}</li>
          ))}
        </ul>
      )}
    </Card>
  )
}

function Header({ title }: { title: string }) {
  return <p className="font-medium text-foreground">{title}</p>
}

function Blocked({
  title,
  body,
  onRefresh,
}: {
  title: string
  body: React.ReactNode
  onRefresh: () => void
}) {
  return (
    <>
      <Header title={title} />
      <p className="text-muted-foreground">{body}</p>
      <Button className="self-start" onClick={onRefresh}>
        Re-check
      </Button>
    </>
  )
}

function CheckIcon({ status }: { status: string }) {
  if (status === "pass") return <CheckCircle2 size={14} className="mt-0.5 text-primary" />
  if (status === "warning") return <Circle size={14} className="mt-0.5 text-muted-foreground" />
  return <XCircle size={14} className="mt-0.5 text-destructive" />
}

function RecoverList({
  entries,
  repoPath,
  onRecovered,
}: {
  entries: SetupStateDto["recover_entries"]
  repoPath: string
  onRecovered: () => void
}) {
  const [busyId, setBusyId] = useState<number | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [conflictFiles, setConflictFiles] = useState<string[] | null>(null)

  const resume = async (id: number) => {
    setBusyId(id)
    setError(null)
    setConflictFiles(null)
    try {
      const outcome = await resumeWork(repoPath, id)
      if (outcome.outcome === "conflict") setConflictFiles(outcome.conflicting_files)
    } catch (e) {
      setError(String(e))
    } finally {
      setBusyId(null)
      onRecovered()
    }
  }

  return (
    <>
      <Header title="Setup was interrupted" />
      <p className="text-muted-foreground">
        Your work was saved but hasn’t been restored yet. Restore it to continue.
      </p>
      {entries.map((e) => (
        <div key={e.id} className="flex items-center justify-between gap-2 rounded border border-border p-2">
          <span className="flex flex-col text-xs">
            <span className="text-foreground">{e.label}</span>
            <span className="text-muted-foreground">
              {e.original_branch} · {e.created_at}
            </span>
          </span>
          <span className="flex items-center gap-2">
            <Badge tone={e.status === "conflict" ? "bad" : "muted"}>{e.status}</Badge>
            {e.status === "saved" && (
              <Button
                variant="primary"
                disabled={busyId === e.id}
                onClick={() => resume(e.id)}
              >
                Restore
              </Button>
            )}
          </span>
        </div>
      ))}
      {conflictFiles && (
        <div className="rounded border border-destructive/40 bg-destructive/5 p-2 text-xs">
          <p className="font-medium text-destructive">
            Restore hit a conflict — your saved work was kept, not discarded.
          </p>
          <p className="text-muted-foreground">Conflict markers are in:</p>
          <ul className="text-muted-foreground">
            {conflictFiles.map((f) => (
              <li key={f}>· {f}</li>
            ))}
          </ul>
        </div>
      )}
      {error && <ErrorLine error={error} />}
    </>
  )
}
