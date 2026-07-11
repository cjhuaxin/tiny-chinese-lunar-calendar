use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::MainThreadMarker;
use objc2_app_kit::{
    NSAboutPanelOptionApplicationName, NSAboutPanelOptionApplicationVersion,
    NSAboutPanelOptionCredits, NSApplication, NSCellImagePosition, NSEvent, NSEventMask, NSMenu,
    NSScreen, NSView, NSWorkspace, NSWorkspaceDidWakeNotification,
};
use objc2_foundation::{
    NSDictionary, NSAttributedString, NSNotification, NSOperationQueue, NSSize, NSString,
};
use std::mem;
use std::ptr::NonNull;
use tray_icon::TrayIcon;

/// tray-icon scales status item images to 18pt; bump to match standard menu bar icons.
const MENU_BAR_ICON_HEIGHT_PT: f64 = 22.0;

/// Brings the app to the foreground so panels and windows are not hidden behind other apps.
pub fn activate_app() {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    #[allow(deprecated)]
    NSApplication::sharedApplication(mtm).activateIgnoringOtherApps(true);
}

/// Raises a Slint/winit window above other applications.
pub fn raise_slint_window(window: &slint::Window) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use slint::winit_030::WinitWindowAccessor;

    activate_app();
    window.with_winit_window(|winit_window| {
        if let Ok(handle) = winit_window.window_handle() {
            if let RawWindowHandle::AppKit(appkit) = handle.as_raw() {
                let ns_view: &NSView = unsafe { appkit.ns_view.cast().as_ref() };
                if let Some(ns_window) = ns_view.window() {
                    ns_window.orderFrontRegardless();
                    ns_window.makeKeyAndOrderFront(None);
                }
            }
        }
        winit_window.focus_window();
    });
}

fn about_panel_string(value: &str) -> Retained<AnyObject> {
    Retained::into_super(Retained::into_super(NSString::from_str(value)))
}

/// Shows the standard macOS About panel with custom primary/secondary text.
/// Pass `hide_credits: true` to omit the credits button (update status dialogs).
fn show_about_style_panel(primary: &str, secondary: &str, hide_credits: bool, mtm: MainThreadMarker) {
    activate_app();

    if hide_credits {
        let keys = [
            unsafe { NSAboutPanelOptionApplicationName },
            unsafe { NSAboutPanelOptionApplicationVersion },
            unsafe { NSAboutPanelOptionCredits },
        ];
        let empty_credits =
            NSAttributedString::from_nsstring(&NSString::from_str(""));
        let objects: [Retained<AnyObject>; 3] = [
            about_panel_string(primary),
            about_panel_string(secondary),
            Retained::into_super(Retained::into_super(empty_credits)),
        ];
        let dict = NSDictionary::from_retained_objects(&keys, &objects);
        unsafe {
            NSApplication::sharedApplication(mtm).orderFrontStandardAboutPanelWithOptions(&dict);
        }
    } else {
        let keys = [
            unsafe { NSAboutPanelOptionApplicationName },
            unsafe { NSAboutPanelOptionApplicationVersion },
        ];
        let objects: [Retained<AnyObject>; 2] = [
            about_panel_string(primary),
            about_panel_string(secondary),
        ];
        let dict = NSDictionary::from_retained_objects(&keys, &objects);
        unsafe {
            NSApplication::sharedApplication(mtm).orderFrontStandardAboutPanelWithOptions(&dict);
        }
    }
}

/// Shows the standard macOS About panel with the compile-time app version and
/// `Credits.html` from the app bundle (clickable GitHub link).
pub fn show_about_panel() {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    show_about_style_panel(
        crate::settings::APP_NAME,
        env!("CARGO_PKG_VERSION"),
        false,
        mtm,
    );
}

/// Shows a concise, centered "already up to date" dialog.
pub fn show_up_to_date_alert(version: &str) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    show_about_style_panel("已是最新版本", version, true, mtm);
}

/// Shows a concise error dialog when the update feed cannot be fetched.
pub fn show_update_error_alert() {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    show_about_style_panel("无法检查更新", "网络连接失败，请检查网络或代理", true, mtm);
}

/// Shows immediate feedback while Sparkle fetches the appcast.
pub fn show_checking_update_alert() {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    show_about_style_panel("正在检查更新", "请稍候…", true, mtm);
}

/// Closes the shared About panel used for update status dialogs, e.g. when
/// handing off from "正在检查更新" to Sparkle's own update window.
pub fn close_update_status_panel() {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let app = NSApplication::sharedApplication(mtm);
    for window in app.windows().iter() {
        // The standard About panel is a private NSAboutPanel subclass.
        let is_about = window.class().name().to_str().is_ok_and(|n| n.contains("About"));
        if is_about {
            window.orderOut(None);
        }
    }
}

/// Forces tray menu icons into monochrome template rendering, matching the system Quit item.
pub fn apply_tray_menu_icon_style(menu: &tray_icon::menu::Menu) {
    use tray_icon::menu::ContextMenu;

    let ns_menu_ptr = menu.ns_menu();
    if ns_menu_ptr.is_null() {
        return;
    }

    unsafe {
        let ns_menu: &NSMenu = &*ns_menu_ptr.cast();
        for item in ns_menu.itemArray().iter() {
            if let Some(image) = item.image() {
                image.setTemplate(true);
                image.setSize(NSSize::new(16.0, 16.0));
                item.setImage(Some(&image));
            }
        }
    }
}

