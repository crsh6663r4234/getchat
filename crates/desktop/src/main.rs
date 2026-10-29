#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod crypto;
mod crypto_attachments;

use serde::Serialize;
use std::{
    fs,
    path::PathBuf,
    sync::{Arc, Mutex},
};
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    webview::{PermissionKind, PermissionResponse},
    Emitter, Manager, State, WindowEvent,
};
use tauri_plugin_deep_link::DeepLinkExt;
use tauri_plugin_notification::NotificationExt;
use tauri_plugin_updater::UpdaterExt;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct UpdateStatus {
    configured: bool,
    current_version: &'static str,
    available: bool,
    version: Option<String>,
    notes: Option<String>,
}

fn update_endpoint() -> Option<&'static str> {
    option_env!("GHOSTCHAT_UPDATE_ENDPOINT").filter(|endpoint| !endpoint.trim().is_empty())
}

fn make_updater(app: &tauri::AppHandle) -> Result<tauri_plugin_updater::Updater, String> {
    let endpoint = update_endpoint()
        .ok_or_else(|| "The GitHub Releases feed is not configured for this build.".to_string())?;
    let endpoint = endpoint
        .parse()
        .map_err(|error| format!("Invalid update feed URL: {error}"))?;
    app.updater_builder()
        .endpoints(vec![endpoint])
        .map_err(|error| error.to_string())?
        .build()
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn check_for_update(app: tauri::AppHandle) -> Result<UpdateStatus, String> {
    if update_endpoint().is_none() {
        return Ok(UpdateStatus {
            configured: false,
            current_version: env!("CARGO_PKG_VERSION"),
            available: false,
            version: None,
            notes: None,
        });
    }

    let update = make_updater(&app)?
        .check()
        .await
        .map_err(|error| error.to_string())?;
    Ok(UpdateStatus {
        configured: true,
        current_version: env!("CARGO_PKG_VERSION"),
        available: update.is_some(),
        version: update.as_ref().map(|update| update.version.clone()),
        notes: update.and_then(|update| update.body),
    })
}

#[tauri::command]
async fn install_update(app: tauri::AppHandle, expected_version: String) -> Result<(), String> {
    let update = make_updater(&app)?
        .check()
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "There is no update available right now.".to_string())?;

    if update.version != expected_version {
        return Err(
            "A newer release was published. Check for updates again before installing.".into(),
        );
    }

    let progress_app = app.clone();
    let mut downloaded = 0usize;
    update
        .download_and_install(
            move |chunk_length, content_length| {
                downloaded += chunk_length;
                if let Some(total) = content_length.filter(|total| *total > 0) {
                    let percent = ((downloaded as f64 / total as f64) * 100.0)
                        .floor()
                        .min(100.0) as u8;
                    let _ = progress_app.emit("ghostchat:update-progress", percent);
                }
            },
            || {},
        )
        .await
        .map_err(|error| error.to_string())?;
    app.restart();
}

#[tauri::command]
fn show_desktop_notification(
    app: tauri::AppHandle,
    title: String,
    body: String,
) -> Result<(), String> {
    let title: String = title.chars().take(80).collect();
    let body: String = body.chars().take(240).collect();
    if title.is_empty() || body.is_empty() {
        return Err("A notification title and message are required.".into());
    }
    app.notification()
        .builder()
        .title(title)
        .body(body)
        .show()
        .map_err(|error| error.to_string())
}

struct DesktopState {
    api_base: String,
    crypto_db: sqlx::SqlitePool,
}

#[derive(Default)]
struct PendingGroupInvites(Arc<Mutex<Vec<String>>>);

fn group_invite_code_from_deep_link(url: &str) -> Option<String> {
    let code = url.strip_prefix("ghostchat://invite/")?;
    if code.len() != 43
        || !code
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        return None;
    }
    Some(code.to_string())
}

