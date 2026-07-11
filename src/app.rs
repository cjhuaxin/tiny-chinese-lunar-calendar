//! Bridges the Slint UI with the calendar services, mirroring the state logic
//! of the Tauri frontend's `useCalendar` hook.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use chrono::{Datelike, Local, NaiveDate};
use slint::{ComponentHandle, VecModel};

use crate::models::{AppSettings, CalendarLabelPriority, DayDetail};
use crate::services::calendar::{build_day_detail, build_month_grid};
use crate::services::{holiday, location, weather};
use crate::settings::{load_settings, save_settings, sync_launch_at_login};
use crate::textfit;
use crate::{DayCellData, MainWindow, SettingsWindow, WeekdayLabel};

use slint::winit_030::{winit, WinitWindowAccessor};
use winit::dpi::LogicalPosition;

const MONTH_NAMES_ZH: [&str; 12] = [
    "一月", "二月", "三月", "四月", "五月", "六月", "七月", "八月", "九月", "十月", "十一月",
    "十二月",
];

const HERO_WEEKDAYS: [&str; 7] = ["周日", "周一", "周二", "周三", "周四", "周五", "周六"];

const CYCLE_INTERVAL: Duration = Duration::from_secs(5);
const DETAIL_REFRESH_INTERVAL: Duration = Duration::from_secs(30);

pub struct App {
    /// The calendar window is created on demand and dropped when hidden so
    /// the renderer (glyph atlas, shaping caches, GL context) is released and
    /// the process returns to its tray-only footprint.
    pub main: RefCell<Option<MainWindow>>,
    pub settings_win: SettingsWindow,
    pub state: Rc<State>,
    _cycle_timer: slint::Timer,
    _refresh_timer: slint::Timer,
}

pub struct State {
    pub focused_year: Cell<i32>,
    pub focused_month: Cell<u32>,
    pub selected_date: RefCell<Option<NaiveDate>>,
    pub settings: RefCell<AppSettings>,
    pub pinned: Cell<bool>,
    detail: RefCell<Option<DayDetail>>,
    relative_texts: RefCell<Vec<String>>,
    cycle_festivals: RefCell<Vec<String>>,
    cycle_index: Cell<usize>,
    last_refresh_date: Cell<NaiveDate>,
    /// Screen anchor in *logical* top-left-origin coordinates:
    /// (tray icon center x, menu bar bottom y).
    pub tray_anchor: Cell<Option<(f64, f64)>>,
    /// When the calendar window was last shown; used to debounce the
    /// focus-lost auto-hide against events fired during window creation.
    shown_at: Cell<Option<std::time::Instant>>,
    /// When the window was last auto-destroyed by the focus watcher. A tray
    /// click right after (the click itself stole focus) means "close", so the
    /// toggle must not immediately re-open the window.
    auto_hidden_at: Cell<Option<std::time::Instant>>,
    /// Remaining corrections for the freshly shown window: macOS/Slint move
    /// the just-created window shortly after our placement, so the Moved
    /// handler shoves it back to the tray anchor a limited number of times.
    pos_corrections: Cell<u32>,
}

thread_local! {
    static APP: RefCell<Option<Rc<App>>> = const { RefCell::new(None) };
    /// Geometry (logical top-left position + logical size) for the next winit
    /// window created. Consumed by the window-attributes hook in main.rs so
    /// windows are born with their final frame. Positioning or resizing them
    /// after they appear flashes: Slint resizes on first show, and Cocoa
    /// anchors resizes at the bottom-left corner, dropping the top edge.
    static PENDING_WINDOW_GEOMETRY: Cell<Option<((f64, f64), (f64, f64))>> =
        const { Cell::new(None) };
}

/// Logical size fixed in main-window.slint; needed to compute the window's
/// frame before it exists. (The settings window reads its live winit size
/// instead, so it needs no such constant.)
const MAIN_WINDOW_SIZE: (f64, f64) = (500.0, 450.0);

