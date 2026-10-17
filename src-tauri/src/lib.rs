mod account;
mod commands;
mod files;
mod forwarding;
mod library;
mod settings;
mod state;

use tauri::Manager;

use crate::state::AppState;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_clipboard_manager::init())
        .setup(|app| {
            let state = AppState::load(app.handle())?;
            app.manage(state);
            account::spawn_sync_loop(app.handle().clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::list_hosts,
            commands::save_host,
            commands::delete_host,
            commands::get_host_secret,
            commands::set_host_secret,
            commands::trust_host_key,
            commands::connect,
            commands::open_local,
            commands::list_serial_ports,
            commands::open_serial,
            commands::write,
            commands::resize,
            commands::disconnect,
            account::account_status,
            account::sign_up,
            account::log_in,
            account::unlock_password_sync,
            account::change_password,
            account::log_out,
            account::sync_now,
            library::list_snippets,
            library::save_snippet,
            library::delete_snippet,
            library::list_forwards,
            library::save_forward,
            library::delete_forward,
            forwarding::start_forward,
            forwarding::stop_forward,
            forwarding::forward_statuses,
            settings::get_settings,
            settings::set_sync_server,
            settings::check_sync_server,
            files::sftp_open,
            files::sftp_close,
            files::sftp_list,
            files::sftp_mkdir,
            files::sftp_rename,
            files::sftp_remove,
            files::local_home,
            files::local_list,
            files::local_mkdir,
            files::local_rename,
            files::transfer,
            files::cancel_transfer,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
