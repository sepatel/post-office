# Desktop GUI

## Tech Stack

| Component | Choice |
|-----------|--------|
| Framework | Tauri v2 |
| Frontend | React + TypeScript (via Vite) |
| Styling | Tailwind CSS |
| State | React Context + hooks |
| System Tray | Tauri `tray-icon` feature |

## Tauri Configuration

```json
// src-tauri/tauri.conf.json
{
  "$schema": "https://raw.githubusercontent.com/nicoulaj/tauri-conf-schema/main/schema.json",
  "productName": "Post Office",
  "version": "0.1.0",
  "identifier": "com.postoffice.app",
  "build": {
    "frontendDist": "../dist",
    "devUrl": "http://localhost:1420",
    "beforeDevCommand": "npm run dev",
    "beforeBuildCommand": "npm run build"
  },
  "app": {
    "windows": [
      {
        "title": "Post Office",
        "width": 1200,
        "height": 800,
        "minWidth": 800,
        "minHeight": 600,
        "visible": false,
        "decorations": true,
        "resizable": true
      }
    ],
    "security": {
      "csp": null
    }
  },
  "bundle": {
    "active": true,
    "targets": "all",
    "icon": [
      "icons/32x32.png",
      "icons/128x128.png",
      "icons/128x128@2x.png",
      "icons/icon.icns",
      "icons/icon.ico"
    ]
  }
}
```

## Tauri Dependencies

```toml
# src-tauri/Cargo.toml
[dependencies]
tauri = { version = "2", features = ["tray-icon"] }
tauri-plugin-notification = "2"
post-office-core = { path = "../crates/core" }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
tokio = { version = "1", features = ["full"] }
```

## System Tray

### Tray Setup

```rust
// src-tauri/src/tray.rs

use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem, CheckMenuItem},
    tray::{TrayIconBuilder, TrayIconEvent, MouseButton, MouseButtonState},
    Manager, AppHandle,
};
use tauri_plugin_notification::NotificationExt;

pub fn setup_tray(app: &tauri::App) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, "show", "Show Window", true, None::<&str>)?;
    let hide = MenuItem::with_id(app, "hide", "Hide Window", true, None::<&str>)?;
    let separator = PredefinedMenuItem::separator(app)?;
    let pause = CheckMenuItem::with_id(app, "pause", "Paused", true, true, None::<&str>)?;
    let status = MenuItem::with_id(app, "status", "Status: Idle", false, None::<&str>)?;
    let separator2 = PredefinedMenuItem::separator(app)?;
    let settings = MenuItem::with_id(app, "settings", "Settings", true, None::<&str>)?;
    let separator3 = PredefinedMenuItem::separator(app)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;

    let menu = Menu::with_items(
        app,
        &[
            &show,
            &hide,
            &separator,
            &pause,
            &status,
            &separator2,
            &settings,
            &separator3,
            &quit,
        ],
    )?;

    let _tray = TrayIconBuilder::with_id("main-tray")
        .icon(app.default_window_icon().unwrap().clone())
        .tooltip("Post Office")
        .menu(&menu)
        .menu_on_left_click(false) // Left click = show, right click = menu
        .on_menu_event(handle_menu_event)
        .on_tray_icon_event(handle_tray_event)
        .build(app)?;

    Ok(())
}

fn handle_menu_event(app: &AppHandle, event: tauri::menu::MenuEvent) {
    match event.id.as_ref() {
        "show" => {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.unminimize();
                let _ = window.show();
                let _ = window.set_focus();
            }
        }
        "hide" => {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.hide();
            }
        }
        "pause" => {
            if let Err(e) = app.emit("toggle-pause", ()) {
                tracing::error!("failed to emit toggle-pause: {e}");
            }
        }
        "settings" => {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.set_focus();
                if let Err(e) = app.emit("navigate", "settings") {
                    tracing::error!("failed to emit navigate: {e}");
                }
            }
        }
        "quit" => {
            app.exit(0);
        }
        _ => {}
    }
}

fn handle_tray_event(tray: &tauri::tray::TrayIcon, event: TrayIconEvent) {
    if let TrayIconEvent::Click {
        button: MouseButton::Left,
        button_state: MouseButtonState::Up,
        ..
    } = event
    {
        let app = tray.app_handle();
        if let Some(window) = app.get_webview_window("main") {
            let _ = window.unminimize();
            let _ = window.show();
            let _ = window.set_focus();
        }
    }
}
```

