"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.activate = activate;
exports.deactivate = deactivate;
const vscode = require("vscode");
const treeProvider_1 = require("./treeProvider");
const workspace_1 = require("./workspace");
const actions_1 = require("./actions");
function activate(context) {
    const provider = new treeProvider_1.VcflowTreeProvider(context);
    const view = vscode.window.createTreeView("vcflowView", { treeDataProvider: provider });
    context.subscriptions.push(view);
    context.subscriptions.push(vscode.commands.registerCommand("vcflow.refresh", () => provider.refresh()));
    context.subscriptions.push(vscode.commands.registerCommand("vcflow.selectWorkspace", async () => {
        const picked = await (0, workspace_1.pickWorkspaceFolder)(context);
        if (picked) {
            await provider.refresh();
        }
    }));
    context.subscriptions.push(vscode.commands.registerCommand("vcflow.showError", (message) => {
        void vscode.window.showErrorMessage(message);
    }));
    context.subscriptions.push(vscode.commands.registerCommand("vcflow.runNextAction", async (primary, folderUri) => {
        const folder = vscode.workspace.getWorkspaceFolder(folderUri);
        if (!folder) {
            void vscode.window.showErrorMessage("VCFlow: workspace folder is no longer open.");
            return;
        }
        await (0, actions_1.runNextAction)(context, folder, primary, () => provider.refresh());
    }));
    // Keep the view current as the window's folder set changes (folder
    // added/removed in a multi-root workspace) -- never on a keybinding, and
    // never intercepting keyboard input globally.
    context.subscriptions.push(vscode.workspace.onDidChangeWorkspaceFolders(() => provider.refresh()));
    void provider.refresh();
}
function deactivate() {
    // No background process, timers, or watchers are started by this
    // extension -- nothing to tear down.
}
//# sourceMappingURL=extension.js.map