fn queue_group_invite_urls(queue: &Arc<Mutex<Vec<String>>>, urls: impl Iterator<Item = String>) {
    let Ok(mut pending) = queue.lock() else {
        return;
    };
    for url in urls {
        if let Some(code) = group_invite_code_from_deep_link(&url) {
            if pending.len() == 8 {
                pending.remove(0);
            }
            pending.push(code);
        }
    }
}

#[tauri::command]
fn take_pending_group_invites(pending: State<'_, PendingGroupInvites>) -> Vec<String> {
    pending
        .0
        .lock()
        .map(|mut codes| std::mem::take(&mut *codes))
        .unwrap_or_default()
}

#[tauri::command]
fn api_base(state: State<'_, DesktopState>) -> String {
    state.api_base.clone()
}

#[tauri::command]
async fn crypto_prepare_device(
    state: State<'_, DesktopState>,
    user_id: String,
) -> Result<crypto::DevicePreparation, String> {
    crypto::prepare_device(&state.crypto_db, &user_id).await
}

#[tauri::command]
async fn crypto_inspect_key_package(
    state: State<'_, DesktopState>,
    peer_user_id: String,
    peer_device_id: String,
    payload: String,
) -> Result<crypto::KeyPackageInspection, String> {
    crypto::inspect_key_package(&state.crypto_db, &peer_user_id, &peer_device_id, &payload).await
}

#[tauri::command]
async fn crypto_create_direct_group(
    state: State<'_, DesktopState>,
    user_id: String,
    conversation_id: String,
    peer_user_id: String,
    peer_device_id: String,
    peer_package_payload: String,
) -> Result<crypto::DirectGroupStart, String> {
    crypto::create_direct_group(
        &state.crypto_db,
        &user_id,
        &conversation_id,
        &peer_user_id,
        &peer_device_id,
        &peer_package_payload,
    )
    .await
}

#[tauri::command]
async fn crypto_create_group(
    state: State<'_, DesktopState>,
    user_id: String,
    conversation_id: String,
    peers: Vec<crypto::GroupPeerKeyPackage>,
) -> Result<crypto::DirectGroupStart, String> {
    crypto::create_group(&state.crypto_db, &user_id, &conversation_id, &peers).await
}

#[tauri::command]
async fn crypto_get_pending_group_membership_update(
    state: State<'_, DesktopState>,
    user_id: String,
    conversation_id: String,
) -> Result<Option<crypto::GroupMembershipUpdate>, String> {
    crypto::get_pending_group_membership_update(&state.crypto_db, &user_id, &conversation_id).await
}

#[tauri::command]
async fn crypto_prepare_group_membership_update(
    state: State<'_, DesktopState>,
    user_id: String,
    conversation_id: String,
    expected_member_ids: Vec<String>,
    peers: Vec<crypto::GroupPeerKeyPackage>,
) -> Result<crypto::GroupMembershipUpdate, String> {
    crypto::prepare_group_membership_update(
        &state.crypto_db,
        &user_id,
        &conversation_id,
        &expected_member_ids,
        &peers,
    )
    .await
}

#[tauri::command]
async fn crypto_prepare_group_removal_update(
    state: State<'_, DesktopState>,
    user_id: String,
    conversation_id: String,
    expected_member_ids: Vec<String>,
    removed_member_ids: Vec<String>,
) -> Result<crypto::GroupMembershipUpdate, String> {
    crypto::prepare_group_removal_update(
        &state.crypto_db,
        &user_id,
        &conversation_id,
        &expected_member_ids,
        &removed_member_ids,
    )
    .await
}

#[tauri::command]
async fn crypto_prepare_group_leave_proposal(
    state: State<'_, DesktopState>,
    user_id: String,
    conversation_id: String,
    expected_member_ids: Vec<String>,
) -> Result<crypto::GroupMembershipUpdate, String> {
    crypto::prepare_group_leave_proposal(
        &state.crypto_db,
        &user_id,
        &conversation_id,
        &expected_member_ids,
    )
    .await
}

