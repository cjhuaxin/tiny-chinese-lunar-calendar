use std::fs;
use std::path::PathBuf;

use crate::auto_return;
use crate::models::AppSettings;

pub const BUNDLE_ID: &str = "com.cjhuaxin.tclc";
pub const APP_NAME: &str = "小小万年历";

pub fn app_data_dir() -> Result<PathBuf, String> {
    let dir = dirs::data_dir()
        .ok_or_else(|| "cannot resolve application data directory".to_string())?
        .join(BUNDLE_ID);
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

fn settings_path() -> Result<PathBuf, String> {
    Ok(app_data_dir()?.join("settings.json"))
}

pub fn load_settings() -> AppSettings {
    let Ok(path) = settings_path() else {
        return AppSettings::default();
    };
    if !path.exists() {
        return AppSettings::default();
    }
    let mut settings = fs::read_to_string(&path)
        .ok()
        .and_then(|content| serde_json::from_str::<AppSettings>(&content).ok())
        .unwrap_or_default();
    settings.auto_return_minutes =
        auto_return::normalize_auto_return_minutes(settings.auto_return_minutes);
    settings
}

pub fn save_settings(settings: &AppSettings) -> Result<(), String> {
    let path = settings_path()?;
    let content = serde_json::to_string_pretty(settings).map_err(|e| e.to_string())?;
    fs::write(path, content).map_err(|e| e.to_string())
}

pub fn sync_launch_at_login(enabled: bool) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;

    // When running from a bundle, register the .app; otherwise register the binary.
    let app_path = exe
        .ancestors()
        .find(|p| p.extension().is_some_and(|ext| ext == "app"))
        .map(|p| p.to_path_buf())
        .unwrap_or(exe);

    let auto = auto_launch::AutoLaunchBuilder::new()
        .set_app_name(APP_NAME)
        .set_app_path(&app_path.to_string_lossy())
        .build()
        .map_err(|e| e.to_string())?;

    let is_enabled = auto.is_enabled().map_err(|e| e.to_string())?;
    if enabled && !is_enabled {
        auto.enable().map_err(|e| e.to_string())?;
    } else if !enabled && is_enabled {
        auto.disable().map_err(|e| e.to_string())?;
    }
    Ok(())
}
