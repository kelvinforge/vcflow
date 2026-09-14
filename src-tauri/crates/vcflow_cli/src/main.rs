//! Standalone `vcflow` executable. Thin argv/stdout adapter over
//! `workflow_service` -- the same orchestration path the Tauri app uses, so
//! there is exactly one workflow implementation, not two. Compiles and runs
//! independently of Tauri; parses no branch/MR/release rules itself.
//!
//! Every subcommand below is a 1:1 mapping to the Tauri command of the same
//! orchestration (see src-tauri/src/commands.rs for the Tauri-side wrapper):
//!
//!   status                    -> get_next_action
//!   repo-status               -> get_repo_status
//!   list                      -> list_work_items
//!   create-work-item          -> create_work_item
//!   move-to-new-branch        -> move_changes_to_new_branch
//!   commit                    -> commit_work_item
//!   push                      -> push_work_item
//!   finish                    -> finish_work_item
//!   save-work                 -> save_work
//!   resume-work               -> resume_work
//!   discard-work              -> discard_work
//!   hotfix-create             -> create_hotfix
//!   hotfix-finish             -> finish_hotfix
//!   hotfix-status             -> get_hotfix_status
//!   release-preview           -> get_release_preview
//!   release-create            -> create_release_candidate
//!   release-finish            -> finish_release
//!   release-sync              -> sync_develop_after_release
//!   release-status            -> get_release_status
//!   update-branch             -> update_branch
//!   save-token                -> save_token
//!
//! Not exposed here (Tauri-only, per the task scope): setup/preflight wizard,
//! conflict resolution, role overrides, audit log viewing, and OS-integration
//! commands (open folder/URL). None of these are workflow orchestration --
//! see the Remaining Work section of the report.

use std::process::ExitCode;

fn print_json<T: serde::Serialize>(value: &T) {
    println!("{}", serde_json::to_string_pretty(value).unwrap());
}

fn print_error(message: &str) {
    let payload = serde_json::json!({ "error": message });
    eprintln!("{}", serde_json::to_string(&payload).unwrap());
}

fn flag(args: &[String], name: &str) -> Option<String> {
    let idx = args.iter().position(|a| a == name)?;
    args.get(idx + 1).cloned()
}

fn flag_present(args: &[String], name: &str) -> bool {
    args.iter().any(|a| a == name)
}

fn require_flag(args: &[String], name: &str) -> Result<String, String> {
    flag(args, name).ok_or_else(|| format!("missing required flag {name}"))
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();

    let (command, rest) = match args.split_first() {
        Some((cmd, rest)) => (cmd.as_str(), rest.to_vec()),
        None => {
            print_error("usage: vcflow <command> --repo <path> [options] (see --help)");
            return ExitCode::FAILURE;
        }
    };

    let result = run(command, &rest).await;
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            print_error(&message);
            ExitCode::FAILURE
        }
    }
}

async fn run(command: &str, args: &[String]) -> Result<(), String> {
    let repo = || require_flag(args, "--repo");

    match command {
        "status" => print_json(&workflow_service::get_next_action(&repo()?).await?),
        "repo-status" => print_json(&workflow_service::get_repo_status(repo()?).await?),
        "list" => print_json(&workflow_service::list_work_items(repo()?).await?),
        "create-work-item" => {
            let kind = require_flag(args, "--kind")?;
            let slug = require_flag(args, "--slug")?;
            print_json(&workflow_service::create_work_item(repo()?, kind, slug).await?)
        }
        "move-to-new-branch" => {
            let kind = require_flag(args, "--kind")?;
            let slug = require_flag(args, "--slug")?;
            print_json(&workflow_service::move_changes_to_new_branch(repo()?, kind, slug).await?)
        }
        "commit" => {
            let message = require_flag(args, "--message")?;
            print_json(&workflow_service::commit_work_item(repo()?, message).await?)
        }
        "push" => print_json(&workflow_service::push_work_item(repo()?).await?),
        "finish" => {
            let title = require_flag(args, "--title")?;
            print_json(&workflow_service::finish_work_item(repo()?, title).await?)
        }
        "save-work" => print_json(&workflow_service::save_work(repo()?).await?),
        "resume-work" => {
            let id = require_flag(args, "--id")?.parse::<i64>().map_err(|e| e.to_string())?;
            print_json(&workflow_service::resume_work(repo()?, id).await?)
        }
        "discard-work" => {
            let id = require_flag(args, "--id")?.parse::<i64>().map_err(|e| e.to_string())?;
            print_json(&workflow_service::discard_work(repo()?, id).await?)
        }
        "hotfix-create" => {
            let slug = require_flag(args, "--slug")?;
            print_json(&workflow_service::create_hotfix(repo()?, slug).await?)
        }
        "hotfix-finish" => {
            let title = require_flag(args, "--title")?;
            print_json(&workflow_service::finish_hotfix(repo()?, title).await?)
        }
        "hotfix-status" => print_json(&workflow_service::get_hotfix_status(repo()?).await?),
        "release-preview" => print_json(&workflow_service::get_release_preview(repo()?).await?),
        "release-create" => {
            let version = require_flag(args, "--version")?;
            let changelog = flag(args, "--changelog").unwrap_or_default();
            let supersede = flag_present(args, "--supersede-confirmed");
            print_json(
                &workflow_service::create_release_candidate(repo()?, version, changelog, supersede)
                    .await?,
            )
        }
        "release-finish" => {
            let title = require_flag(args, "--title")?;
            print_json(&workflow_service::finish_release(repo()?, title).await?)
        }
        "release-sync" => {
            let branch = require_flag(args, "--branch")?;
            let title = require_flag(args, "--title")?;
            print_json(&workflow_service::sync_develop_after_release(repo()?, branch, title).await?)
        }
        "release-status" => print_json(&workflow_service::get_release_status(repo()?).await?),
        "update-branch" => print_json(&workflow_service::update_branch(repo()?).await?),
        "save-token" => {
            let host = require_flag(args, "--host")?;
            let token = require_flag(args, "--token")?;
            workflow_service::save_token(repo()?, host, token).await?;
            print_json(&serde_json::json!({ "ok": true }))
        }
        other => return Err(format!("unknown command '{other}'")),
    }
    Ok(())
}
