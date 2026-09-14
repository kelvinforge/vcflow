"use strict";
// Executes a Next Action's `primary` id by calling the matching `vcflow`
// mutation, prompting for any required input with native VS Code UI
// (QuickPick/InputBox) -- never a Webview form. This file decides *which*
// CLI subcommand to run and what to ask the user for; it never decides
// *whether* the action is allowed (that stays server-side in
// workflow_service/workflow_engine, enforced again on every CLI call).
Object.defineProperty(exports, "__esModule", { value: true });
exports.isActionable = isActionable;
exports.runNextAction = runNextAction;
const vscode = require("vscode");
const vcflowClient_1 = require("./vcflowClient");
/** `null` return means "nothing to run" (user cancelled a prompt, or this
 * primary action has no CLI-executable mapping yet) -- never a silent no-op
 * that pretends to have succeeded. */
async function argsFor(primary, repoPath) {
    switch (primary) {
        case "commit": {
            const message = await vscode.window.showInputBox({
                prompt: "Commit message",
                ignoreFocusOut: true,
            });
            return message ? ["commit", "--repo", repoPath, "--message", message] : null;
        }
        case "push":
            return ["push", "--repo", repoPath];
        case "finish": {
            const title = await vscode.window.showInputBox({
                prompt: "Merge request title (target: develop)",
                ignoreFocusOut: true,
            });
            return title ? ["finish", "--repo", repoPath, "--title", title] : null;
        }
        case "finish_hotfix": {
            const title = await vscode.window.showInputBox({
                prompt: "Hotfix merge request title",
                ignoreFocusOut: true,
            });
            return title ? ["hotfix-finish", "--repo", repoPath, "--title", title] : null;
        }
        case "finish_release": {
            const title = await vscode.window.showInputBox({
                prompt: "Release merge request title",
                ignoreFocusOut: true,
            });
            return title ? ["release-finish", "--repo", repoPath, "--title", title] : null;
        }
        case "sync_develop": {
            const branch = await vscode.window.showInputBox({
                prompt: "Release candidate branch (e.g. release/1.4.0)",
                ignoreFocusOut: true,
            });
            if (!branch)
                return null;
            const title = await vscode.window.showInputBox({
                prompt: "Sync merge request title",
                value: `sync: ${branch}`,
                ignoreFocusOut: true,
            });
            return title ? ["release-sync", "--repo", repoPath, "--branch", branch, "--title", title] : null;
        }
        case "update_branch":
            return ["update-branch", "--repo", repoPath];
        case "start_work_item": {
            const kind = await vscode.window.showQuickPick(["feature", "bug", "chore"], {
                placeHolder: "Work item type",
            });
            if (!kind)
                return null;
            const slug = await vscode.window.showInputBox({
                prompt: "Branch slug (e.g. payment-retry)",
                ignoreFocusOut: true,
            });
            return slug ? ["create-work-item", "--repo", repoPath, "--kind", kind, "--slug", slug] : null;
        }
        case "move_to_new_branch": {
            const kind = await vscode.window.showQuickPick(["feature", "bug", "chore"], {
                placeHolder: "Work item type for the new branch",
            });
            if (!kind)
                return null;
            const slug = await vscode.window.showInputBox({
                prompt: "Branch slug (e.g. payment-retry)",
                ignoreFocusOut: true,
            });
            return slug ? ["move-to-new-branch", "--repo", repoPath, "--kind", kind, "--slug", slug] : null;
        }
        default:
            return null;
    }
}
/** `resolve_in_working_dir` / `resolve_mr_conflict` (a real conflict) and
 * `return_to_develop` (no dedicated CLI mutation -- Tauri does this as a
 * side effect of push/finish, never standalone) have no 1-click mapping;
 * anything not listed in `argsFor` falls here too. */
const NOT_CLICKABLE = new Set(["resolve_in_working_dir", "resolve_mr_conflict", "return_to_develop"]);
function isActionable(primary) {
    return primary !== null && !NOT_CLICKABLE.has(primary);
}
async function runNextAction(context, folder, primary, onDone) {
    const repoPath = folder.uri.fsPath;
    let exe;
    try {
        exe = (0, vcflowClient_1.resolveExecutable)(context, folder);
    }
    catch (e) {
        void vscode.window.showErrorMessage(e.message);
        return;
    }
    const args = await argsFor(primary, repoPath);
    if (!args) {
        // User cancelled a prompt, or this action has no CLI mapping -- either
        // way, nothing ran and nothing needs reporting as a failure.
        return;
    }
    try {
        await (0, vcflowClient_1.runVcflow)(exe, args, repoPath);
        await onDone();
    }
    catch (e) {
        const message = e instanceof vcflowClient_1.VcflowError ? e.message : String(e);
        void vscode.window.showErrorMessage(`VCFlow: ${message}`);
    }
}
//# sourceMappingURL=actions.js.map