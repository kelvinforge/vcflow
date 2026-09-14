// Single-workspace-folder resolution for a multi-root VS Code window.
// VCFlow operates on one repository at a time; when more than one folder is
// open we ask which one explicitly rather than silently guessing.

import * as vscode from "vscode";

const SELECTED_KEY = "vcflow.selectedWorkspaceUri";

export function currentWorkspaceFolder(
  context: vscode.ExtensionContext,
): vscode.WorkspaceFolder | undefined {
  const folders = vscode.workspace.workspaceFolders;
  if (!folders || folders.length === 0) {
    return undefined;
  }
  if (folders.length === 1) {
    return folders[0];
  }
  const selected = context.workspaceState.get<string>(SELECTED_KEY);
  const match = folders.find((f) => f.uri.toString() === selected);
  return match ?? folders[0];
}

export async function pickWorkspaceFolder(
  context: vscode.ExtensionContext,
): Promise<vscode.WorkspaceFolder | undefined> {
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
