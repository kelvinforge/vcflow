import * as vscode from "vscode";
import { VcflowTreeProvider } from "./treeProvider";
import { pickWorkspaceFolder, currentWorkspaceFolder } from "./workspace";
import { runNextAction, saveToken } from "./actions";

export function activate(context: vscode.ExtensionContext): void {
  const provider = new VcflowTreeProvider(context);
  const view = vscode.window.createTreeView("vcflowView", { treeDataProvider: provider });
  context.subscriptions.push(view);

  context.subscriptions.push(
    vscode.commands.registerCommand("vcflow.refresh", () => provider.refresh()),
  );
  context.subscriptions.push(
    vscode.commands.registerCommand("vcflow.selectWorkspace", async () => {
      const picked = await pickWorkspaceFolder(context);
      if (picked) {
        await provider.refresh();
      }
    }),
  );
  context.subscriptions.push(
    vscode.commands.registerCommand("vcflow.showError", (message: string) => {
      void vscode.window.showErrorMessage(message);
    }),
  );
  context.subscriptions.push(
    vscode.commands.registerCommand(
      "vcflow.runNextAction",
      async (primary: string, folderUri: vscode.Uri) => {
        const folder = vscode.workspace.getWorkspaceFolder(folderUri);
        if (!folder) {
          void vscode.window.showErrorMessage("VCFlow: workspace folder is no longer open.");
          return;
        }
        await runNextAction(context, folder, primary, () => provider.refresh());
      },
    ),
  );
  context.subscriptions.push(
    vscode.commands.registerCommand("vcflow.saveToken", async () => {
      const folder = currentWorkspaceFolder(context);
      if (!folder) {
        void vscode.window.showWarningMessage("VCFlow: no workspace folder is open.");
        return;
      }
      await saveToken(context, folder, () => provider.refresh());
    }),
  );

  // Keep the view current as the window's folder set changes (folder
  // added/removed in a multi-root workspace) -- never on a keybinding, and
  // never intercepting keyboard input globally.
  context.subscriptions.push(
    vscode.workspace.onDidChangeWorkspaceFolders(() => provider.refresh()),
  );

  void provider.refresh();
}

export function deactivate(): void {
  // No background process, timers, or watchers are started by this
  // extension -- nothing to tear down.
}
