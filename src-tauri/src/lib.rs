mod models;
mod commands;
mod process_manager;

use crate::process_manager::ProcessManager;
use std::sync::Arc;
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .manage(Arc::new(ProcessManager::new()))
        .setup(|app| {
            #[cfg(target_os = "macos")]
            {
                use window_vibrancy::{apply_vibrancy, NSVisualEffectMaterial, NSVisualEffectState};
                if let Some(window) = app.get_webview_window("main") {
                    let _ = apply_vibrancy(&window, NSVisualEffectMaterial::UnderWindowBackground, Some(NSVisualEffectState::Active), None);
                }
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::detect_project,
            commands::scan_projects,
            commands::get_tool_versions,
            commands::run_action,
            commands::stop_process,
            commands::git_log,
            commands::git_init,
            commands::git_status,
            commands::pick_folder,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}