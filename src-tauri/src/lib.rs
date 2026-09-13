use tauri::Manager;

#[cfg(desktop)]
mod mcp;

#[tauri::command]
fn show_overlay(app: tauri::AppHandle) -> Result<(), String> {
    if let Some(window) = app.get_webview_window("overlay") {
        window.show().map_err(|e| e.to_string())?;
        window.set_focus().map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
fn toggle_overlay(app: tauri::AppHandle) -> Result<bool, String> {
    if let Some(window) = app.get_webview_window("overlay") {
        let visible = window.is_visible().map_err(|e| e.to_string())?;
        if visible {
            window.hide().map_err(|e| e.to_string())?;
            Ok(false)
        } else {
            window.show().map_err(|e| e.to_string())?;
            window.set_focus().map_err(|e| e.to_string())?;
            Ok(true)
        }
    } else {
        Err("Overlay window not found".to_string())
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_opener::init());

    // Updates ship as signed installers, so the updater is desktop-only.
    #[cfg(desktop)]
    let builder = builder
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init());

    #[cfg(desktop)]
    let builder = builder
        .manage(mcp::McpState::new())
        .invoke_handler(tauri::generate_handler![
            show_overlay,
            toggle_overlay,
            mcp::mcp_reply,
            mcp::mcp_list_sessions,
            mcp::mcp_enqueue_user,
            mcp::mcp_host_name,
            mcp::mcp_set_instructions
        ])
        .setup(|app| {
            let state = app.state::<mcp::McpState>().inner().clone();
            mcp::start(app.handle().clone(), state);
            Ok(())
        });

    #[cfg(not(desktop))]
    let builder = builder.invoke_handler(tauri::generate_handler![show_overlay, toggle_overlay]);

    builder
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