#[tauri::command]
async fn crypto_prepare_group_leave_commit(
    state: State<'_, DesktopState>,
    user_id: String,
    conversation_id: String,
    operation_id: String,
    leaving_user_id: String,
    expected_member_ids: Vec<String>,
) -> Result<crypto::GroupMembershipUpdate, String> {
    crypto::prepare_group_leave_commit(
        &state.crypto_db,
        &user_id,
        &conversation_id,
        &operation_id,
        &leaving_user_id,
        &expected_member_ids,
    )
    .await
}

#[tauri::command]
async fn crypto_forget_group_state(
    state: State<'_, DesktopState>,
    user_id: String,
    conversation_id: String,
) -> Result<(), String> {
    crypto::forget_group_state(&state.crypto_db, &user_id, &conversation_id).await
}

#[tauri::command]
async fn crypto_finalize_group_membership_update(
    state: State<'_, DesktopState>,
    user_id: String,
    conversation_id: String,
    operation_id: String,
) -> Result<String, String> {
    crypto::finalize_group_membership_update(
        &state.crypto_db,
        &user_id,
        &conversation_id,
        &operation_id,
    )
    .await
}

#[tauri::command]
async fn crypto_get_pending_bootstrap(
    state: State<'_, DesktopState>,
    user_id: String,
    conversation_id: String,
) -> Result<Option<crypto::DirectGroupStart>, String> {
    crypto::get_pending_bootstrap(&state.crypto_db, &user_id, &conversation_id).await
}

#[tauri::command]
async fn crypto_clear_pending_bootstrap(
    state: State<'_, DesktopState>,
    user_id: String,
    conversation_id: String,
) -> Result<(), String> {
    crypto::clear_pending_bootstrap(&state.crypto_db, &user_id, &conversation_id).await
}

#[tauri::command]
async fn crypto_encrypt_text(
    state: State<'_, DesktopState>,
    user_id: String,
    conversation_id: String,
    client_message_id: String,
    content: String,
) -> Result<crypto::EncryptedApplicationMessage, String> {
    crypto::encrypt_text(
        &state.crypto_db,
        &user_id,
        &conversation_id,
        &client_message_id,
        &content,
    )
    .await
}

#[tauri::command]
async fn crypto_get_cached_sent_text(
    state: State<'_, DesktopState>,
    client_message_id: String,
) -> Result<Option<String>, String> {
    crypto::get_cached_sent_text(&state.crypto_db, &client_message_id).await
}

#[tauri::command]
async fn crypto_process_event(
    state: State<'_, DesktopState>,
    user_id: String,
    conversation_id: String,
    sender_id: String,
    client_message_id: Option<String>,
    event_sequence: i64,
    conversation_is_group: bool,
    expected_member_ids: Vec<String>,
    payload: String,
) -> Result<crypto::CryptoEventResult, String> {
    crypto::process_crypto_event(
        &state.crypto_db,
        &user_id,
        &conversation_id,
        &sender_id,
        client_message_id.as_deref(),
        event_sequence,
        conversation_is_group,
        &expected_member_ids,
        &payload,
    )
    .await
}

#[tauri::command]
async fn crypto_get_event_cursor(
    state: State<'_, DesktopState>,
    user_id: String,
    conversation_id: String,
) -> Result<Option<i64>, String> {
    crypto::get_crypto_event_cursor(&state.crypto_db, &user_id, &conversation_id).await
}

#[tauri::command]
async fn crypto_advance_event_cursor(
    state: State<'_, DesktopState>,
    user_id: String,
    conversation_id: String,
    sequence: i64,
) -> Result<i64, String> {
    crypto::advance_crypto_event_cursor(&state.crypto_db, &user_id, &conversation_id, sequence)
        .await
}

#[tauri::command]
async fn crypto_upload_encrypted_attachment(
    app: tauri::AppHandle,
    conversation_id: String,
    server_base: String,
    token: String,
) -> Result<Option<crypto_attachments::EncryptedAttachmentManifest>, String> {
    crypto_attachments::upload(app, conversation_id, server_base, token).await
}

#[tauri::command]
async fn crypto_download_encrypted_attachment(
    app: tauri::AppHandle,
    server_base: String,
    token: String,
    manifest: crypto_attachments::EncryptedAttachmentManifest,
) -> Result<Option<String>, String> {
    crypto_attachments::download(app, server_base, token, manifest).await
}