### Dynamic Tray Updates

```rust
// src-tauri/src/commands.rs

use tauri::{AppHandle, Manager};
use tauri_plugin_notification::NotificationExt;

#[tauri::command]
pub fn update_tray_status(app: AppHandle, status: TrayStatus) {
    let Some(tray) = app.tray_by_id("main-tray") else {
        return;
    };

    match status {
        TrayStatus::Idle => {
            let _ = tray.set_tooltip(Some("Post Office — Idle"));
        }
        TrayStatus::Processing(count) => {
            let _ = tray.set_tooltip(Some(&format!("Post Office — Processing {count} emails")));
        }
        TrayStatus::Error(msg) => {
            let _ = tray.set_tooltip(Some(&format!("Post Office — Error: {msg}")));
            if let Err(e) = app
                .notification()
                .builder()
                .title("Post Office Error")
                .body(&msg)
                .show()
            {
                tracing::error!("failed to send notification: {e}");
            }
        }
        TrayStatus::Paused => {
            let _ = tray.set_tooltip(Some("Post Office — Paused"));
        }
    }
}

pub enum TrayStatus {
    Idle,
    Processing(usize),
    Error(String),
    Paused,
}
```

## IPC Commands

```rust
// src-tauri/src/commands.rs

use post_office_core::db::Database;
use post_office_core::rules::models::Rule;
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Debug, thiserror::Error)]
pub enum CommandError {
    #[error(transparent)]
    Database(#[from] post_office_core::db::DbError),

    #[error(transparent)]
    Gmail(#[from] post_office_core::gmail::GmailError),

    #[error(transparent)]
    Llm(#[from] post_office_core::llm::LlmError),
}

// Tauri commands require Result<T, String>
impl From<CommandError> for String {
    fn from(err: CommandError) -> String {
        err.to_string()
    }
}

pub struct AppState {
    pub db: Arc<Database>,
    pub processing_state: Arc<Mutex<ProcessingState>>,
}

pub struct ProcessingState {
    pub paused: bool,
    pub last_processed: Option<chrono::DateTime<chrono::Utc>>,
    pub emails_processed_today: usize,
}

#[tauri::command]
pub async fn gmail_authenticate(app: AppHandle) -> Result<String, String> {
    todo!()
}

#[tauri::command]
pub async fn gmail_get_profile(state: tauri::State<'_, AppState>) -> Result<GmailProfile, String> {
    todo!()
}

#[tauri::command]
pub async fn rules_list(state: tauri::State<'_, AppState>) -> Result<Vec<Rule>, String> {
    state.db.rules().list_all().map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn rules_create(
    state: tauri::State<'_, AppState>,
    rule: CreateRuleRequest,
) -> Result<Rule, String> {
    state.db.rules().create(&rule).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn rules_update(
    state: tauri::State<'_, AppState>,
    id: i64,
    rule: UpdateRuleRequest,
) -> Result<Rule, String> {
    state.db.rules().update(id, &rule).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn rules_delete(state: tauri::State<'_, AppState>, id: i64) -> Result<(), String> {
    state.db.rules().delete(id).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn history_list(
    state: tauri::State<'_, AppState>,
    page: u32,
    per_page: u32,
    filter: Option<HistoryFilter>,
) -> Result<HistoryPage, String> {
    state.db.history().list(page, per_page, filter).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn history_search(
    state: tauri::State<'_, AppState>,
    query: String,
) -> Result<Vec<HistoryEntry>, String> {
    state.db.history().search(&query).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn config_get(state: tauri::State<'_, AppState>) -> Result<AppConfig, String> {
    state.db.config().get_all().map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn config_set(
    state: tauri::State<'_, AppState>,
    key: String,
    value: String,
) -> Result<(), String> {
    state.db.config().set(&key, &value).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn processing_pause(state: tauri::State<'_, AppState>) -> Result<(), String> {
    state.processing_state.lock().await.paused = true;
    Ok(())
}

#[tauri::command]
pub async fn processing_resume(state: tauri::State<'_, AppState>) -> Result<(), String> {
    state.processing_state.lock().await.paused = false;
    Ok(())
}

#[tauri::command]
pub async fn processing_status(
    state: tauri::State<'_, AppState>,
) -> Result<ProcessingStatus, String> {
    let ps = state.processing_state.lock().await;
    Ok(ProcessingStatus {
        paused: ps.paused,
        last_processed: ps.last_processed,
        emails_processed_today: ps.emails_processed_today,
    })
}

#[tauri::command]
pub async fn processing_run_now(app: AppHandle) -> Result<(), String> {
    app.emit("run-now", ()).map_err(|e| e.to_string())
}
```

