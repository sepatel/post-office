use std::process::Command;
use tauri::{
    menu::{MenuBuilder, MenuItemBuilder, PredefinedMenuItem},
    tray::{TrayIcon, TrayIconBuilder},
    Manager,
};

/// Detect if the system is using a dark theme on Linux
fn is_dark_theme() -> bool {
    // Try GNOME color-scheme first (GNOME 42+)
    if let Ok(output) = Command::new("gsettings")
        .args(["get", "org.gnome.desktop.interface", "color-scheme"])
        .output()
    {
        let stdout = String::from_utf8_lossy(&output.stdout);
        if stdout.contains("dark") {
            return true;
        }
    }

    // Fallback: check GTK theme name
    if let Ok(output) = Command::new("gsettings")
        .args(["get", "org.gnome.desktop.interface", "gtk-theme"])
        .output()
    {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let theme = stdout.trim().to_lowercase();
        if theme.contains("dark") || theme.contains("night") {
            return true;
        }
    }

    // Check dconf directly
    if let Ok(output) = Command::new("dconf")
        .args(["read", "/org/gnome/desktop/interface/color-scheme"])
        .output()
    {
        let stdout = String::from_utf8_lossy(&output.stdout);
        if stdout.contains("dark") {
            return true;
        }
    }

    // Check GTK-3 settings.ini
    if let Some(home) = dirs::home_dir() {
        let ini = home.join(".config/gtk-3.0/settings.ini");
        if let Ok(contents) = std::fs::read_to_string(&ini) {
            let mut in_settings = false;
            for line in contents.lines() {
                let l = line.trim();
                if l.starts_with("[") {
                    in_settings = l.eq_ignore_ascii_case("[Settings]");
                    continue;
                }
                if in_settings
                    && l.to_lowercase()
                        .starts_with("gtk-application-prefer-dark-theme")
                {
                    if let Some(v) = l.split('=').nth(1) {
                        if v.trim() == "1" {
                            return true;
                        }
                    }
                }
                if in_settings && l.to_lowercase().starts_with("gtk-theme-name") {
                    if let Some(v) = l.split('=').nth(1) {
                        if v.to_lowercase().contains("dark") {
                            return true;
                        }
                    }
                }
            }
        }
    }

    // Check KDE color scheme
    if let Ok(output) = Command::new("kreadconfig5")
        .args(["--group", "General", "--key", "ColorScheme"])
        .output()
    {
        let stdout = String::from_utf8_lossy(&output.stdout);
        if stdout.to_lowercase().contains("dark") {
            return true;
        }
    }

    false
}

/// Decide whether to use the light-colored icon (for dark backgrounds)
fn use_light_icon(theme: &str) -> bool {
    match theme {
        "dark" => true,
        "light" => false,
        _ => is_dark_theme(),
    }
}

fn icon_path(app: &tauri::AppHandle, name: &str) -> Option<std::path::PathBuf> {
    app.path()
        .resource_dir()
        .ok()
        .map(|dir| dir.join("icons").join(name))
}

/// Load the tray icon based on the given theme preference
fn load_tray_icon(app: &tauri::AppHandle, theme: &str) -> tauri::image::Image<'static> {
    let preferred = if use_light_icon(theme) {
        "icon_light_32.png"
    } else {
        "icon_dark_32.png"
    };
    let fallback = if use_light_icon(theme) {
        "icon_dark_32.png"
    } else {
        "icon_light_32.png"
    };

    for name in [preferred, fallback] {
        if let Some(path) = icon_path(app, name) {
            if path.exists() {
                if let Ok(image) = tauri::image::Image::from_path(&path) {
                    return image.to_owned();
                }
            }
        }
    }

    // Fallback to default window icon
    app.default_window_icon()
        .cloned()
        .expect("No default icon available")
        .to_owned()
}

pub fn setup_tray(app: &tauri::App, theme: &str) -> Result<TrayIcon, Box<dyn std::error::Error>> {
    let show_item = MenuItemBuilder::with_id("show", "Show Window").build(app)?;
    let hide_item = MenuItemBuilder::with_id("hide", "Hide Window").build(app)?;
    let quit_item = MenuItemBuilder::with_id("quit", "Quit").build(app)?;

    let menu = MenuBuilder::new(app)
        .item(&show_item)
        .item(&hide_item)
        .item(&PredefinedMenuItem::separator(app)?)
        .item(&quit_item)
        .build()?;

    let icon = load_tray_icon(app.handle(), theme);

    let tray = TrayIconBuilder::new()
        .icon(icon)
        .menu(&menu)
        .tooltip("Post Office")
        .on_menu_event(move |app, event| match event.id().as_ref() {
            "show" => {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.set_focus();
                }
            }
            "hide" => {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.hide();
                }
            }
            "quit" => {
                app.exit(0);
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let tauri::tray::TrayIconEvent::Click {
                button: tauri::tray::MouseButton::Left,
                button_state: tauri::tray::MouseButtonState::Up,
                ..
            } = event
            {
                let app = tray.app_handle();
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.set_focus();
                }
            }
        })
        .build(app)?;

    Ok(tray)
}

/// Re-apply the tray icon based on the given theme preference
pub fn apply_tray_theme(app: &tauri::AppHandle, theme: &str) {
    let icon = load_tray_icon(app, theme);
    if let Some(tray) = app.state::<crate::AppState>().tray.lock().unwrap().as_ref() {
        let _ = tray.set_icon(Some(icon));
    }
}