#[tauri::command]
async fn crypto_has_direct_group(
    state: State<'_, DesktopState>,
    user_id: String,
    conversation_id: String,
) -> Result<bool, String> {
    crypto::has_direct_group(&state.crypto_db, &user_id, &conversation_id).await
}

#[tauri::command]
async fn crypto_direct_peer_fingerprint(
    state: State<'_, DesktopState>,
    user_id: String,
    peer_user_id: String,
    conversation_id: String,
) -> Result<Option<String>, String> {
    crypto::direct_peer_fingerprint(&state.crypto_db, &user_id, &peer_user_id, &conversation_id)
        .await
}

#[tauri::command]
async fn crypto_group_fingerprint(
    state: State<'_, DesktopState>,
    user_id: String,
    conversation_id: String,
) -> Result<Option<String>, String> {
    crypto::group_fingerprint(&state.crypto_db, &user_id, &conversation_id).await
}

#[tauri::command]
fn credential_get(server_key: String) -> Result<Option<String>, String> {
    windows_credential::get(&server_key)
}

#[tauri::command]
fn credential_set(server_key: String, token: String) -> Result<(), String> {
    windows_credential::set(&server_key, &token)
}

#[tauri::command]
fn credential_delete(server_key: String) -> Result<(), String> {
    windows_credential::delete(&server_key)
}

#[tauri::command]
fn credential_get_pending_recovery(server_key: String) -> Result<Option<String>, String> {
    windows_credential::get_pending_recovery(&server_key)
}

#[tauri::command]
fn credential_set_pending_recovery(server_key: String, token: String) -> Result<(), String> {
    windows_credential::set_pending_recovery(&server_key, &token)
}

#[tauri::command]
fn credential_delete_pending_recovery(server_key: String) -> Result<(), String> {
    windows_credential::delete_pending_recovery(&server_key)
}

#[tauri::command]
fn credential_get_recovery(server_key: String) -> Result<Option<String>, String> {
    windows_credential::get_recovery(&server_key)
}

#[tauri::command]
fn credential_set_recovery(server_key: String, token: String) -> Result<(), String> {
    windows_credential::set_recovery(&server_key, &token)
}

#[tauri::command]
fn credential_delete_recovery(server_key: String) -> Result<(), String> {
    windows_credential::delete_recovery(&server_key)
}

fn app_database_path(app: &tauri::AppHandle) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let directory = app.path().app_data_dir()?;
    fs::create_dir_all(&directory)?;
    Ok(directory.join("ghostchat.sqlite"))
}

#[tauri::command]
fn get_start_at_login() -> Result<bool, String> {
    #[cfg(windows)]
    {
        use winreg::{enums::HKEY_CURRENT_USER, RegKey};

        let current_user = RegKey::predef(HKEY_CURRENT_USER);
        let run_key =
            match current_user.open_subkey("Software\\Microsoft\\Windows\\CurrentVersion\\Run") {
                Ok(key) => key,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
                Err(error) => return Err(error.to_string()),
            };
        match run_key.get_value::<String, _>("GhostChat") {
            Ok(_) => Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error.to_string()),
        }
    }
    #[cfg(not(windows))]
    {
        Ok(false)
    }
}

#[tauri::command]
fn set_start_at_login(enabled: bool) -> Result<(), String> {
    #[cfg(windows)]
    {
        use winreg::{enums::HKEY_CURRENT_USER, RegKey};

        let current_user = RegKey::predef(HKEY_CURRENT_USER);
        let (run_key, _) = current_user
            .create_subkey("Software\\Microsoft\\Windows\\CurrentVersion\\Run")
            .map_err(|error| error.to_string())?;
        if enabled {
            let executable = std::env::current_exe().map_err(|error| error.to_string())?;
            let command = format!("\"{}\"", executable.display());
            run_key
                .set_value("GhostChat", &command)
                .map_err(|error| error.to_string())
        } else {
            match run_key.delete_value("GhostChat") {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(error.to_string()),
            }
        }
    }
    #[cfg(not(windows))]
    {
        let _ = enabled;
        Err("Starting GhostChat with Windows is only available on Windows.".to_string())
    }
}

