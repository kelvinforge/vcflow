// Native TreeView data provider. No Webview, no custom keyboard handling --
// this is a plain vscode.TreeDataProvider rendered by VS Code's own tree
// widget. Every fact shown here is read verbatim from `vcflow`'s JSON output;
// this file never re-derives a workflow decision (that stays in
// workflow_engine/workflow_service, behind the CLI).
//
// Information architecture (spec: VCFlow Tauri -> VS Code UI Migration, §3-8):
// answers, in order, "what branch / is my tree safe / what's next / what am I
// on / what else is active / what's waiting for review" -- as sections, not a
// flat list, with Next Action as the most prominent one.

import * as vscode from "vscode";
import { resolveExecutable, runVcflow, VcflowError } from "./vcflowClient";
import { currentWorkspaceFolder } from "./workspace";
import { isActionable } from "./actions";

interface RepoStatus {
  branch: string;
  dirty: boolean;
  dirty_count: number;
  ahead: number;
  behind: number;
  diverged: boolean;
  branch_guard: string | null;
  role: string;
}

interface NextAction {
  title: string;
  description: string;
  primary: string | null;
  helper: string | null;
}

interface WipItem {
  branch: string;
  work_type: string;
  status: string;
}

interface WorkList {
  current: WipItem | null;
  other: WipItem[];
  waiting: WipItem[];
}

/** A blocked next-action needs the user's hands (a real conflict), not a
 * single click -- distinct from "nothing to do" (primary === null) and a
 * normal one-click actionable step. */
const BLOCKED_PRIMARY_ACTIONS = new Set(["resolve_in_working_dir", "resolve_mr_conflict"]);

type Node =
  | { kind: "message"; label: string; description?: string; tooltip?: string; icon?: vscode.ThemeIcon; command?: vscode.Command }
  | { kind: "section"; id: string; label: string; children: Node[] }
  | { kind: "leaf"; label: string; description?: string; tooltip?: string; icon?: vscode.ThemeIcon; command?: vscode.Command };

export class VcflowTreeProvider implements vscode.TreeDataProvider<Node> {
  private readonly onDidChangeTreeDataEmitter = new vscode.EventEmitter<void>();
  readonly onDidChangeTreeData = this.onDidChangeTreeDataEmitter.event;

  private roots: Node[] = [];
  private loading = false;

  constructor(private readonly context: vscode.ExtensionContext) {}

  getTreeItem(node: Node): vscode.TreeItem {
    if (node.kind === "section") {
      const item = new vscode.TreeItem(node.label, vscode.TreeItemCollapsibleState.Expanded);
      item.contextValue = "vcflowSection";
      return item;
    }
    const item = new vscode.TreeItem(node.label, vscode.TreeItemCollapsibleState.None);
    item.description = node.description;
    item.tooltip = node.tooltip;
    item.iconPath = node.icon;
    item.command = node.command;
    return item;
  }

  getChildren(element?: Node): Node[] {
    if (!element) {
      return this.roots;
    }
    return element.kind === "section" ? element.children : [];
  }

  private setRoots(roots: Node[]): void {
    this.roots = roots;
    this.onDidChangeTreeDataEmitter.fire();
  }

  private setMessage(node: Omit<Extract<Node, { kind: "message" }>, "kind">): void {
    this.setRoots([{ kind: "message", ...node }]);
  }

  /** Re-reads real repo/workflow state from `vcflow` and redraws. Never
   * throws -- every failure path (no workspace, no executable, process
   * failure, bad JSON) ends in an error node instead of an unhandled
   * rejection, so a broken `vcflow` cannot crash the extension host. */
  async refresh(): Promise<void> {
    if (this.loading) {
      return;
    }
    this.loading = true;
    try {
      await this.load();
    } finally {
      this.loading = false;
    }
  }

  private async load(): Promise<void> {
    const folder = currentWorkspaceFolder(this.context);
    if (!folder) {
      this.setMessage({ label: "No workspace open", icon: new vscode.ThemeIcon("warning") });
      return;
    }

    this.setMessage({ label: "Loading…", icon: new vscode.ThemeIcon("sync~spin") });

    let exe: string;
    try {
      exe = resolveExecutable(this.context, folder);
    } catch (e) {
      this.setMessage({
        label: "vcflow not found",
        description: "click to open settings",
        tooltip: (e as VcflowError).message,
        icon: new vscode.ThemeIcon("error"),
        command: {
          command: "workbench.action.openSettings",
          title: "Open Settings",
          arguments: ["vcflow.executablePath"],
        },
      });
      return;
    }

    const repoPath = folder.uri.fsPath;
    const [repoStatusResult, nextActionResult, listResult] = await Promise.allSettled([
      runVcflow(exe, ["repo-status", "--repo", repoPath], repoPath) as Promise<RepoStatus>,
      runVcflow(exe, ["status", "--repo", repoPath], repoPath) as Promise<NextAction>,
      runVcflow(exe, ["list", "--repo", repoPath], repoPath) as Promise<WorkList>,
    ]);

    // A `vcflow` process/parse failure on any one call surfaces as a single
    // clear error node rather than a partially-populated, misleading tree.
    for (const result of [repoStatusResult, nextActionResult, listResult]) {
      if (result.status === "rejected") {
        const err = result.reason;
        const message = err instanceof VcflowError ? err.message : String(err);
        this.setMessage({
          label: "vcflow failed",
          description: message,
          tooltip: message,
          icon: new vscode.ThemeIcon("error"),
          command: {
            command: "vcflow.showError",
            title: "Show full error",
            arguments: [message],
          },
        });
        return;
      }
    }

    const repoStatus = (repoStatusResult as PromiseFulfilledResult<RepoStatus>).value;
    const nextAction = (nextActionResult as PromiseFulfilledResult<NextAction>).value;
    const list = (listResult as PromiseFulfilledResult<WorkList>).value;

    this.setRoots([
      repositorySection(repoStatus),
      nextActionSection(nextAction, folder.uri),
      currentWorkSection(list.current),
      otherWorkSection(list.other),
      waitingSection(list.waiting),
    ]);
  }
}

