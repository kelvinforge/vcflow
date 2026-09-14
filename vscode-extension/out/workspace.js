"use strict";
// Single-workspace-folder resolution for a multi-root VS Code window.
// VCFlow operates on one repository at a time; when more than one folder is
// open we ask which one explicitly rather than silently guessing.
Object.defineProperty(exports, "__esModule", { value: true });
exports.currentWorkspaceFolder = currentWorkspaceFolder;
exports.pickWorkspaceFolder = pickWorkspaceFolder;
const vscode = require("vscode");
const SELECTED_KEY = "vcflow.selectedWorkspaceUri";
function currentWorkspaceFolder(context) {
    const folders = vscode.workspace.workspaceFolders;
    if (!folders || folders.length === 0) {
        return undefined;
    }
    if (folders.length === 1) {
        return folders[0];
    }
    const selected = context.workspaceState.get(SELECTED_KEY);
    const match = folders.find((f) => f.uri.toString() === selected);
    return match ?? folders[0];
}
async function pickWorkspaceFolder(context) {
    const folders = vscode.workspace.workspaceFolders;
    if (!folders || folders.length === 0) {
        vscode.window.showWarningMessage("VCFlow: no workspace folder is open.");
        return undefined;
    }
    if (folders.length === 1) {
        return folders[0];
    }
    const picked = await vscode.window.showWorkspaceFolderPick({
        placeHolder: "Select the repository VCFlow should show",
    });
    if (picked) {
        await context.workspaceState.update(SELECTED_KEY, picked.uri.toString());
    }
    return picked;
}
//# sourceMappingURL=workspace.js.map