fn set_pending_window_geometry(pos: (f64, f64), size: (f64, f64)) {
    PENDING_WINDOW_GEOMETRY.with(|cell| cell.set(Some((pos, size))));
}

/// Called from the winit window-attributes hook.
pub fn take_pending_window_geometry() -> Option<((f64, f64), (f64, f64))> {
    PENDING_WINDOW_GEOMETRY.with(|cell| cell.take())
}

pub fn install_app(app: Rc<App>) {
    APP.with(|cell| {
        *cell.borrow_mut() = Some(app);
    });
}

pub fn with_app(f: impl FnOnce(&Rc<App>)) {
    APP.with(|cell| {
        if let Some(app) = cell.borrow().as_ref() {
            f(app);
        }
    });
}

fn today() -> NaiveDate {
    Local::now().date_naive()
}

impl App {
    pub fn new() -> Result<Rc<Self>, slint::PlatformError> {
        let settings_win = SettingsWindow::new()?;

        let now = today();
        let state = Rc::new(State {
            focused_year: Cell::new(now.year()),
            focused_month: Cell::new(now.month()),
            selected_date: RefCell::new(None),
            settings: RefCell::new(load_settings()),
            pinned: Cell::new(false),
            detail: RefCell::new(None),
            relative_texts: RefCell::new(Vec::new()),
            cycle_festivals: RefCell::new(Vec::new()),
            cycle_index: Cell::new(0),
            last_refresh_date: Cell::new(now),
            tray_anchor: Cell::new(None),
            shown_at: Cell::new(None),
            auto_hidden_at: Cell::new(None),
            pos_corrections: Cell::new(0),
        });

        // The timers look the window up through the app registry: the window
        // only exists while visible.
        let cycle_timer = slint::Timer::default();
        {
            let state = state.clone();
            cycle_timer.start(slint::TimerMode::Repeated, CYCLE_INTERVAL, move || {
                state.cycle_index.set(state.cycle_index.get().wrapping_add(1));
                let state = state.clone();
                with_app(move |app| {
                    if let Some(main) = app.main_handle() {
                        apply_cycle_texts(&main, &state);
                    }
                });
            });
        }

        let refresh_timer = slint::Timer::default();
        {
            let state = state.clone();
            refresh_timer.start(
                slint::TimerMode::Repeated,
                DETAIL_REFRESH_INTERVAL,
                move || {
                    let state = state.clone();
                    with_app(move |app| {
                        let Some(main) = app.main_handle() else {
                            return;
                        };
                        if state.last_refresh_date.get() != today() {
                            refresh_all(&main, &state);
                        } else {
                            refresh_detail(&main, &state);
                        }
                    });
                },
            );
        }

        let app = Rc::new(Self {
            main: RefCell::new(None),
            settings_win,
            state,
            _cycle_timer: cycle_timer,
            _refresh_timer: refresh_timer,
        });

        app.wire_settings_callbacks();

        Ok(app)
    }

    /// Returns a strong handle to the calendar window, if it currently exists.
    pub fn main_handle(&self) -> Option<MainWindow> {
        self.main.borrow().as_ref().map(|m| m.clone_strong())
    }

    /// Creates (or returns) the calendar window, wiring callbacks and filling
    /// in the current month data.
    pub fn ensure_main(self: &Rc<Self>) -> Result<MainWindow, slint::PlatformError> {
        if let Some(main) = self.main_handle() {
            return Ok(main);
        }
        let main = MainWindow::new()?;
        self.wire_main_callbacks(&main);
        self.install_focus_watch(&main);
        main.set_pinned(self.state.pinned.get());
        refresh_all(&main, &self.state);
        *self.main.borrow_mut() = Some(main.clone_strong());
        Ok(main)
    }

    /// Hides and destroys the calendar window, releasing its renderer
    /// resources, then nudges malloc to return freed pages to the OS.
    pub fn drop_main(&self) {
        if let Some(main) = self.main.borrow_mut().take() {
            let _ = main.window().hide();
        }
        release_malloc_pages();
    }