fn main() {
    #[cfg(all(windows, not(debug_assertions)))]
    install_panic_dialog();

    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            show_main_window(app);
        }))
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .on_permission_request(|_, permission| match permission {
            PermissionKind::Microphone | PermissionKind::DisplayCapture => {
                PermissionResponse::Allow
            }
            _ => PermissionResponse::Default,
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .setup(|app| {
            app.manage(PendingGroupInvites::default());
            #[cfg(any(target_os = "linux", all(debug_assertions, windows)))]
            app.deep_link().register_all()?;
            let pending_group_invites = app.state::<PendingGroupInvites>().0.clone();
            if let Some(urls) = app.deep_link().get_current()? {
                queue_group_invite_urls(
                    &pending_group_invites,
                    urls.into_iter().map(|url| url.to_string()),
                );
            }
            let app_handle = app.handle().clone();
            app.deep_link().on_open_url(move |event| {
                queue_group_invite_urls(
                    &pending_group_invites,
                    event.urls().iter().map(ToString::to_string),
                );
                let _ = app_handle.emit("ghostchat:group-invite-ready", ());
            });
            let open_item = MenuItem::with_id(app, "open", "Open GhostChat", true, None::<&str>)?;
            let quit_item = MenuItem::with_id(app, "quit", "Quit GhostChat", true, None::<&str>)?;
            let tray_menu = Menu::with_items(app, &[&open_item, &quit_item])?;
            let tray_icon = app
                .default_window_icon()
                .cloned()
                .ok_or_else(|| std::io::Error::other("GhostChat tray icon is missing"))?;
            TrayIconBuilder::new()
                .icon(tray_icon)
                .tooltip("GhostChat")
                .menu(&tray_menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id().as_ref() {
                    "open" => show_main_window(app),
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if matches!(
                        event,
                        TrayIconEvent::Click {
                            button: MouseButton::Left,
                            button_state: MouseButtonState::Up,
                            ..
                        } | TrayIconEvent::DoubleClick {
                            button: MouseButton::Left,
                            ..
                        }
                    ) {
                        show_main_window(tray.app_handle());
                    }
                })
                .build(app)?;
            let database_path = app_database_path(app.handle())?;
            let attachment_dir = database_path
                .parent()
                .ok_or_else(|| std::io::Error::other("GhostChat data directory is missing"))?
                .join("attachments");
            let path = database_path.to_string_lossy().to_string();
            let (api, crypto_db) = tauri::async_runtime::block_on(async move {
                let db = ghostchat_server::open_database(&path)
                    .await
                    .map_err(|error| error.to_string())?;
                let crypto_db = db.clone();
                let state =
                    ghostchat_server::build_state_with_attachment_dir(db, attachment_dir).await;
                let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                    .await
                    .map_err(|error| error.to_string())?;
                let address = listener.local_addr().map_err(|error| error.to_string())?;
                tauri::async_runtime::spawn(async move {
                    if let Err(error) = ghostchat_server::serve(listener, state).await {
                        eprintln!("GhostChat local service stopped: {error}");
                    }
                });
                Ok::<(String, sqlx::SqlitePool), String>((format!("http://{address}"), crypto_db))
            })
            .map_err(std::io::Error::other)?;
            app.manage(DesktopState {
                api_base: api,
                crypto_db,
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            api_base,
            credential_get,
            credential_set,
            credential_delete,
            credential_get_pending_recovery,
            credential_set_pending_recovery,
            credential_delete_pending_recovery,
            credential_get_recovery,
            credential_set_recovery,
            credential_delete_recovery,
            crypto_prepare_device,
            crypto_inspect_key_package,
            crypto_create_direct_group,
            crypto_create_group,
            crypto_get_pending_group_membership_update,
            crypto_prepare_group_membership_update,
            crypto_prepare_group_removal_update,
            crypto_prepare_group_leave_proposal,
            crypto_prepare_group_leave_commit,
            crypto_forget_group_state,
            crypto_finalize_group_membership_update,
            crypto_get_pending_bootstrap,
            crypto_clear_pending_bootstrap,
            crypto_encrypt_text,
            crypto_get_cached_sent_text,
            crypto_process_event,
            crypto_get_event_cursor,
            crypto_advance_event_cursor,
            crypto_upload_encrypted_attachment,
            crypto_download_encrypted_attachment,
            crypto_has_direct_group,
            crypto_direct_peer_fingerprint,
            crypto_group_fingerprint,
            get_start_at_login,
            set_start_at_login,
            take_pending_group_invites,
            check_for_update,
            install_update,
            show_desktop_notification
        ])
        .run(tauri::generate_context!())
        .expect("GhostChat failed to start");
}

fn show_main_window(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

#[cfg(all(windows, not(debug_assertions)))]
fn install_panic_dialog() {
    use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};

    std::panic::set_hook(Box::new(|panic| {
        let title: Vec<u16> = "GhostChat"
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let message: Vec<u16> = format!("GhostChat could not start.\n\n{panic}")
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        unsafe {
            MessageBoxW(
                std::ptr::null_mut(),
                message.as_ptr(),
                title.as_ptr(),
                MB_OK | MB_ICONERROR,
            );
        }
    }));
}

#[cfg(windows)]
mod windows_credential {
    #[cfg(not(test))]
    use rand::RngCore;
    use std::{ptr, slice};
    use windows_sys::Win32::Security::Credentials::{
        CredDeleteW, CredFree, CredReadW, CredWriteW, CREDENTIALW, CRED_PERSIST_LOCAL_MACHINE,
        CRED_TYPE_GENERIC,
    };
    use zeroize::Zeroizing;

    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(std::iter::once(0)).collect()
    }
    fn target(server_key: &str, kind: &str) -> Result<Vec<u16>, String> {
        if server_key.len() != 64 || !server_key.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err("Invalid server credential key.".into());
        }
        Ok(wide(&format!("GhostChat.{kind}.{server_key}")))
    }

    fn get_at(mut target: Vec<u16>) -> Result<Option<String>, String> {
        let mut credential: *mut CREDENTIALW = ptr::null_mut();
        let success =
            unsafe { CredReadW(target.as_mut_ptr(), CRED_TYPE_GENERIC, 0, &mut credential) };
        if success == 0 {
            return Ok(None);
        }
        let result = unsafe {
            let record = &*credential;
            let bytes =
                slice::from_raw_parts(record.CredentialBlob, record.CredentialBlobSize as usize);
            String::from_utf8(bytes.to_vec())
                .map_err(|_| "The saved session is invalid.".to_string())
        };
        unsafe {
            CredFree(credential.cast());
        }
        result.map(Some)
    }

    pub fn get(server_key: &str) -> Result<Option<String>, String> {
        get_at(target(server_key, "Session")?)
    }

    pub fn get_pending_recovery(server_key: &str) -> Result<Option<String>, String> {
        get_at(target(server_key, "RecoveryPending")?)
    }

    pub fn get_recovery(server_key: &str) -> Result<Option<String>, String> {
        get_at(target(server_key, "Recovery")?)
    }

    fn set_at(mut target: Vec<u16>, token: &str, username: &str) -> Result<(), String> {
        if token.is_empty() || token.len() > 128 {
            return Err("The credential value is invalid.".into());
        }
        let mut username = wide(username);
        let mut bytes = Zeroizing::new(token.as_bytes().to_vec());
        let credential = CREDENTIALW {
            Flags: 0,
            Type: CRED_TYPE_GENERIC,
            TargetName: target.as_mut_ptr(),
            Comment: ptr::null_mut(),
            LastWritten: Default::default(),
            CredentialBlobSize: bytes.len() as u32,
            CredentialBlob: bytes.as_mut_ptr(),
            Persist: CRED_PERSIST_LOCAL_MACHINE,
            AttributeCount: 0,
            Attributes: ptr::null_mut(),
            TargetAlias: ptr::null_mut(),
            UserName: username.as_mut_ptr(),
        };
        let success = unsafe { CredWriteW(&credential, 0) };
        if success == 0 {
            Err("Windows could not store the credential in Credential Manager.".into())
        } else {
            Ok(())
        }
    }

    pub fn set(server_key: &str, token: &str) -> Result<(), String> {
        set_at(target(server_key, "Session")?, token, "GhostChat")
    }

    pub fn set_pending_recovery(server_key: &str, token: &str) -> Result<(), String> {
        set_at(
            target(server_key, "RecoveryPending")?,
            token,
            "GhostChat recovery backup",
        )
    }

    pub fn set_recovery(server_key: &str, token: &str) -> Result<(), String> {
        set_at(
            target(server_key, "Recovery")?,
            token,
            "GhostChat recovery key",
        )
    }

    #[cfg(not(test))]
    pub fn get_or_create_crypto_storage_key() -> Result<Zeroizing<[u8; 32]>, String> {
        let key_target = wide("GhostChat.MlsStorageKey");
        if let Some(existing) = get_at(key_target.clone())?.map(Zeroizing::new) {
            if existing.len() != 64 || !existing.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                return Err("The saved local MLS storage key is invalid; encrypted data was left untouched.".into());
            }
            let mut key = Zeroizing::new([0u8; 32]);
            for (index, byte) in key.iter_mut().enumerate() {
                let start = index * 2;
                *byte = u8::from_str_radix(&existing[start..start + 2], 16)
                    .map_err(|_| "The saved local MLS storage key is invalid; encrypted data was left untouched.".to_string())?;
            }
            return Ok(key);
        }

        let mut key = Zeroizing::new([0u8; 32]);
        rand::rngs::OsRng.fill_bytes(&mut *key);
        let encoded = Zeroizing::new(
            key.iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>(),
        );
        set_at(key_target, &encoded, "GhostChat MLS storage key")?;
        Ok(key)
    }

    fn delete_at(mut target: Vec<u16>) -> Result<(), String> {
        let _ = unsafe { CredDeleteW(target.as_mut_ptr(), CRED_TYPE_GENERIC, 0) };
        Ok(())
    }

    pub fn delete(server_key: &str) -> Result<(), String> {
        delete_at(target(server_key, "Session")?)
    }

    pub fn delete_pending_recovery(server_key: &str) -> Result<(), String> {
        delete_at(target(server_key, "RecoveryPending")?)
    }

    pub fn delete_recovery(server_key: &str) -> Result<(), String> {
        delete_at(target(server_key, "Recovery")?)
    }
}

