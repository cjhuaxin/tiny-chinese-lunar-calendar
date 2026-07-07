use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::MainThreadMarker;
use objc2_app_kit::{
    NSAboutPanelOptionApplicationName, NSAboutPanelOptionApplicationVersion, NSAlert,
    NSAlertStyle, NSApplication, NSCellImagePosition, NSEvent, NSEventMask, NSFont, NSImageScaling,
    NSImageView, NSLayoutAttribute, NSScreen, NSStackView, NSTextAlignment, NSTextField,
    NSUserInterfaceLayoutOrientation, NSView, NSWorkspace, NSWorkspaceDidWakeNotification,
};
use objc2_foundation::{NSArray, NSDictionary, NSEdgeInsets, NSNotification, NSOperationQueue, NSSize, NSString};
use std::mem;
use std::ptr::NonNull;
use tray_icon::TrayIcon;

/// tray-icon scales status item images to 18pt; bump to match standard menu bar icons.
const MENU_BAR_ICON_HEIGHT_PT: f64 = 22.0;

/// Shows the standard macOS About panel with the compile-time app version and
/// `Credits.html` from the app bundle (clickable GitHub link).
pub fn show_about_panel() {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };

    let keys = [
        unsafe { NSAboutPanelOptionApplicationName },
        unsafe { NSAboutPanelOptionApplicationVersion },
    ];
    let objects: [Retained<AnyObject>; 2] = [
        Retained::into_super(Retained::into_super(NSString::from_str(
            crate::settings::APP_NAME,
        ))),
        Retained::into_super(Retained::into_super(NSString::from_str(env!(
            "CARGO_PKG_VERSION"
        )))),
    ];

    let dict = NSDictionary::from_retained_objects(&keys, &objects);
    unsafe {
        NSApplication::sharedApplication(mtm).orderFrontStandardAboutPanelWithOptions(&dict);
    }
}

fn centered_label(text: &str, font: Option<&NSFont>, mtm: MainThreadMarker) -> Retained<NSTextField> {
    let label = NSTextField::labelWithString(&NSString::from_str(text), mtm);
    label.setEditable(false);
    label.setSelectable(false);
    label.setBezeled(false);
    label.setDrawsBackground(false);
    label.setAlignment(NSTextAlignment::Center);
    if let Some(cell) = label.cell() {
        cell.setAlignment(NSTextAlignment::Center);
    }
    if let Some(font) = font {
        label.setFont(Some(font));
    }
    label
}

fn app_icon_view(mtm: MainThreadMarker) -> Retained<NSImageView> {
    let view = NSImageView::new(mtm);
    if let Some(icon) = NSApplication::sharedApplication(mtm).applicationIconImage() {
        icon.setSize(NSSize::new(56.0, 56.0));
        view.setImage(Some(&icon));
    }
    view.setImageScaling(NSImageScaling::ScaleProportionallyUpOrDown);
    view
}

fn centered_status_accessory(title: &str, subtitle: &str, mtm: MainThreadMarker) -> Retained<NSStackView> {
    let title_font = NSFont::boldSystemFontOfSize(15.0);
    let subtitle_font = NSFont::systemFontOfSize(13.0);

    let icon = app_icon_view(mtm);
    let title_label = centered_label(title, Some(&title_font), mtm);
    let subtitle_label = centered_label(subtitle, Some(&subtitle_font), mtm);

    let views: [&NSView; 3] = [icon.as_ref(), title_label.as_ref(), subtitle_label.as_ref()];
    let views_array = NSArray::from_slice(&views);
    let stack = NSStackView::stackViewWithViews(&views_array, mtm);
    stack.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
    stack.setAlignment(NSLayoutAttribute::CenterX);
    stack.setSpacing(10.0);
    stack.setEdgeInsets(NSEdgeInsets {
        top: 12.0,
        left: 16.0,
        bottom: 4.0,
        right: 16.0,
    });
    stack
}

fn show_status_alert(title: &str, subtitle: &str, mtm: MainThreadMarker) {
    let alert = NSAlert::new(mtm);
    alert.setMessageText(&NSString::from_str(""));
    alert.setInformativeText(&NSString::from_str(""));
    alert.setAlertStyle(NSAlertStyle::Informational);
    alert.setAccessoryView(Some(&centered_status_accessory(title, subtitle, mtm)));
    alert.addButtonWithTitle(&NSString::from_str("好"));
    alert.layout();
    alert.runModal();
}

/// Shows a concise, centered "already up to date" dialog.
pub fn show_up_to_date_alert(version: &str) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    show_status_alert("已是最新版本", &format!("v{version}"), mtm);
}

/// Shows a concise error dialog when the update feed cannot be fetched.
pub fn show_update_error_alert() {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    show_status_alert("无法检查更新", "请稍后再试", mtm);
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