    fn wire_main_callbacks(self: &Rc<Self>, main: &MainWindow) {

        {
            let main_weak = main.as_weak();
            let state = self.state.clone();
            main.on_select_day(move |date| {
                let Some(main) = main_weak.upgrade() else {
                    return;
                };
                if let Ok(parsed) = NaiveDate::parse_from_str(date.as_str(), "%Y-%m-%d") {
                    *state.selected_date.borrow_mut() = Some(parsed);
                    state.focused_year.set(parsed.year());
                    state.focused_month.set(parsed.month());
                    refresh_all(&main, &state);
                }
            });
        }

        {
            let main_weak = main.as_weak();
            let state = self.state.clone();
            main.on_change_month(move |delta| {
                let Some(main) = main_weak.upgrade() else {
                    return;
                };
                let total = state.focused_year.get() * 12
                    + (state.focused_month.get() as i32 - 1)
                    + delta;
                let year = total.div_euclid(12);
                let month = (total.rem_euclid(12) + 1) as u32;
                apply_year_month(&main, &state, year, month);
            });
        }

        {
            let main_weak = main.as_weak();
            let state = self.state.clone();
            main.on_go_today(move || {
                let Some(main) = main_weak.upgrade() else {
                    return;
                };
                let now = today();
                state.focused_year.set(now.year());
                state.focused_month.set(now.month());
                *state.selected_date.borrow_mut() = None;
                refresh_all(&main, &state);
            });
        }

        {
            let main_weak = main.as_weak();
            let state = self.state.clone();
            main.on_toggle_pin(move || {
                let Some(main) = main_weak.upgrade() else {
                    return;
                };
                let next = !state.pinned.get();
                state.pinned.set(next);
                main.set_pinned(next);
            });
        }

        {
            let main_weak = main.as_weak();
            let state = self.state.clone();
            main.on_confirm_year_month(move |year, month| {
                let Some(main) = main_weak.upgrade() else {
                    return;
                };
                apply_year_month(&main, &state, year, month as u32);
            });
        }

        {
            let main_weak = main.as_weak();
            main.on_start_drag(move || {
                if let Some(main) = main_weak.upgrade() {
                    main.window().with_winit_window(|w| {
                        let _ = w.drag_window();
                    });
                }
            });
        }
    }

    fn wire_settings_callbacks(self: &Rc<Self>) {
        let win = &self.settings_win;

        {
            let win_weak = win.as_weak();
            let state = self.state.clone();
            win.on_save(move || {
                let Some(win) = win_weak.upgrade() else {
                    return;
                };
                let new_settings = AppSettings {
                    sunday_first: win.get_draft_sunday_first(),
                    show_international_festivals: win.get_draft_show_intl(),
                    launch_at_login: win.get_draft_launch_login(),
                    calendar_label_priority: if win.get_draft_priority_intl() {
                        CalendarLabelPriority::InternationalFestival
                    } else {
                        CalendarLabelPriority::SolarTerm
                    },
                };
                if let Err(err) = save_settings(&new_settings) {
                    eprintln!("failed to save settings: {err}");
                }
                if let Err(err) = sync_launch_at_login(new_settings.launch_at_login) {
                    eprintln!("failed to sync launch at login: {err}");
                }
                *state.settings.borrow_mut() = new_settings;
                let _ = win.window().hide();
                let state = state.clone();
                with_app(move |app| {
                    if let Some(main) = app.main_handle() {
                        refresh_all(&main, &state);
                    }
                });
            });
        }

        {
            let win_weak = win.as_weak();
            win.on_cancel(move || {
                if let Some(win) = win_weak.upgrade() {
                    let _ = win.window().hide();
                }
            });
        }

        {
            let win_weak = win.as_weak();
            win.on_start_drag(move || {
                if let Some(win) = win_weak.upgrade() {
                    win.window().with_winit_window(|w| {
                        let _ = w.drag_window();
                    });
                }
            });
        }
    }

