#[cfg(target_os = "macos")]
mod macos {
    use std::cell::RefCell;
    use std::time::Duration;

    use objc2::rc::Retained;
    use objc2::runtime::{NSObjectProtocol, ProtocolObject};
    use objc2::{define_class, msg_send, ClassType, MainThreadMarker};
    use objc2_core_location::{
        kCLLocationAccuracyKilometer, CLError, CLAuthorizationStatus, CLLocation,
        CLLocationManager, CLLocationManagerDelegate,
    };
    use objc2_foundation::{NSArray, NSError};

    use super::LocationResult;
    use crate::services::weather;

    const MAX_LOCATION_ATTEMPTS: u32 = 6;
    const RETRY_DELAY_MS: u64 = 2000;
    const LOCATION_TIMEOUT_MS: u64 = 30_000;
    /// Last-known fixes older than this are ignored; they may predate a
    /// commute and would otherwise short-circuit a fresh location request.
    const MAX_FIX_AGE_SECS: f64 = 10.0 * 60.0;

    thread_local! {
        static PENDING_CALLBACK: RefCell<Option<Box<dyn FnOnce(LocationResult) + Send>>> =
            const { RefCell::new(None) };
        static LOCATION_MANAGER: RefCell<Option<Retained<CLLocationManager>>> =
            const { RefCell::new(None) };
        static LOCATION_DELEGATE: RefCell<Option<Retained<LocationDelegate>>> =
            const { RefCell::new(None) };
        static REQUEST_IN_FLIGHT: RefCell<bool> = const { RefCell::new(false) };
        static LOCATION_ATTEMPTS: RefCell<u32> = const { RefCell::new(0) };
    }

    fn finish(result: LocationResult) {
        REQUEST_IN_FLIGHT.with(|cell| *cell.borrow_mut() = false);
        LOCATION_ATTEMPTS.with(|cell| *cell.borrow_mut() = 0);
        PENDING_CALLBACK.with(|cell| {
            if let Some(callback) = cell.borrow_mut().take() {
                callback(result);
            }
        });
    }

    fn has_pending_callback() -> bool {
        PENDING_CALLBACK.with(|cell| cell.borrow().is_some())
    }

    fn store_coords(lat: f64, lon: f64) {
        weather::store_coordinates(lat, lon, weather::CoordSource::CoreLocation);
    }

    // No in-process cache on top: the disk cache carries the TTL (1h for
    // CoreLocation fixes), so a long-running tray process re-locates when
    // the user may have moved (e.g. home → office).
    fn cached_coords() -> Option<(f64, f64)> {
        weather::cached_coordinates()
    }

    fn current_authorization_status(manager: &CLLocationManager) -> CLAuthorizationStatus {
        unsafe { manager.authorizationStatus() }
    }

    fn is_authorized(status: CLAuthorizationStatus) -> bool {
        status == CLAuthorizationStatus::AuthorizedWhenInUse
            || status == CLAuthorizationStatus::AuthorizedAlways
    }

    fn location_services_enabled() -> bool {
        unsafe { CLLocationManager::locationServicesEnabled_class() }
    }

    fn location_error_message(error: &NSError) -> String {
        match CLError(error.code()) {
            CLError::Denied => "定位未授权".to_string(),
            CLError::LocationUnknown => "暂时无法定位".to_string(),
            CLError::Network => "定位网络异常".to_string(),
            _ => error.localizedDescription().to_string(),
        }
    }

    fn is_retryable_error(error: &NSError) -> bool {
        CLError(error.code()) == CLError::LocationUnknown
    }

    fn try_last_known_location(manager: &CLLocationManager) -> Option<(f64, f64)> {
        let location = unsafe { manager.location() }?;
        let age_secs = -unsafe { location.timestamp() }.timeIntervalSinceNow();
        if age_secs > MAX_FIX_AGE_SECS {
            return None;
        }
        let coord = unsafe { location.coordinate() };
        Some((coord.latitude, coord.longitude))
    }

    fn stop_location_updates(manager: &CLLocationManager) {
        unsafe {
            manager.stopUpdatingLocation();
        }
    }

    define_class!(
        #[unsafe(super(objc2::runtime::NSObject))]
        #[name = "TclcLocationDelegate"]
        struct LocationDelegate;

        unsafe impl NSObjectProtocol for LocationDelegate {}

        unsafe impl CLLocationManagerDelegate for LocationDelegate {
            #[unsafe(method(locationManager:didUpdateLocations:))]
            fn location_manager_did_update_locations(
                &self,
                manager: &CLLocationManager,
                locations: &NSArray<CLLocation>,
            ) {
                if let Some(location) = locations.lastObject() {
                    stop_location_updates(manager);
                    let coord = unsafe { location.coordinate() };
                    store_coords(coord.latitude, coord.longitude);
                    finish(Ok((coord.latitude, coord.longitude)));
                } else {
                    stop_location_updates(manager);
                    finish(Err("no location returned".to_string()));
                }
            }

            #[unsafe(method(locationManager:didFailWithError:))]
            fn location_manager_did_fail(
                &self,
                manager: &CLLocationManager,
                error: &NSError,
            ) {
                let attempts = LOCATION_ATTEMPTS.with(|cell| *cell.borrow());
                if is_retryable_error(error) && attempts < MAX_LOCATION_ATTEMPTS {
                    LOCATION_ATTEMPTS.with(|cell| *cell.borrow_mut() = attempts + 1);
                    eprintln!(
                        "location attempt {}/{} failed: {}",
                        attempts + 1,
                        MAX_LOCATION_ATTEMPTS,
                        location_error_message(error)
                    );
                    stop_location_updates(manager);
                    REQUEST_IN_FLIGHT.with(|cell| *cell.borrow_mut() = false);
                    schedule_retry();
                    return;
                }

                stop_location_updates(manager);

                if let Some(coords) = try_last_known_location(manager) {
                    store_coords(coords.0, coords.1);
                    finish(Ok(coords));
                    return;
                }

                finish(Err(location_error_message(error)));
            }

            #[unsafe(method(locationManagerDidChangeAuthorization:))]
            fn location_manager_did_change_authorization(&self, manager: &CLLocationManager) {
                if !has_pending_callback() {
                    return;
                }

                let status = unsafe { manager.authorizationStatus() };
                if is_authorized(status) {
                    try_begin_location_request(manager);
                } else if status == CLAuthorizationStatus::Denied
                    || status == CLAuthorizationStatus::Restricted
                {
                    finish(Err("定位未授权".to_string()));
                }
            }
        }
    );

