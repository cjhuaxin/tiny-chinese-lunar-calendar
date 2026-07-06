#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod fontload;
mod models;
mod services;
mod settings;
mod textfit;
mod tray;

slint::include_modules!();

use std::cell::Cell;
use std::time::Duration;

use slint::winit_030::winit;
use tray_icon::menu::{AboutMetadata, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};

use crate::app::{install_app, with_app, App};
use crate::services::holiday;

const SETTINGS_MENU_ID: &str = "settings";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Destroy the winit window (and the femtovg GL context with its glyph
    // atlas) whenever a window is hidden, so hiding the calendar returns the
    // process to its tray-only memory footprint. Slint recreates the window
    // on the next show().
    std::env::set_var("SLINT_DESTROY_WINDOW_ON_HIDE", "1");

    let event_loop_builder = {
        #[allow(unused_mut)]
        let mut builder =
            winit::event_loop::EventLoop::<slint::winit_030::SlintEvent>::with_user_event();
        #[cfg(target_os = "macos")]
        {
            use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};
            builder.with_activation_policy(ActivationPolicy::Accessory);
        }
        builder
    };

    slint::BackendSelector::new()
        .backend_name("winit".into())
        // femtovg keeps system fonts memory-mapped on disk (fontdb + mmap),
        // unlike Skia's fontique which copies whole .ttc files onto the heap.
        .renderer_name("femtovg".into())
        .with_winit_event_loop_builder(event_loop_builder)
        .with_winit_window_attributes_hook(|attributes| {
            // Both app windows are frameless. Slint would apply no-frame only
            // *after* the window is shown; removing the title bar then keeps
            // the content rect, dropping the top edge by the title bar height
            // - a visible jump. Create the window borderless from the start.
            let attributes = attributes.with_transparent(true).with_decorations(false);
            // Create the window directly with its final frame (queued just
            // before show()); moving or resizing it after it appears causes
            // a visible jump (Cocoa anchors resizes at the bottom-left).
            if let Some(((x, y), (w, h))) = app::take_pending_window_geometry() {
                attributes
                    .with_position(winit::dpi::LogicalPosition::new(x, y))
                    .with_inner_size(winit::dpi::LogicalSize::new(w, h))
            } else {
                attributes
            }
        })
        .select()?;

    let app = App::new()?;
    install_app(app);

    // Sync launch-at-login with the persisted preference, like the Tauri setup hook.
    with_app(|app| {
        let launch = app.state.settings.borrow().launch_at_login;
        if let Err(err) = settings::sync_launch_at_login(launch) {
            eprintln!("failed to sync launch at login: {err}");
        }
    });

    // Load holiday data; refresh the UI once fresh data arrives from the network.
    holiday::ensure_holiday_data(|| {
        let _ = slint::invoke_from_event_loop(|| {
            with_app(|app| {
                if let Some(main) = app.main_handle() {
                    app::refresh_all(&main, &app.state);
                }
            });
        });
    });

    // Tray events can be delivered from outside the Slint event loop; forward
    // them onto the main thread.
    TrayIconEvent::set_event_handler(Some(|event: TrayIconEvent| {
        let _ = slint::invoke_from_event_loop(move || handle_tray_event(event));
    }));

    MenuEvent::set_event_handler(Some(|event: MenuEvent| {
        let _ = slint::invoke_from_event_loop(move || {
            if event.id.as_ref() == SETTINGS_MENU_ID {
                with_app(|app| app.show_settings());
            }
        });
    }));

    // The tray icon must be created once the event loop is running (macOS).
    slint::Timer::single_shot(Duration::ZERO, || {
        if let Err(err) = create_tray() {
            eprintln!("failed to create tray icon: {err}");
        }

        // Close the (unpinned) calendar when the user clicks anywhere outside
        // the app, like a transient NSPopover. Focus events are unreliable for
        // Accessory apps, so a global mouse monitor is the primary mechanism.
        #[cfg(target_os = "macos")]
        tray::macos::observe_global_clicks(|| {
            let _ = slint::invoke_from_event_loop(|| {
                with_app(|app| {
                    if !app.state.pinned.get() && app.main_handle().is_some() {
                        app.drop_main();
                    }
                });
            });
        });
        tray::icon::set_date_changed_handler(|| {
            with_app(|app| {
                if let Some(main) = app.main_handle() {
                    app::refresh_all(&main, &app.state);
                }
            });
        });
        tray::icon::start_date_watch();
    });

    // Debug aid: show the main window pinned at startup (TCLC_SHOW=1 cargo run).
    if std::env::var("TCLC_SHOW").is_ok() {
        with_app(|app| {
            app.state.pinned.set(true);
            if let Ok(main) = app.ensure_main() {
                main.set_pinned(true);
                let _ = main.show();
            }
        });
    }

    // Debug aid: simulate a tray click (unpinned toggle) 5s after startup,
    // using the real tray anchor like handle_tray_event does.
    if std::env::var("TCLC_TOGGLE").is_ok() {
        slint::Timer::single_shot(Duration::from_secs(5), || {
            with_app(|app| {
                #[cfg(target_os = "macos")]
                {
                    tray::icon::debug_dump_screens();
                    if let Some(anchor) = tray::icon::tray_anchor_logical() {
                        eprintln!("simulated tray click: anchor={anchor:?}");
                        app.state.tray_anchor.set(Some(anchor));
                    }
                }
                app.toggle_main_window();
            });
        });
    }

    // Debug aid: cycle show/hide every 8s to exercise window re-creation.
    // TCLC_CYCLE=once shows and hides a single time, for memory measurements.
    if let Ok(mode) = std::env::var("TCLC_CYCLE") {
        let once = mode == "once";
        let count = std::rc::Rc::new(Cell::new(0u32));
        let timer = Box::new(slint::Timer::default());
        timer.start(slint::TimerMode::Repeated, Duration::from_secs(8), move || {
            count.set(count.get() + 1);
            if once && count.get() > 2 {
                return;
            }
            with_app(|app| {
                let visible = app
                    .main_handle()
                    .is_some_and(|m| m.window().is_visible());
                eprintln!("cycle: visible={visible} -> toggling");
                if visible {
                    app.drop_main();
                } else {
                    app.state.pinned.set(true);
                    if let Ok(main) = app.ensure_main() {
                        main.set_pinned(true);
                        let _ = main.show();
                    }
                }
            });
        });
        Box::leak(timer);
    }

    slint::run_event_loop_until_quit()?;
    Ok(())
}