    /// Toggles the main window like the Tauri tray click handler:
    /// destroy when visible, otherwise create and show below the tray icon.
    pub fn toggle_main_window(self: &Rc<Self>) {
        // Toggling always resets the pin, matching the Tauri behaviour.
        self.state.pinned.set(false);

        if self
            .main_handle()
            .is_some_and(|m| m.window().is_visible())
        {
            self.drop_main();
            return;
        }

        // If the focus watcher just tore the window down because this very
        // tray click stole focus, the user meant to close it - don't re-open.
        if self
            .state
            .auto_hidden_at
            .get()
            .is_some_and(|t| t.elapsed() < Duration::from_millis(400))
        {
            self.state.auto_hidden_at.set(None);
            return;
        }

        let anchor = self.state.tray_anchor.get();
        // Hand the target frame to the window-attributes hook so the window
        // is *created* at the right spot with its final size: positioning it
        // after it appears makes it visibly jump. Must be queued before
        // ensure_main(): the hook runs when the Slint component (window
        // adapter) is created, not when the winit window appears.
        if let Some((center_x, bottom_y)) = anchor {
            set_pending_window_geometry(
                (center_x - MAIN_WINDOW_SIZE.0 / 2.0, bottom_y),
                MAIN_WINDOW_SIZE,
            );
        }

        let Ok(main) = self.ensure_main() else {
            return;
        };

        self.state.shown_at.set(Some(std::time::Instant::now()));
        // Budget for the Moved-event watchdog. The window is created at the
        // right position, but ~100ms after showing, something inside the
        // stack shoves it 32pt down (a title-bar-height frame/content-rect
        // mixup, even though the window is borderless). The watchdog restores
        // the position *synchronously inside the Moved event*, before the
        // wrong frame is ever rendered, so no jump is visible.
        self.state.pos_corrections.set(6);
        let _ = main.show();
        self.ensure_weather_for_main(&main);

        // Deferred best-effort focus (the winit window is created a tick
        // after show()); auto-close itself relies on the global click monitor.
        {
            let main_weak = main.as_weak();
            slint::Timer::single_shot(Duration::from_millis(50), move || {
                if let Some(main) = main_weak.upgrade() {
                    main.window().with_winit_window(|w| w.focus_window());
                }
            });
        }

        if std::env::var("TCLC_DEBUG").is_ok() {
            for delay_ms in [100u64, 400] {
                let main_weak = main.as_weak();
                slint::Timer::single_shot(Duration::from_millis(delay_ms), move || {
                    if let Some(main) = main_weak.upgrade() {
                        main.window().with_winit_window(|w| {
                            eprintln!(
                                "position after {delay_ms}ms: {:?} scale={}",
                                w.outer_position(),
                                w.scale_factor()
                            );
                        });
                    }
                });
            }
        }
    }

    /// Opens the settings window centered on the monitor containing the tray
    /// anchor, mirroring `show_settings_window` from the Tauri version.
    pub fn show_settings(&self) {
        let settings = self.state.settings.borrow().clone();
        self.settings_win.set_draft_sunday_first(settings.sunday_first);
        self.settings_win
            .set_draft_show_intl(settings.show_international_festivals);
        self.settings_win
            .set_draft_launch_login(settings.launch_at_login);
        self.settings_win.set_draft_priority_intl(matches!(
            settings.calendar_label_priority,
            CalendarLabelPriority::InternationalFestival
        ));

        // NOTE: no pending-geometry trick here. The settings component lives
        // for the whole app, so the attributes hook only ran once at startup;
        // queuing geometry now would leak into the next calendar-window
        // creation instead. Post-show centering is fine for this window.
        let anchor = self.state.tray_anchor.get();
        let _ = self.settings_win.show();
        center_settings_on_anchor_screen(&self.settings_win, anchor, 8);
        #[cfg(target_os = "macos")]
        crate::tray::macos::raise_slint_window(self.settings_win.window());
    }

