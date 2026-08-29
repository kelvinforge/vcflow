import { useEffect } from "react"
import { getCurrentWindow } from "@tauri-apps/api/window"
import { useWorkflow } from "@/hooks/useWorkflow"
import { useSetupGate } from "@/hooks/useSetupGate"
import { loadLastRepo } from "@/lib/repo"
import { RepoHeader, RepoHealth } from "@/components/RepoHeader"
import { SetupCard } from "@/components/SetupCard"
import { ExistingWorkflow } from "@/components/ExistingWorkflow"

function App() {
  const wf = useWorkflow(loadLastRepo())
  const setup = useSetupGate(wf.repoPath)
  const ready = setup.state?.phase === "ready"

  // Window titlebar: "vcflow v0.1.0 — feature/foo" (branch appended once known).
  const branch = wf.status?.branch
  useEffect(() => {
    const base = `vcflow v${__APP_VERSION__}`
    void getCurrentWindow()
      .setTitle(branch ? `${base} — ${branch}` : base)
      .catch(() => {})
  }, [branch])

  return (
    <main className="flex min-h-svh w-full flex-col gap-4 p-6 pt-0">
      <header className="sticky top-0 z-10 -mx-6 flex flex-col gap-2 border-b border-border bg-background px-6 py-3">
        <RepoHeader
          key={wf.repoPath}
          repoPath={wf.repoPath}
          status={wf.status}
          lastRefreshed={wf.lastRefreshed}
          refreshing={wf.refreshing}
          // Directory picker is blocked while Initial Workflow is running.
          onOpenRepo={setup.initializing ? () => {} : wf.setRepoPath}
          onRefresh={wf.refreshNow}
        />
      </header>

      {ready ? (
        <ExistingWorkflow wf={wf} />
      ) : (
        <SetupCard
          state={setup.state}
          loading={setup.loading}
          initializing={setup.initializing}
          step={setup.step}
          initError={setup.initError}
          repoPath={wf.repoPath}
          onInitialize={setup.initialize}
          onRefresh={setup.refresh}
          onRecovered={() => {
            setup.refresh()
            wf.refreshNow()
          }}
        />
      )}

      <footer className="sticky bottom-0 -mx-6 mt-auto flex items-center justify-between border-t border-border bg-background px-6 py-2 text-xs text-muted-foreground">
        <span className="flex items-baseline gap-1">
          vcflow
          <span>v{__APP_VERSION__}</span>
        </span>
        <RepoHealth status={wf.status} />
      </footer>
    </main>
  )
}

export default App
