import { useEffect, useState } from "react"
import { check, type Update } from "@tauri-apps/plugin-updater"
import { relaunch } from "@tauri-apps/plugin-process"
import { Button } from "@/components/ui"

/**
 * Footer notice for a newer GitHub Release. Checks once on startup; the
 * plugin verifies the minisign signature before installing, so a bad or
 * missing .sig fails here and the app keeps running.
 *
 * `blocked` disables Update while relaunching would strand work (Initial
 * Workflow, branch inspection parked on another branch).
 */
// ponytail: no idle detection for single git commands (busy is a ref in
// useWorkflow); the user clicks Update, so they pick the moment.
export function UpdateNotice({ blocked }: { blocked: boolean }) {
  const [update, setUpdate] = useState<Update | null>(null)
  const [progress, setProgress] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [dismissed, setDismissed] = useState(false)

  useEffect(() => {
    // Offline / dev build / endpoint down: stay silent, nothing to offer.
    check().then(setUpdate, () => {})
  }, [])

  if (!update || dismissed) return null

  const install = async () => {
    setError(null)
    let total = 0
    let done = 0
    try {
      await update.downloadAndInstall((e) => {
        if (e.event === "Started") total = e.data.contentLength ?? 0
        if (e.event === "Progress") {
          done += e.data.chunkLength
          setProgress(total ? `${Math.round((done / total) * 100)}%` : "downloading…")
        }
        if (e.event === "Finished") setProgress("installing…")
      })
      await relaunch()
    } catch (e) {
      setProgress(null)
      setError(`Update failed: ${String(e)}`)
    }
  }

  return (
    <span className="flex items-center gap-2">
      {error ? (
        <span className="text-destructive">{error}</span>
      ) : (
        <span>vcflow {update.version} is available</span>
      )}
      {progress ? (
        <span>{progress}</span>
      ) : (
        <>
          <Button
            variant="primary"
            className="px-2 py-0 text-xs"
            disabled={blocked}
            title={blocked ? "Finish the current workflow step first" : undefined}
            onClick={install}
          >
            {error ? "Retry" : "Update"}
          </Button>
          <Button className="px-2 py-0 text-xs" onClick={() => setDismissed(true)}>
            Later
          </Button>
        </>
      )}
    </span>
  )
}
