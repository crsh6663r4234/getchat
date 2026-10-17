use serde::Serialize;
use std::{fs, path::PathBuf};
use tauri::{Emitter, Manager, State};
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

struct DesktopState {
    api_base: String,
}

#[tauri::command]
fn api_base(state: State<'_, DesktopState>) -> String {
    state.api_base.clone()
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

fn app_database_path(app: &tauri::AppHandle) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let directory = app.path().app_data_dir()?;
    fs::create_dir_all(&directory)?;
    Ok(directory.join("ghostchat.sqlite"))
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(|app| {
            let path = app_database_path(app.handle())?;
            let path = path.to_string_lossy().to_string();
            let api = tauri::async_runtime::block_on(async move {
                let db = ghostchat_server::open_database(&path)
                    .await
                    .map_err(|error| error.to_string())?;
                let state = ghostchat_server::build_state(db).await;
                let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                    .await
                    .map_err(|error| error.to_string())?;
                let address = listener.local_addr().map_err(|error| error.to_string())?;
                tauri::async_runtime::spawn(async move {
                    if let Err(error) = ghostchat_server::serve(listener, state).await {
                        eprintln!("GhostChat local service stopped: {error}");
                    }
                });
                Ok::<String, String>(format!("http://{address}"))
            })
            .map_err(std::io::Error::other)?;
            app.manage(DesktopState { api_base: api });
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
            check_for_update,
            install_update
        ])
        .run(tauri::generate_context!())
        .expect("GhostChat failed to start");
}

#[cfg(windows)]
mod windows_credential {
    use std::{ptr, slice};
    use windows_sys::Win32::Security::Credentials::{
        CredDeleteW, CredFree, CredReadW, CredWriteW, CREDENTIALW, CRED_PERSIST_LOCAL_MACHINE,
        CRED_TYPE_GENERIC,
    };

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

    fn set_at(mut target: Vec<u16>, token: &str, username: &str) -> Result<(), String> {
        if token.is_empty() || token.len() > 128 {
            return Err("The credential value is invalid.".into());
        }
        let mut username = wide(username);
        let mut bytes = token.as_bytes().to_vec();
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
    pub fn set_pending_recovery(_: &str, _: &str) -> Result<(), String> {
        Err("Secure recovery storage is only available on Windows.".into())
    }
    pub fn delete_pending_recovery(_: &str) -> Result<(), String> {
        Ok(())
    }
}
