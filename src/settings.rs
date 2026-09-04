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

/// Sync the OS login-item with the persisted preference.
///
/// `prompt_user` should be true only when the user just toggled the setting, so
/// we can open System Settings if macOS needs approval. Startup sync stays quiet:
/// if the toggle is on but the login item is missing or still the old broken
/// LaunchAgent, it is repaired without a dialog.
pub fn sync_launch_at_login(enabled: bool, prompt_user: bool) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        return macos::sync(enabled, prompt_user);
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = prompt_user;
        sync_launch_at_login_generic(enabled)
    }
}

#[cfg(not(target_os = "macos"))]
fn sync_launch_at_login_generic(enabled: bool) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let auto = auto_launch::AutoLaunchBuilder::new()
        .set_app_name(APP_NAME)
        .set_app_path(&exe.to_string_lossy())
        .build()
        .map_err(|e| e.to_string())?;
    apply_auto_launch(&auto, enabled)
}

fn apply_auto_launch(auto: &auto_launch::AutoLaunch, enabled: bool) -> Result<(), String> {
    let is_enabled = auto.is_enabled().map_err(|e| e.to_string())?;
    if enabled && !is_enabled {
        auto.enable().map_err(|e| e.to_string())?;
    } else if !enabled && is_enabled {
        auto.disable().map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[cfg(target_os = "macos")]
mod macos {
    use super::{apply_auto_launch, APP_NAME, BUNDLE_ID};
    use smappservice_rs::{AppService, ServiceManagementError, ServiceStatus, ServiceType};
    use std::fs;
    use std::path::PathBuf;
    use std::process::Command;

    const APPROVAL_MSG: &str = "请在「系统设置 → 通用 → 登录项」中允许小小万年历开机启动";

    pub(super) fn sync(enabled: bool, prompt_user: bool) -> Result<(), String> {
        // The previous LaunchAgent pointed ProgramArguments at the .app bundle
        // directory (not an executable). launchd then aborted with EX_CONFIG.
        remove_launch_agent(APP_NAME);

        if !enabled {
            let _ = unregister_smappservice();
            remove_launch_agent(BUNDLE_ID);
            return Ok(());
        }

        if bundled_app_path().is_none() {
            // cargo run: persist the flag; the real .app registers on next launch.
            return Ok(());
        }

        // Preference is on. If SMAppService is already live, drop the fallback
        // agent. Otherwise (re)register, and if that still is not Enabled —
        // including "needs approval" on a quiet startup — write a working
        // LaunchAgent so a checked toggle actually starts the app at login.
        if smappservice_enabled() {
            remove_launch_agent(BUNDLE_ID);
            return Ok(());
        }

        match register_smappservice(prompt_user) {
            Ok(()) => {
                remove_launch_agent(BUNDLE_ID);
                Ok(())
            }
            Err(err) if err == APPROVAL_MSG => Err(err),
            Err(err) => {
                eprintln!("SMAppService register failed: {err}; falling back to LaunchAgent");
                sync_launch_agent(true)
            }
        }
    }

    fn smappservice_enabled() -> bool {
        AppService::new(ServiceType::MainApp).status() == ServiceStatus::Enabled
    }

    fn bundled_app_path() -> Option<PathBuf> {
        std::env::current_exe().ok()?.ancestors().find_map(|p| {
            p.extension()
                .is_some_and(|ext| ext == "app")
                .then(|| p.to_path_buf())
        })
    }

    fn register_smappservice(prompt_user: bool) -> Result<(), String> {
        let service = AppService::new(ServiceType::MainApp);
        match service.status() {
            ServiceStatus::Enabled => return Ok(()),
            ServiceStatus::RequiresApproval => return needs_approval(prompt_user),
            _ => {}
        }

        match service.register() {
            Ok(()) => match service.status() {
                ServiceStatus::Enabled => Ok(()),
                ServiceStatus::RequiresApproval => needs_approval(prompt_user),
                other => Err(format!("开机启动注册未完成（{other}）")),
            },
            Err(ServiceManagementError::AlreadyRegistered) => match service.status() {
                ServiceStatus::Enabled => Ok(()),
                ServiceStatus::RequiresApproval => needs_approval(prompt_user),
                other => Err(format!("开机启动已注册但未生效（{other}）")),
            },
            Err(err) => Err(err.to_string()),
        }
    }

    fn unregister_smappservice() -> Result<(), String> {
        let service = AppService::new(ServiceType::MainApp);
        match service.status() {
            ServiceStatus::NotRegistered | ServiceStatus::NotFound => Ok(()),
            _ => match service.unregister() {
                Ok(()) => Ok(()),
                Err(ServiceManagementError::JobNotFound) => Ok(()),
                Err(err) => Err(err.to_string()),
            },
        }
    }

    fn needs_approval(prompt_user: bool) -> Result<(), String> {
        if prompt_user {
            AppService::open_system_settings_login_items();
            Err(APPROVAL_MSG.to_string())
        } else {
            // Quiet path: not actually enabled. Caller falls back to LaunchAgent.
            Err("launch at login requires approval in System Settings".into())
        }
    }

    /// LaunchAgent fallback: `open -a App.app` so launchd never tries to exec a directory,
    /// and Launch Services reuses an already-running instance if the plist is loaded now.
    fn sync_launch_agent(enabled: bool) -> Result<(), String> {
        let Some(bundle) = bundled_app_path() else {
            if !enabled {
                remove_launch_agent(BUNDLE_ID);
            }
            return Ok(());
        };

        if enabled && !launch_agent_is_valid(&bundle) {
            remove_launch_agent(BUNDLE_ID);
        }

        let auto = auto_launch::AutoLaunchBuilder::new()
            .set_app_name(BUNDLE_ID)
            .set_app_path("/usr/bin/open")
            .set_args(&["-a", &bundle.to_string_lossy()])
            .set_macos_launch_mode(auto_launch::MacOSLaunchMode::LaunchAgent)
            .set_bundle_identifiers(&[BUNDLE_ID])
            .build()
            .map_err(|e| e.to_string())?;
        apply_auto_launch(&auto, enabled)
    }

    fn launch_agent_is_valid(bundle: &std::path::Path) -> bool {
        let Some(path) = launch_agent_path(BUNDLE_ID) else {
            return false;
        };
        let Ok(contents) = fs::read_to_string(path) else {
            return false;
        };
        contents.contains("<string>/usr/bin/open</string>")
            && contents.contains(&*bundle.to_string_lossy())
    }

    fn launch_agent_path(label: &str) -> Option<PathBuf> {
        Some(
            dirs::home_dir()?
                .join("Library")
                .join("LaunchAgents")
                .join(format!("{label}.plist")),
        )
    }

    fn remove_launch_agent(label: &str) {
        let Some(plist) = launch_agent_path(label) else {
            return;
        };

        if let Some(uid) = current_uid() {
            let spec = format!("gui/{uid}/{label}");
            let _ = Command::new("launchctl").args(["bootout", &spec]).output();
        }
        if plist.exists() {
            let _ = fs::remove_file(plist);
        }
    }

    fn current_uid() -> Option<u32> {
        let output = Command::new("id").arg("-u").output().ok()?;
        String::from_utf8(output.stdout).ok()?.trim().parse().ok()
    }
}