    fn ensure_manager() -> Retained<CLLocationManager> {
        LOCATION_MANAGER.with(|cell| {
            if let Some(manager) = cell.borrow().as_ref() {
                return manager.clone();
            }

            let manager = unsafe {
                let manager = CLLocationManager::new();
                manager.setDesiredAccuracy(kCLLocationAccuracyKilometer);
                manager
            };

            let delegate: Retained<LocationDelegate> =
                unsafe { msg_send![LocationDelegate::class(), new] };
            let delegate_proto: &ProtocolObject<dyn CLLocationManagerDelegate> =
                ProtocolObject::from_ref(&*delegate);
            unsafe {
                manager.setDelegate(Some(delegate_proto));
            }

            *cell.borrow_mut() = Some(manager.clone());
            LOCATION_DELEGATE.with(|d| *d.borrow_mut() = Some(delegate));
            manager
        })
    }

    fn schedule_retry() {
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(RETRY_DELAY_MS));
            let _ = slint::invoke_from_event_loop(move || {
                if !has_pending_callback() {
                    return;
                }
                let manager = ensure_manager();
                if let Some(coords) = try_last_known_location(&manager) {
                    store_coords(coords.0, coords.1);
                    finish(Ok(coords));
                    return;
                }
                REQUEST_IN_FLIGHT.with(|cell| *cell.borrow_mut() = true);
                unsafe {
                    manager.startUpdatingLocation();
                    manager.requestLocation();
                }
            });
        });
    }

    fn schedule_timeout() {
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(LOCATION_TIMEOUT_MS));
            let _ = slint::invoke_from_event_loop(move || {
                if !REQUEST_IN_FLIGHT.with(|cell| *cell.borrow()) || !has_pending_callback() {
                    return;
                }

                let manager = ensure_manager();
                stop_location_updates(&manager);
                REQUEST_IN_FLIGHT.with(|cell| *cell.borrow_mut() = false);

                if let Some(coords) = try_last_known_location(&manager) {
                    store_coords(coords.0, coords.1);
                    finish(Ok(coords));
                    return;
                }

                finish(Err("暂时无法定位".to_string()));
            });
        });
    }

    fn try_begin_location_request(manager: &CLLocationManager) {
        if REQUEST_IN_FLIGHT.with(|cell| *cell.borrow()) {
            return;
        }

        if let Some(coords) = try_last_known_location(manager) {
            store_coords(coords.0, coords.1);
            finish(Ok(coords));
            return;
        }

        REQUEST_IN_FLIGHT.with(|cell| *cell.borrow_mut() = true);
        LOCATION_ATTEMPTS.with(|cell| *cell.borrow_mut() = 1);
        schedule_timeout();
        unsafe {
            manager.startUpdatingLocation();
            manager.requestLocation();
        }
    }

    fn run_on_main(f: impl FnOnce() + Send + 'static) {
        if MainThreadMarker::new().is_some() {
            f();
        } else {
            let _ = slint::invoke_from_event_loop(f);
        }
    }

    fn request_on_main_thread(callback: Box<dyn FnOnce(LocationResult) + Send>) {
        let Some(_mtm) = MainThreadMarker::new() else {
            callback(Err("location must run on main thread".to_string()));
            return;
        };

        if let Some(coords) = cached_coords() {
            callback(Ok(coords));
            return;
        }

        // Register the callback before creating the manager so delegate callbacks
        // cannot fire with no listener attached.
        PENDING_CALLBACK.with(|cell| {
            *cell.borrow_mut() = Some(callback);
        });

        if !location_services_enabled() {
            finish(Err("系统定位服务未开启".to_string()));
            return;
        }

        let manager = ensure_manager();
        let status = current_authorization_status(&manager);
        if status == CLAuthorizationStatus::Denied || status == CLAuthorizationStatus::Restricted {
            finish(Err("定位未授权".to_string()));
            return;
        }

        if status == CLAuthorizationStatus::NotDetermined {
            unsafe {
                manager.requestWhenInUseAuthorization();
            }
            return;
        }

        try_begin_location_request(&manager);
    }

    pub fn request_coordinates_async(callback: impl FnOnce(LocationResult) + Send + 'static) {
        run_on_main(move || request_on_main_thread(Box::new(callback)));
    }
}

pub type LocationResult = Result<(f64, f64), String>;

#[cfg(target_os = "macos")]
pub use macos::request_coordinates_async as request_coordinates;

#[cfg(not(target_os = "macos"))]
pub fn request_coordinates(callback: impl FnOnce(LocationResult) + Send + 'static) {
    callback(Err("location only supported on macOS".to_string()));
}