fn create_tray() -> Result<(), Box<dyn std::error::Error>> {
    let menu = Menu::new();
    let settings_item = MenuItem::with_id(SETTINGS_MENU_ID, "设置", true, None);
    let about_item = PredefinedMenuItem::about(
        Some("关于小小万年历"),
        Some(AboutMetadata {
            name: Some(settings::APP_NAME.to_string()),
            version: Some(env!("CARGO_PKG_VERSION").to_string()),
            ..Default::default()
        }),
    );
    let quit_item = PredefinedMenuItem::quit(Some("退出"));
    menu.append_items(&[
        &settings_item,
        &about_item,
        &PredefinedMenuItem::separator(),
        &quit_item,
    ])?;

    let tray = TrayIconBuilder::new()
        .with_id("main-tray")
        .with_tooltip(settings::APP_NAME)
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(false)
        .build()?;

    tray::icon::install_tray(tray);
    Ok(())
}

fn handle_tray_event(event: TrayIconEvent) {
    // Refresh the anchor from the NSStatusItem itself (logical points,
    // top-left origin): the rects carried by tray-icon events are physical
    // pixels flipped against the primary display, which is ambiguous on
    // multi-monitor setups.
    #[cfg(target_os = "macos")]
    if let Some(anchor) = tray::icon::tray_anchor_logical() {
        with_app(|app| app.state.tray_anchor.set(Some(anchor)));
    }

    if let TrayIconEvent::Click {
        button: MouseButton::Left,
        button_state: MouseButtonState::Up,
        ..
    } = event
    {
        with_app(|app| app.toggle_main_window());
    }
}
