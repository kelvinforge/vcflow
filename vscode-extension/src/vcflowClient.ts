// Thin process adapter over the standalone `vcflow` executable. Never
// reimplements workflow rules, never runs git itself -- every fact shown by
// the extension comes from parsing `vcflow`'s JSON stdout.

import * as cp from "child_process";
import * as fs from "fs";
import * as path from "path";
import * as vscode from "vscode";

export class VcflowError extends Error {}

function exeName(): string {
  return process.platform === "win32" ? "vcflow.exe" : "vcflow";
}

/**
 * Resolution order:
 *   1. `vcflow.executablePath` setting, if set.
 *   2. `<workspace>/src-tauri/target/debug/vcflow` then `.../release/vcflow`
 *      (this repo's own dev build -- the common case during development).
 *   3. `<extension>/bin/vcflow` (a future bundled binary; not shipped yet).
 * Throws `VcflowError` with a message meant to be shown directly to the user
 * when nothing exists.
 */
export function resolveExecutable(
  context: vscode.ExtensionContext,
  workspaceFolder: vscode.WorkspaceFolder | undefined,
): string {
  const configured = vscode.workspace
    .getConfiguration("vcflow", workspaceFolder)
    .get<string>("executablePath");
  const candidates: string[] = [];
  if (configured && configured.trim().length > 0) {
    candidates.push(configured.trim());
  }
  if (workspaceFolder) {
    const base = workspaceFolder.uri.fsPath;
    candidates.push(path.join(base, "src-tauri", "target", "debug", exeName()));
    candidates.push(path.join(base, "src-tauri", "target", "release", exeName()));
  }
  candidates.push(path.join(context.extensionPath, "bin", exeName()));

  for (const candidate of candidates) {
    if (candidate && fs.existsSync(candidate)) {
      return candidate;
    }
  }
  throw new VcflowError(
    `vcflow executable not found. Set "vcflow.executablePath" in Settings, or build it ` +
      `(cargo build -p vcflow_cli) inside a workspace's src-tauri/. Looked at:\n` +
      candidates.map((c) => `  - ${c}`).join("\n"),
  );
}

/** Runs `<exe> <args>`, parses stdout as JSON. Rejects with `VcflowError` on
 * spawn failure, non-zero exit (using the process's own `{"error": "..."}`
 * stderr contract when present), or unparsable output -- never throws
 * synchronously, so a caller can always show the error instead of crashing. */
export function runVcflow(exe: string, args: string[], cwd: string): Promise<unknown> {
  return new Promise((resolve, reject) => {
    cp.execFile(
      exe,
      args,
      { cwd, maxBuffer: 10 * 1024 * 1024, timeout: 30_000 },
      (err, stdout, stderr) => {
        if (err) {
          let message = stderr?.trim() || err.message;
          try {
            const parsed = JSON.parse(stderr);
            if (parsed && typeof parsed.error === "string") {
              message = parsed.error;
            }
          } catch {
            // stderr wasn't JSON -- fall back to the raw text/err.message above.
          }
          reject(new VcflowError(message));
          return;
        }
        try {
          resolve(JSON.parse(stdout));
        } catch (e) {
          reject(new VcflowError(`could not parse vcflow output as JSON: ${(e as Error).message}`));
        }
      },
    );
  });
}