#[cfg(not(windows))]
mod windows_credential {
    pub fn get(_: &str) -> Result<Option<String>, String> {
        Ok(None)
    }
    pub fn set(_: &str, _: &str) -> Result<(), String> {
        Err("Secure session storage is only available on Windows.".into())
    }
    pub fn delete(_: &str) -> Result<(), String> {
        Ok(())
    }
    pub fn get_pending_recovery(_: &str) -> Result<Option<String>, String> {
        Ok(None)
    }
    pub fn get_recovery(_: &str) -> Result<Option<String>, String> {
        Ok(None)
    }
    pub fn set_pending_recovery(_: &str, _: &str) -> Result<(), String> {
        Err("Secure recovery storage is only available on Windows.".into())
    }
    pub fn set_recovery(_: &str, _: &str) -> Result<(), String> {
        Err("Secure recovery storage is only available on Windows.".into())
    }
    #[cfg(not(test))]
    pub fn get_or_create_crypto_storage_key() -> Result<zeroize::Zeroizing<[u8; 32]>, String> {
        Err("Encrypted MLS device storage currently requires Windows Credential Manager.".into())
    }
    pub fn delete_pending_recovery(_: &str) -> Result<(), String> {
        Ok(())
    }
    pub fn delete_recovery(_: &str) -> Result<(), String> {
        Ok(())
    }
}
