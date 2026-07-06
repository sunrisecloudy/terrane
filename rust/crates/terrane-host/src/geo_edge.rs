#[cfg(target_os = "macos")]
mod platform {
    use std::sync::{Arc, Condvar, Mutex};
    use std::time::{Duration, Instant, SystemTime};

    use objc2::rc::Retained;
    use objc2::runtime::ProtocolObject;
    use objc2::{define_class, msg_send, AnyThread, DefinedClass};
    use objc2_core_location::{
        CLAuthorizationStatus, CLLocation, CLLocationManager, CLLocationManagerDelegate,
    };
    use objc2_foundation::{NSArray, NSDate, NSError, NSObject, NSObjectProtocol, NSRunLoop};
    use terrane_cap_geo::{observed_event, round_for_precision, GeoPrecision};
    use terrane_core::{Error, EventRecord, Result};

    const GEO_TIMEOUT: Duration = Duration::from_secs(15);
    const RUN_LOOP_TICK: f64 = 0.05;

    #[derive(Debug, Clone)]
    struct NativeFix {
        lat_e7: i64,
        lon_e7: i64,
        accuracy_m: u32,
    }

    #[derive(Debug, Clone)]
    enum LocationResult {
        Fix(NativeFix),
        Failed(String),
    }

    #[derive(Clone, Default)]
    struct SharedState {
        result: Arc<(Mutex<Option<LocationResult>>, Condvar)>,
    }

    #[derive(Clone)]
    struct Ivars {
        state: SharedState,
    }

    define_class!(
        #[unsafe(super(NSObject))]
        #[ivars = Ivars]
        struct TerraneGeoDelegate;

        unsafe impl NSObjectProtocol for TerraneGeoDelegate {}

        #[allow(non_snake_case)]
        unsafe impl CLLocationManagerDelegate for TerraneGeoDelegate {
            #[unsafe(method(locationManager:didUpdateLocations:))]
            fn locationManager_didUpdateLocations(
                &self,
                manager: &CLLocationManager,
                locations: &NSArray<CLLocation>,
            ) {
                let result = locations
                    .lastObject()
                    .ok_or_else(|| "CoreLocation returned no locations".to_string())
                    .and_then(|location| native_fix_from_location(&location));
                unsafe {
                    manager.stopUpdatingLocation();
                }
                self.ivars().state.finish(match result {
                    Ok(fix) => LocationResult::Fix(fix),
                    Err(error) => LocationResult::Failed(error),
                });
            }

            #[unsafe(method(locationManager:didFailWithError:))]
            fn locationManager_didFailWithError(
                &self,
                manager: &CLLocationManager,
                error: &NSError,
            ) {
                unsafe {
                    manager.stopUpdatingLocation();
                }
                self.ivars().state.finish(LocationResult::Failed(format!(
                    "CoreLocation failed: {}",
                    error.localizedDescription()
                )));
            }

            #[unsafe(method(locationManagerDidChangeAuthorization:))]
            fn locationManagerDidChangeAuthorization(&self, manager: &CLLocationManager) {
                let status = unsafe { manager.authorizationStatus() };
                if is_authorized(status) {
                    unsafe {
                        manager.requestLocation();
                    }
                } else if status == CLAuthorizationStatus::Denied
                    || status == CLAuthorizationStatus::Restricted
                {
                    self.ivars().state.finish(LocationResult::Failed(format!(
                        "CoreLocation authorization is {}",
                        authorization_name(status)
                    )));
                }
            }
        }
    );

    impl TerraneGeoDelegate {
        fn new(state: SharedState) -> Retained<Self> {
            let this = Self::alloc().set_ivars(Ivars { state });
            unsafe { msg_send![super(this), init] }
        }
    }

    impl SharedState {
        fn finish(&self, result: LocationResult) {
            let (lock, condvar) = &*self.result;
            let mut slot = match lock.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            if slot.is_none() {
                *slot = Some(result);
                condvar.notify_all();
            }
        }

        fn take(&self) -> Option<LocationResult> {
            let (lock, _) = &*self.result;
            match lock.lock() {
                Ok(mut guard) => guard.take(),
                Err(poisoned) => poisoned.into_inner().take(),
            }
        }
    }

    pub fn supports() -> bool {
        true
    }

    pub fn locate(app: &str, precision: &str) -> Result<Vec<EventRecord>> {
        let precision = GeoPrecision::parse(precision)?;
        let native = request_native_fix()?;
        let (lat_e7, lon_e7, accuracy_m) =
            round_for_precision(native.lat_e7, native.lon_e7, native.accuracy_m, precision);
        let observed_at = terrane_cap_time::system_time_to_epoch_ms(SystemTime::now())?;
        Ok(vec![observed_event(
            app,
            lat_e7,
            lon_e7,
            accuracy_m,
            precision.as_str(),
            observed_at,
        )?])
    }

