import { useState } from "react"
import { Check, KeyRound } from "lucide-react"
import {
  deleteToken,
  reValidateToken,
  saveToken,
  type RepoStatus,
  type TokenValidation,
} from "@/lib/tauri"
import { Button, ErrorLine, Input } from "@/components/ui"

const HOST_KEY = "gwe.gitlabHost"

type View = "collapsed" | "menu" | "form"

/** Bare host from a remote URL (git@host:.., https://host/.., ssh://git@host/..). */
function hostFromRemote(url?: string | null): string {
  return url?.match(/^(?:git@|https?:\/\/|ssh:\/\/git@)([^/:]+)/)?.[1] ?? ""
}

/**
 * Token status + management, shown in the Connection card. Provider-neutral:
 * the button state is driven by RepoStatus.gitlab_ok -- whether the app can
 * actually talk to the provider API (GitLab or GitHub), not merely whether a
 * token string is stored. The token itself never reaches this component
 * beyond the transient input below.
 */
export function TokenButton({
  repoPath,
  status,
  onChanged,
}: {
  repoPath: string
  status: RepoStatus | null
  onChanged: () => void
}) {
  const ok = status?.gitlab_ok ?? false
  const [view, setView] = useState<View>("collapsed")
  const [host, setHost] = useState(
    () => localStorage.getItem(HOST_KEY) || hostFromRemote(status?.remote_url),
  )
  const [token, setToken] = useState("")
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [msg, setMsg] = useState<string | null>(null)
  const [validation, setValidation] = useState<TokenValidation | null>(null)

  const wrap = async (fn: () => Promise<void>) => {
    setBusy(true)
    setError(null)
    try {
      await fn()
    } catch (e) {
      setError(String(e))
    } finally {
      setBusy(false)
    }
  }

  const save = () =>
    wrap(async () => {
      await saveToken(repoPath, host.trim(), token.trim())
      setToken("")
      setMsg("Saved to OS keychain.")
      setView("collapsed")
      onChanged()
    })

  const validate = () =>
    wrap(async () => {
      setValidation(await reValidateToken(repoPath))
      setMsg(null)
    })

  const remove = () =>
    wrap(async () => {
      await deleteToken(repoPath, host.trim())
      setValidation(null)
      setMsg("Token deleted.")
      setView("collapsed")
      onChanged()
    })

  return (
    <div className="flex flex-col gap-2 text-xs">
      <div className="flex items-center gap-2">
        {ok ? (
          <button
            className="inline-flex items-center gap-1 rounded bg-primary/15 px-2 py-1 font-medium text-primary"
            onClick={() => setView(view === "collapsed" ? "menu" : "collapsed")}
          >
            <Check size={12} /> Token
          </button>
        ) : (
          <button
            className="inline-flex items-center gap-1 rounded bg-destructive/10 px-2 py-1 font-medium text-destructive"
            onClick={() => setView(view === "collapsed" ? "form" : "collapsed")}
          >
            <KeyRound size={12} /> Add access token
          </button>
        )}
        {!ok && status?.gitlab_error && (
          <span className="truncate text-muted-foreground">{status.gitlab_error}</span>
        )}
      </div>

      {view === "menu" && (
        <div className="flex gap-2">
          <Button disabled={busy} onClick={validate}>
            Validate
          </Button>
          <Button disabled={busy} onClick={() => setView("form")}>
            Update
          </Button>
          <Button variant="destructive" disabled={busy || !host.trim()} onClick={remove}>
            Delete
          </Button>
        </div>
      )}

      {view === "form" && (
        <div className="flex flex-col gap-2">
          <Input
            value={host}
            onChange={(e) => {
              setHost(e.target.value)
              localStorage.setItem(HOST_KEY, e.target.value)
            }}
            placeholder="Provider host (e.g. github.com or gitlab.example.com)"
          />
          <Input
            type="password"
            autoFocus
            value={token}
            onChange={(e) => setToken(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && host.trim() && token.trim() && save()}
            placeholder="Personal Access Token"
          />
          <div className="flex gap-2">
            <Button variant="primary" disabled={busy || !host.trim() || !token.trim()} onClick={save}>
              {busy ? "Saving…" : "Save token"}
            </Button>
            <Button disabled={busy} onClick={() => setView(ok ? "menu" : "collapsed")}>
              Cancel
            </Button>
          </div>
        </div>
      )}

      {msg && <p className="text-muted-foreground">{msg}</p>}
      {validation && (
        <ul className="flex flex-col gap-0.5 border-t border-border pt-2 text-muted-foreground">
          {validation.capabilities.map((c) => (
            <li key={c.label}>
              {c.status === "yes" ? "✓" : c.status === "no" ? "✗" : "?"} {c.label} — {c.reason}
            </li>
          ))}
        </ul>
      )}
      {error && <ErrorLine error={error} />}
    </div>
  )
}
