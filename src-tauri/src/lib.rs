mod commands;
mod events;
mod remote_probe_cache;
mod repo_lock;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
  // Cap libgit2's TCP connect (SSH probe, push, fetch) instead of the OS
  // default (~75s): preflight runs its SSH probe while holding the repo lock.
  // Safety: process-global libgit2 option, set once before any git2 use.
  let _ = unsafe { git2::opts::set_server_connect_timeout_in_milliseconds(10_000) };
  tauri::Builder::default()
    .plugin(tauri_plugin_dialog::init())
    .plugin(tauri_plugin_process::init())
    .manage(repo_lock::RepoLockRegistry::default())
    .manage(remote_probe_cache::RemoteProbeCache::default())
    .setup(|app| {
      #[cfg(desktop)]
      app
        .handle()
        .plugin(tauri_plugin_updater::Builder::new().build())?;
      if cfg!(debug_assertions) {
        app.handle().plugin(
          tauri_plugin_log::Builder::default()
            .level(log::LevelFilter::Info)
            .build(),
        )?;
      }
      Ok(())
    })
    .invoke_handler(tauri::generate_handler![
      commands::get_repo_status,
      commands::refresh_repo_status,
      commands::repository_preflight,
      commands::initialize_workflow,
      commands::get_setup_state,
      commands::save_token,
      commands::create_work_item,
      commands::move_changes_to_new_branch,
      commands::commit_work_item,
      commands::push_work_item,
      commands::finish_work_item,
      commands::get_mr_status,
      commands::get_next_action,
      commands::save_work,
      commands::list_saved_work,
      commands::resume_work,
      commands::discard_work,
      commands::list_work_items,
      commands::continue_work,
      commands::inspect_branch,
      commands::end_branch_inspection,
      commands::drop_work,
      commands::create_hotfix,
      commands::finish_hotfix,
      commands::get_hotfix_status,
      commands::start_conflict_resolution,
      commands::open_in_external_tool,
      commands::verify_and_commit_resolution,
      commands::re_validate_token,
      commands::delete_token,
      commands::list_role_overrides,
      commands::set_role_override,
      commands::remove_role_override,
      commands::get_audit_log,
      commands::get_command_log,
      commands::get_hotfix_version_preview,
      commands::get_release_preview,
      commands::create_release_candidate,
      commands::finish_release,
      commands::sync_develop_after_release,
      commands::get_release_status,
      commands::update_branch,
      commands::open_working_directory,
      commands::open_url
    ])
    .run(tauri::generate_context!())
    .expect("error while running tauri application");
}