    /// Fetches location + weather when the calendar popover opens.
    fn ensure_weather_for_main(self: &Rc<Self>, main: &MainWindow) {
        weather::set_loading();
        weather::apply_weather_to_window(main);

        let main_weak = main.as_weak();
        location::request_coordinates(move |result| {
            match result {
                Ok((lat, lon)) => {
                    let main_weak = main_weak.clone();
                    weather::ensure_weather(lat, lon, move || {
                        let main_weak = main_weak.clone();
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(main) = main_weak.upgrade() {
                                weather::apply_weather_to_window(&main);
                            }
                        });
                    });
                }
                Err(err) => {
                    eprintln!("location failed: {err}");
                    let main_weak = main_weak.clone();
                    std::thread::spawn(move || {
                        if let Some((lat, lon)) = weather::resolve_coordinates_fallback() {
                            weather::ensure_weather(lat, lon, move || {
                                let main_weak = main_weak.clone();
                                let _ = slint::invoke_from_event_loop(move || {
                                    if let Some(main) = main_weak.upgrade() {
                                        weather::apply_weather_to_window(&main);
                                    }
                                });
                            });
                        } else {
                            weather::set_unavailable(&err);
                            let _ = slint::invoke_from_event_loop(move || {
                                if let Some(main) = main_weak.upgrade() {
                                    weather::apply_weather_to_window(&main);
                                }
                            });
                        }
                    });
                }
            }
        });
    }

    /// Installs the winit focus-lost hook that auto-destroys the main window
    /// unless it is pinned.
    fn install_focus_watch(self: &Rc<Self>, main: &MainWindow) {
        let state = self.state.clone();
        let got_focus = Cell::new(false);
        main.window().on_winit_window_event(move |window, event| {
            match event {
                winit::event::WindowEvent::Focused(true) => {
                    got_focus.set(true);
                }
                winit::event::WindowEvent::Resized(size) => {
                    if std::env::var("TCLC_DEBUG").is_ok() {
                        eprintln!("resized to {size:?}");
                    }
                }
                winit::event::WindowEvent::Moved(pos) => {
                    // Shortly after creation something inside the stack moves
                    // the window 32pt below our placement (a title-bar-height
                    // frame/content-rect mixup). Correct it *synchronously*
                    // inside the Moved event so the wrong position is never
                    // rendered. Corrections are capped and time-boxed so user
                    // drags aren't fought.
                    let budget = state.pos_corrections.get();
                    let recent = state
                        .shown_at
                        .get()
                        .is_some_and(|t| t.elapsed() < Duration::from_millis(700));
                    if budget > 0 && recent {
                        if let Some((center_x, bottom_y)) = state.tray_anchor.get() {
                            window.with_winit_window(|w| {
                                let scale = w.scale_factor();
                                let width = w.outer_size().width as f64 / scale;
                                let target_x = center_x - width / 2.0;
                                let off_target = (pos.x as f64 - target_x * scale).abs() > 1.0
                                    || (pos.y as f64 - bottom_y * scale).abs() > 1.0;
                                if std::env::var("TCLC_DEBUG").is_ok() {
                                    eprintln!(
                                        "moved to {pos:?}, target=({target_x}, {bottom_y}) \
                                         (logical), off_target={off_target}, budget={budget}"
                                    );
                                }
                                if off_target {
                                    state.pos_corrections.set(budget - 1);
                                    w.set_outer_position(LogicalPosition::new(
                                        target_x, bottom_y,
                                    ));
                                }
                            });
                        }
                    }
                }
                winit::event::WindowEvent::Focused(false) => {
                    if std::env::var("TCLC_DEBUG").is_ok() {
                        eprintln!(
                            "focus lost (got_focus={}, since_show={:?})",
                            got_focus.get(),
                            state.shown_at.get().map(|t| t.elapsed())
                        );
                    }
                    // Ignore spurious focus-lost events during window
                    // creation: only tear down once the window actually held
                    // focus and has been up for a moment.
                    let settled = state
                        .shown_at
                        .get()
                        .is_some_and(|t| t.elapsed() >= Duration::from_millis(300));
                    if !state.pinned.get() && got_focus.get() && settled {
                        state.auto_hidden_at.set(Some(std::time::Instant::now()));
                        // Defer the teardown: destroying the window from
                        // inside its own event callback would be re-entrant.
                        slint::Timer::single_shot(Duration::ZERO, || {
                            with_app(|app| app.drop_main());
                        });
                    }
                }
                _ => {}
            }
            slint::winit_030::EventResult::Propagate
        });
    }
}

