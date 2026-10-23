//! Tauri desktop shell and commands.

use std::sync::Arc;

use kittentts_rs::text::TtsSettings;
use tauri::{Emitter, Manager, State};

use crate::config;
use crate::core::{AppConfigUpdate, AppStateDto, Core};

type AppCore<'a> = State<'a, Arc<Core>>;

#[tauri::command]
fn get_state(core: AppCore<'_>) -> AppStateDto {
    core.state()
}

#[tauri::command]
async fn connect(core: AppCore<'_>, username: String) -> Result<AppStateDto, String> {
    let core = core.inner().clone();
    core.connect(&username).await?;
    Ok(core.state())
}

#[tauri::command]
fn disconnect(core: AppCore<'_>) -> AppStateDto {
    core.disconnect();
    core.state()
}

#[tauri::command]
fn update_settings(core: AppCore<'_>, settings: TtsSettings) -> Result<AppStateDto, String> {
    core.update_settings(settings)?;
    Ok(core.state())
}

#[tauri::command]
fn update_app_config(core: AppCore<'_>, config: AppConfigUpdate) -> Result<AppStateDto, String> {
    core.update_app_config(config)?;
    Ok(core.state())
}

#[tauri::command]
fn speak(core: AppCore<'_>, text: String) -> Result<AppStateDto, String> {
    core.speak(&text)?;
    Ok(core.state())
}

#[tauri::command]
fn skip(core: AppCore<'_>) -> AppStateDto {
    core.skip();
    core.state()
}

#[tauri::command]
fn clear_queue(core: AppCore<'_>) -> AppStateDto {
    core.clear_queue();
    core.state()
}

#[tauri::command]
fn set_paused(core: AppCore<'_>, paused: bool) -> AppStateDto {
    core.set_paused(paused);
    core.state()
}

#[tauri::command]
fn ack(core: AppCore<'_>, seq: u64) {
    core.ack(seq);
}

pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let config_path = app
                .path()
                .app_data_dir()
                .map(|dir| dir.join("config.json"))
                .unwrap_or_else(|_| config::default_config_path());
            let core = Core::new(config_path, None, false).map_err(std::io::Error::other)?;

            let synth = core.clone();
            tauri::async_runtime::spawn(async move { synth.run_synth_loop().await });

            let mut events = core.subscribe();
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                loop {
                    match events.recv().await {
                        Ok(event) => {
                            let _ = handle.emit("yapper://event", &event);
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
            });

            if core.auto_connect() {
                let username = core.username();
                if !username.is_empty() {
                    let core = core.clone();
                    tauri::async_runtime::spawn(async move {
                        tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
                        if let Err(error) = core.connect(&username).await {
                            core.log("error", error);
                        }
                    });
                }
            }

            app.manage(core);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_state,
            connect,
            disconnect,
            update_settings,
            update_app_config,
            speak,
            skip,
            clear_queue,
            set_paused,
            ack,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Yapper");
}