    fn request_native_fix() -> Result<NativeFix> {
        if !unsafe { CLLocationManager::locationServicesEnabled_class() } {
            return Err(Error::Runtime(
                "CoreLocation location services are disabled".into(),
            ));
        }

        let manager = unsafe { CLLocationManager::new() };
        let state = SharedState::default();
        let delegate = TerraneGeoDelegate::new(state.clone());
        let delegate_ref: &ProtocolObject<dyn CLLocationManagerDelegate> =
            ProtocolObject::from_ref(&*delegate);

        unsafe {
            manager.setDelegate(Some(delegate_ref));
            manager.setDesiredAccuracy(100.0);
            manager.setPausesLocationUpdatesAutomatically(true);
        }

        let status = unsafe { manager.authorizationStatus() };
        if status == CLAuthorizationStatus::Denied || status == CLAuthorizationStatus::Restricted {
            return Err(Error::Runtime(format!(
                "CoreLocation authorization is {}",
                authorization_name(status)
            )));
        }

        unsafe {
            if status == CLAuthorizationStatus::NotDetermined {
                manager.requestWhenInUseAuthorization();
            } else {
                manager.requestLocation();
            }
        }

        let deadline = Instant::now() + GEO_TIMEOUT;
        while Instant::now() < deadline {
            if let Some(result) = state.take() {
                unsafe {
                    manager.setDelegate(None);
                }
                return match result {
                    LocationResult::Fix(fix) => Ok(fix),
                    LocationResult::Failed(error) => Err(Error::Runtime(error)),
                };
            }
            NSRunLoop::currentRunLoop().runUntilDate(&NSDate::dateWithTimeIntervalSinceNow(
                RUN_LOOP_TICK,
            ));
        }

        unsafe {
            manager.stopUpdatingLocation();
            manager.setDelegate(None);
        }
        Err(Error::Runtime(
            "CoreLocation timed out before returning a location fix".into(),
        ))
    }

    fn native_fix_from_location(location: &CLLocation) -> std::result::Result<NativeFix, String> {
        let coordinate = unsafe { location.coordinate() };
        if !unsafe { coordinate.is_valid() } {
            return Err("CoreLocation returned an invalid coordinate".to_string());
        }
        let lat_e7 = degrees_to_e7(coordinate.latitude)?;
        let lon_e7 = degrees_to_e7(coordinate.longitude)?;
        let accuracy = unsafe { location.horizontalAccuracy() };
        let accuracy_m = if accuracy.is_finite() && accuracy > 0.0 {
            accuracy.ceil().min(u32::MAX as f64) as u32
        } else {
            0
        };
        Ok(NativeFix {
            lat_e7,
            lon_e7,
            accuracy_m,
        })
    }

    fn degrees_to_e7(value: f64) -> std::result::Result<i64, String> {
        if !value.is_finite() {
            return Err("CoreLocation returned a non-finite coordinate".to_string());
        }
        Ok((value * 10_000_000.0).round() as i64)
    }

    fn is_authorized(status: CLAuthorizationStatus) -> bool {
        status == CLAuthorizationStatus::AuthorizedAlways
            || status == CLAuthorizationStatus::AuthorizedWhenInUse
    }

    fn authorization_name(status: CLAuthorizationStatus) -> &'static str {
        if status == CLAuthorizationStatus::NotDetermined {
            "notDetermined"
        } else if status == CLAuthorizationStatus::Restricted {
            "restricted"
        } else if status == CLAuthorizationStatus::Denied {
            "denied"
        } else if status == CLAuthorizationStatus::AuthorizedAlways {
            "authorizedAlways"
        } else if status == CLAuthorizationStatus::AuthorizedWhenInUse {
            "authorizedWhenInUse"
        } else {
            "unknown"
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    use terrane_core::{Error, EventRecord, Result};

    pub fn supports() -> bool {
        false
    }

    pub fn locate(_app: &str, _precision: &str) -> Result<Vec<EventRecord>> {
        Err(Error::Runtime(
            "geo.locate is not supported by the CLI host edge; use a host with OS/browser geolocation"
                .into(),
        ))
    }
}

pub fn supports() -> bool {
    platform::supports()
}

pub fn locate(app: &str, precision: &str) -> terrane_core::Result<Vec<terrane_core::EventRecord>> {
    platform::locate(app, precision)
}