/// Centers the settings window on the screen containing the tray anchor.
/// Under SLINT_DESTROY_WINDOW_ON_HIDE the winit window is recreated
/// asynchronously after `show()`, so `with_winit_window` can be a no-op at
/// first; retry on subsequent event-loop iterations until it runs.
fn center_settings_on_anchor_screen(
    win: &SettingsWindow,
    anchor: Option<(f64, f64)>,
    retries_left: u32,
) {
    let placed = win
        .window()
        .with_winit_window(|w| {
            let win_scale = w.scale_factor();
            let (win_w, win_h) = (
                w.outer_size().width as f64 / win_scale,
                w.outer_size().height as f64 / win_scale,
            );

            // Logical frame of the screen containing the tray anchor.
            #[cfg(target_os = "macos")]
            let frame = anchor.and_then(|(x, y)| crate::tray::macos::screen_frame_at(x, y));
            #[cfg(not(target_os = "macos"))]
            let frame: Option<(f64, f64, f64, f64)> = None;

            let frame = frame.or_else(|| {
                w.current_monitor().or_else(|| w.primary_monitor()).map(|m| {
                    let scale = m.scale_factor();
                    let pos = m.position();
                    let size = m.size();
                    (
                        pos.x as f64 / scale,
                        pos.y as f64 / scale,
                        size.width as f64 / scale,
                        size.height as f64 / scale,
                    )
                })
            });

            if let Some((mx, my, mw, mh)) = frame {
                w.set_outer_position(LogicalPosition::new(
                    mx + (mw - win_w) / 2.0,
                    my + (mh - win_h) / 2.0,
                ));
            }
            w.focus_window();
        })
        .is_some();

    if !placed && retries_left > 0 {
        let win_weak = win.as_weak();
        slint::Timer::single_shot(Duration::from_millis(16), move || {
            if let Some(win) = win_weak.upgrade() {
                center_settings_on_anchor_screen(&win, anchor, retries_left - 1);
            }
        });
    }
}

/// Asks malloc to return freed-but-cached pages to the OS. Called after the
/// calendar window is hidden (and its renderer destroyed) so the resident
/// footprint drops back towards the tray-only baseline.
#[cfg(target_os = "macos")]
pub fn release_malloc_pages() {
    extern "C" {
        fn malloc_zone_pressure_relief(zone: *mut std::ffi::c_void, goal: usize) -> usize;
    }
    unsafe {
        malloc_zone_pressure_relief(std::ptr::null_mut(), 0);
    }
}

#[cfg(not(target_os = "macos"))]
pub fn release_malloc_pages() {}

fn apply_year_month(main: &MainWindow, state: &Rc<State>, year: i32, month: u32) {
    state.focused_year.set(year);
    state.focused_month.set(month);
    let now = today();
    if year == now.year() && month == now.month() {
        *state.selected_date.borrow_mut() = None;
    } else {
        *state.selected_date.borrow_mut() = NaiveDate::from_ymd_opt(year, month, 1);
    }
    refresh_all(main, state);
}

pub fn refresh_all(main: &MainWindow, state: &Rc<State>) {
    state.last_refresh_date.set(today());
    refresh_grid(main, state);
    refresh_detail(main, state);
}