function section(id: string, label: string, children: Node[]): Node {
  return { kind: "section", id, label, children };
}

function leaf(
  label: string,
  description?: string,
  tooltip?: string,
  icon?: vscode.ThemeIcon,
  command?: vscode.Command,
): Node {
  return { kind: "leaf", label, description, tooltip, icon, command };
}

function repositorySection(repoStatus: RepoStatus): Node {
  const protectedText = repoStatus.branch_guard ? `Protected: ${repoStatus.branch_guard}` : "Protected: no";
  const branch = leaf(
    "Branch",
    repoStatus.branch,
    `Branch\n${repoStatus.branch}\n\n${protectedText}\nRole: ${repoStatus.role}`,
    new vscode.ThemeIcon(repoStatus.branch_guard ? "shield" : "git-branch"),
  );

  const bits: string[] = [repoStatus.dirty ? `${repoStatus.dirty_count} change${repoStatus.dirty_count === 1 ? "" : "s"}` : "Clean"];
  if (repoStatus.ahead > 0) bits.push(`↑${repoStatus.ahead}`);
  if (repoStatus.behind > 0) bits.push(`↓${repoStatus.behind}`);
  if (repoStatus.diverged) bits.push("Diverged");
  const workingTree = leaf(
    "Working Tree",
    bits.join(" · "),
    "Working tree state as reported by vcflow (never computed by the extension).",
    new vscode.ThemeIcon(repoStatus.diverged ? "warning" : repoStatus.dirty ? "diff-modified" : "check"),
  );

  return section("repository", "Repository", [branch, workingTree]);
}

function nextActionSection(nextAction: NextAction, folderUri: vscode.Uri): Node {
  const icon =
    nextAction.primary === null
      ? new vscode.ThemeIcon("info")
      : BLOCKED_PRIMARY_ACTIONS.has(nextAction.primary)
        ? new vscode.ThemeIcon("error")
        : isActionable(nextAction.primary)
          ? new vscode.ThemeIcon("arrow-right")
          : new vscode.ThemeIcon("circle-outline");

  const clickable = isActionable(nextAction.primary);
  const tooltipLines = [nextAction.description, nextAction.helper];
  if (nextAction.primary && !clickable) {
    tooltipLines.push("(not yet runnable from VS Code -- use the Tauri app for this step)");
  }
  const tooltip = tooltipLines.filter(Boolean).join("\n\n");

  const command: vscode.Command | undefined = clickable
    ? {
        command: "vcflow.runNextAction",
        title: "Run",
        arguments: [nextAction.primary, folderUri],
      }
    : undefined;

  const item = leaf(nextAction.title, undefined, tooltip || undefined, icon, command);
  return section("nextAction", "Next Action", [item]);
}

function workItemTooltip(item: WipItem): string {
  return `${item.branch}\nType: ${item.work_type}\nStatus: ${item.status}`;
}

function currentWorkSection(current: WipItem | null): Node {
  const child = current
    ? leaf(current.branch, current.status, workItemTooltip(current), new vscode.ThemeIcon("git-commit"))
    : leaf("None", undefined, "No work item is currently checked out.", new vscode.ThemeIcon("circle-slash"));
  // Always shown, even when empty -- an empty Current Work is itself workflow
  // state (spec §7: "empty state communicates workflow state").
  return section("currentWork", "Current Work", [child]);
}

function otherWorkSection(items: WipItem[]): Node {
  const children = items.length
    ? items.map((item) => leaf(item.branch, item.status, workItemTooltip(item), new vscode.ThemeIcon("history")))
    : [leaf("None", undefined, "No other active work items.", new vscode.ThemeIcon("dash"))];
  return section("otherWork", "Other Work", children);
}

function waitingSection(items: WipItem[]): Node {
  const children = items.length
    ? items.map((item) =>
        leaf(
          item.branch,
          "waiting for review",
          `${workItemTooltip(item)}\n\nThis work item has been handed off and is waiting on review/merge, not simply another branch.`,
          new vscode.ThemeIcon("clock"),
        ),
      )
    : [leaf("None", undefined, "Nothing is waiting on review.", new vscode.ThemeIcon("dash"))];
  return section("waiting", "Waiting on Review", children);
}