## Frontend Pages

### Page Structure

```
src/
├── App.tsx                    # Router + layout
├── main.tsx                   # Entry point
├── pages/
│   ├── Dashboard.tsx          # Overview + recent activity
│   ├── Rules.tsx              # Rule list
│   ├── RuleEditor.tsx         # Create/edit rule
│   ├── History.tsx            # Processing history (searchable)
│   └── Settings.tsx           # Configuration
├── components/
│   ├── Layout.tsx             # Sidebar + content area
│   ├── Sidebar.tsx            # Navigation
│   ├── RuleCard.tsx           # Rule summary card
│   ├── HistoryRow.tsx         # Single history entry
│   ├── EmailPreview.tsx       # Email content preview
│   └── ConfigForm.tsx         # Settings form
└── lib/
    └── tauri.ts               # Tauri invoke wrappers
```

### Dashboard Page

Shows:
- Processing status (active/paused, last run time)
- Today's stats (emails processed, rules matched)
- Recent activity (last 10 processed emails)
- Quick actions (Run Now, Pause/Resume)

### Rules Page

Shows:
- List of all rules with enable/disable toggle
- Drag-and-drop reordering (priority)
- Quick edit button
- Delete with confirmation

### Rule Editor Page

Shows:
- Rule name + description
- Condition builder (visual or JSON)
- Prompt editor (large textarea)
- Action checkboxes
- Priority slider
- Test button (dry run against recent emails)

### History Page

Shows:
- Searchable list of processed emails
- Filter by: date range, rule, action, status
- Full-text search across email subjects, senders, LLM responses
- Click to expand: full email preview + LLM response + actions taken

### Settings Page

Shows:
- Gmail account connection status
- OAuth2 re-authentication button
- LLM endpoint configuration (base URL, API key, model)
- Polling interval slider
- Default actions for unmatched emails
- Import/export rules

## Window Behavior

| Action | Behavior |
|--------|----------|
| Close button (X) | Minimize to tray (not quit) |
| Left click tray | Show/focus main window |
| Right click tray | Context menu |
| Double click tray | Show/focus main window |
| Cmd+Q / Ctrl+Q | Actually quit (from tray menu) |

### Minimize to Tray

```rust
// src-tauri/src/lib.rs

.on_window_event(|window, event| {
    if let tauri::WindowEvent::CloseRequested { api, .. } = event {
        // Hide instead of close
        let _ = window.hide();
        api.prevent_close();
    }
})
```

## Tauri Events (Backend → Frontend)

```rust
// Events emitted by backend
app.emit("email-processed", &EmailProcessedEvent { ... })?;
app.emit("processing-status", &ProcessingStatus { ... })?;
app.emit("tray-status", &TrayStatus { ... })?;
app.emit("navigate", "settings")?;

// Frontend listens
use tauri::event::listen;

listen("email-processed", |event| {
    // Update history list
});

listen("navigate", |event| {
    let page: String = event.payload();
    router.navigate(&page);
});
```

## Notifications

```rust
use tauri_plugin_notification::NotificationExt;

// From tray or processing
app.notification()
    .builder()
    .title("Email Processed")
    .body("Invoice detected and labeled")
    .show()
    .expect("notification failed");
```
