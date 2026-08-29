// Action Registry -- the single place that maps a backend NextAction id to
// UI (label/icon) and a handler. Components never branch on `primary`
// themselves; they look it up here. The registry does NOT derive workflow
// state -- it only knows "given this id the backend chose, what does the
// button say and which command does it call".

import type { LucideIcon } from "lucide-react"
import { GitCommitVertical, GitPullRequestArrow, Play, Rocket, TriangleAlert, Undo2 } from "lucide-react"
import {
  createWorkItem,
  finishHotfix,
  finishWorkItem,
  commitWorkItem,
  type PrimaryActionId,
  type WorkItemKind,
} from "@/lib/tauri"

/** Input fields an action needs collected before it can run. */
export type FieldName = "message" | "title" | "kind" | "slug"

export interface RunCtx {
  repoPath: string
  values: Partial<Record<FieldName, string>>
}

export interface ActionDef {
  label: string
  icon: LucideIcon
  /** "trigger": has a run(); "guidance": show the backend description, no button. */
  kind: "trigger" | "guidance"
  fields: FieldName[]
  run?: (ctx: RunCtx) => Promise<unknown>
  /** "conflict": NextActionCard defers to the Conflict Resolution panel. */
  defersTo?: "conflict"
}

export const ACTIONS: Record<PrimaryActionId, ActionDef> = {
  commit: {
    label: "Commit changes",
    icon: GitCommitVertical,
    kind: "trigger",
    fields: ["message"],
    run: ({ repoPath, values }) => commitWorkItem(repoPath, values.message ?? ""),
  },
  finish: {
    label: "Finish (push + open MR)",
    icon: GitPullRequestArrow,
    kind: "trigger",
    fields: ["title"],
    run: ({ repoPath, values }) => finishWorkItem(repoPath, values.title ?? ""),
  },
  finish_hotfix: {
    label: "Finish hotfix (push + MR → master + sync MR)",
    icon: Rocket,
    kind: "trigger",
    fields: ["title"],
    run: ({ repoPath, values }) => finishHotfix(repoPath, values.title ?? ""),
  },
  start_work_item: {
    label: "Start work item",
    icon: Play,
    kind: "trigger",
    fields: ["kind", "slug"],
    run: ({ repoPath, values }) =>
      createWorkItem(repoPath, (values.kind as WorkItemKind) ?? "feature", values.slug ?? ""),
  },
  resolve_mr_conflict: {
    label: "Resolve merge conflict",
    icon: TriangleAlert,
    kind: "guidance",
    fields: [],
    defersTo: "conflict",
  },
  resolve_in_working_dir: {
    label: "Resolve in your working directory",
    icon: TriangleAlert,
    kind: "guidance",
    fields: [],
  },
  return_to_develop: {
    label: "Return to develop",
    icon: Undo2,
    kind: "guidance",
    fields: [],
  },
}
