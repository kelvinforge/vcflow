import { useEffect } from "react"
import { getCurrentWindow } from "@tauri-apps/api/window"
import { useWorkflow } from "@/hooks/useWorkflow"
import { useSetupGate } from "@/hooks/useSetupGate"
import { loadLastRepo } from "@/lib/repo"
import { RepoHeader } from "@/components/RepoHeader"
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
    <main className="mx-auto flex min-h-svh w-full max-w-xl flex-col gap-4 p-6">
      <h1 className="flex items-baseline gap-2 text-lg font-semibold text-foreground">
        vcflow
        <span className="text-xs font-normal text-muted-foreground">v{__APP_VERSION__}</span>
      </h1>

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
    </main>
  )
}

export default App