pub fn observe_system_wake(handler: impl Fn() + Send + Sync + 'static) {
    let block = RcBlock::new(move |_notification: NonNull<NSNotification>| {
        handler();
    });

    let workspace = NSWorkspace::sharedWorkspace();
    let center = workspace.notificationCenter();

    let observer = unsafe {
        center.addObserverForName_object_queue_usingBlock(
            Some(NSWorkspaceDidWakeNotification),
            None,
            Some(NSOperationQueue::mainQueue().as_ref()),
            &block,
        )
    };

    // Keep the observer alive for the lifetime of the app.
    mem::forget(observer);
}

/// Height of the primary screen in Cocoa points, used to flip between
/// Cocoa's bottom-left and winit's top-left global coordinate systems.
fn primary_screen_height(mtm: MainThreadMarker) -> Option<f64> {
    Some(NSScreen::screens(mtm).firstObject()?.frame().size.height)
}

/// Returns the tray icon's anchor point in *logical* top-left-origin screen
/// coordinates: (horizontal center of the icon, bottom edge of the menu bar).
/// Reading the NSStatusItem frame directly avoids the physical-pixel round
/// trip through the tray-icon crate, which is ambiguous on multi-monitor
/// setups with mixed scale factors.
pub fn tray_anchor_logical(tray: &TrayIcon) -> Option<(f64, f64)> {
    let mtm = MainThreadMarker::new()?;
    let status_item = tray.ns_status_item()?;
    let button = status_item.button(mtm)?;
    let window = button.window()?;
    let frame = window.frame();
    let screen_height = primary_screen_height(mtm)?;

    let center_x = frame.origin.x + frame.size.width / 2.0;
    // Cocoa frame origin is the bottom-left corner; the flipped y of that
    // bottom edge is exactly where the popover's top should go.
    let bottom_y = screen_height - frame.origin.y;
    Some((center_x, bottom_y))
}

/// Returns the logical top-left-origin frame (x, y, width, height) of the
/// screen containing the given logical point.
pub fn screen_frame_at(x: f64, y: f64) -> Option<(f64, f64, f64, f64)> {
    let mtm = MainThreadMarker::new()?;
    let screen_height = primary_screen_height(mtm)?;
    for screen in NSScreen::screens(mtm) {
        let frame = screen.frame();
        let fx = frame.origin.x;
        let fy = screen_height - frame.origin.y - frame.size.height;
        let (fw, fh) = (frame.size.width, frame.size.height);
        if x >= fx && x < fx + fw && y >= fy && y < fy + fh {
            return Some((fx, fy, fw, fh));
        }
    }
    None
}

/// Debug helper: dumps every screen's raw Cocoa frame/visibleFrame plus the
/// status-item window's raw frame, so coordinate-flipping bugs can be traced.
pub fn debug_dump_screens(tray: &TrayIcon) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    for (i, screen) in NSScreen::screens(mtm).iter().enumerate() {
        let f = screen.frame();
        let v = screen.visibleFrame();
        eprintln!(
            "screen[{i}]: frame=({}, {}, {}x{}) visible=({}, {}, {}x{}) scale={}",
            f.origin.x,
            f.origin.y,
            f.size.width,
            f.size.height,
            v.origin.x,
            v.origin.y,
            v.size.width,
            v.size.height,
            screen.backingScaleFactor(),
        );
    }
    if let Some(status_item) = tray.ns_status_item() {
        if let Some(button) = status_item.button(mtm) {
            if let Some(window) = button.window() {
                let f = window.frame();
                eprintln!(
                    "status window: frame=({}, {}, {}x{}) on-screen={:?}",
                    f.origin.x,
                    f.origin.y,
                    f.size.width,
                    f.size.height,
                    window.screen().map(|s| s.frame()),
                );
            }
        }
    }
}

/// Observes mouse clicks in *other* applications (global NSEvent monitor).
/// Used to close the calendar popover when the user clicks anywhere outside
/// the app, mirroring NSPopover's transient behaviour. Clicks inside our own
/// windows or on our status item never reach this monitor.
pub fn observe_global_clicks(handler: impl Fn() + 'static) {
    let block = RcBlock::new(move |_event: NonNull<NSEvent>| {
        handler();
    });

    let mask = NSEventMask::LeftMouseDown
        | NSEventMask::RightMouseDown
        | NSEventMask::OtherMouseDown;

    let monitor = NSEvent::addGlobalMonitorForEventsMatchingMask_handler(mask, &block);

    // Keep the monitor alive for the lifetime of the app.
    mem::forget(monitor);
}

pub fn resize_tray_icon(tray: &TrayIcon) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let Some(status_item) = tray.ns_status_item() else {
        return;
    };
    let Some(button) = status_item.button(mtm) else {
        return;
    };
    let Some(image) = button.image() else {
        return;
    };

    let size = image.size();
    let width = if size.height > 0.0 {
        size.width / size.height * MENU_BAR_ICON_HEIGHT_PT
    } else {
        MENU_BAR_ICON_HEIGHT_PT
    };

    image.setSize(NSSize::new(width, MENU_BAR_ICON_HEIGHT_PT));
    button.setImagePosition(NSCellImagePosition::ImageOnly);
    button.setImage(Some(&image));
}
