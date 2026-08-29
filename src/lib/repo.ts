// Per-viewer repo picker persistence. localStorage only -- convenience, not
// state the backend cares about.

const RECENTS_KEY = "gwe.recentRepos"
const LAST_REPO_KEY = "gwe.lastRepo"

export function loadLastRepo(): string {
  return localStorage.getItem(LAST_REPO_KEY) ?? "."
}

export function loadRecentRepos(): string[] {
  try {
    return JSON.parse(localStorage.getItem(RECENTS_KEY) ?? "[]")
  } catch {
    return []
  }
}

export function rememberRepo(path: string, current: string[]): string[] {
  const next = [path, ...current.filter((r) => r !== path)].slice(0, 8)
  localStorage.setItem(LAST_REPO_KEY, path)
  localStorage.setItem(RECENTS_KEY, JSON.stringify(next))
  return next
}