fn refresh_grid(main: &MainWindow, state: &Rc<State>) {
    let settings = state.settings.borrow().clone();
    let year = state.focused_year.get();
    let month = state.focused_month.get();
    let selected = *state.selected_date.borrow();

    let grid = build_month_grid(year, month, &settings, selected);

    let cells: Vec<DayCellData> = grid
        .days
        .iter()
        .map(|cell| DayCellData {
            date: cell.date.clone().into(),
            solar_day: cell.solar_day as i32,
            lunar_text: cell.lunar_text.clone().into(),
            lunar_kind: cell.lunar_text_kind.clone().into(),
            is_current_month: cell.is_current_month,
            is_visible: cell.is_current_month || cell.is_outside_visible,
            is_today: cell.is_today,
            is_selected: cell.is_selected,
            is_weekend: cell.is_weekend,
            workday_tag: cell.workday_tag.clone().unwrap_or_default().into(),
        })
        .collect();

    main.set_days(Rc::new(VecModel::from(cells)).into());
    main.set_grid_rows(grid.rows as i32);
    main.set_year_text(format!("{year}年").into());
    main.set_month_name(MONTH_NAMES_ZH[(month - 1) as usize].into());
    main.set_focused_year(year);
    main.set_focused_month(month as i32);
    main.set_current_year(today().year());

    let labels: Vec<WeekdayLabel> = if settings.sunday_first {
        ["日", "一", "二", "三", "四", "五", "六"]
            .iter()
            .enumerate()
            .map(|(i, t)| WeekdayLabel {
                text: (*t).into(),
                weekend: i == 0 || i == 6,
            })
            .collect()
    } else {
        ["一", "二", "三", "四", "五", "六", "日"]
            .iter()
            .enumerate()
            .map(|(i, t)| WeekdayLabel {
                text: (*t).into(),
                weekend: i >= 5,
            })
            .collect()
    };
    main.set_weekday_labels(Rc::new(VecModel::from(labels)).into());
}

fn refresh_detail(main: &MainWindow, state: &Rc<State>) {
    let settings = state.settings.borrow().clone();
    let date = state.selected_date.borrow().unwrap_or_else(today);
    let detail = build_day_detail(date, state.focused_year.get(), &settings);

    let changed = state.detail.borrow().as_ref() != Some(&detail);
    if changed {
        state.cycle_index.set(0);
    }

    main.set_hero_day(date.day().to_string().into());
    main.set_hero_weekday(
        HERO_WEEKDAYS[date.weekday().num_days_from_sunday() as usize].into(),
    );
    main.set_lunar_title(detail.lunar_date_title.clone().into());

    let mut relative = vec![detail.humanized_date.clone()];
    if let Some(alt) = detail.alternate_humanized.clone() {
        relative.push(alt);
    }

    *state.relative_texts.borrow_mut() = relative;

    let workday_line = match holiday::workday_tag(date.year(), date.month(), date.day()) {
        Some(tag) if tag == "休" => "法定假日",
        Some(_) => "调休上班",
        None => "",
    };
    main.set_workday_line(workday_line.into());

    let fit = textfit::fit_festivals(&detail.lunar_date_title, &detail.festivals);
    main.set_festivals_more(fit.more_count as i32);
    main.set_festivals_text(fit.visible_text.clone().into());
    *state.cycle_festivals.borrow_mut() = fit.cycle_festivals.clone();

    *state.detail.borrow_mut() = Some(detail);
    apply_cycle_texts(main, state);
}

fn apply_cycle_texts(main: &MainWindow, state: &Rc<State>) {
    let index = state.cycle_index.get();

    let detail = state.detail.borrow();
    let zodiac = detail
        .as_ref()
        .map(|d| d.zodiac.clone())
        .unwrap_or_default();

    let relative = state.relative_texts.borrow();
    let mut meta_parts: Vec<String> = Vec::new();
    if !zodiac.is_empty() {
        meta_parts.push(format!("{zodiac}年"));
    }
    if !relative.is_empty() {
        meta_parts.push(relative[index % relative.len()].clone());
    }
    main.set_meta_text(meta_parts.join(" · ").into());

    let cycle = state.cycle_festivals.borrow();
    if cycle.len() > 1 {
        main.set_festivals_text(cycle[index % cycle.len()].clone().into());
    }
}
