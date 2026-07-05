use std::collections::{BTreeMap, VecDeque};

use terrane_cap_interface::{Error, Result};

pub const MAX_FIXES_PER_APP: usize = 20;
pub const COARSE_DEGREES_E7: i64 = 100_000;
pub const COARSE_MIN_ACCURACY_M: u32 = 1_000;
pub const RATE_LIMIT_MS: u64 = 10_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeoFix {
    pub lat_e7: i64,
    pub lon_e7: i64,
    pub accuracy_m: u32,
    pub precision: String,
    pub observed_at: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GeoState {
    pub fixes: BTreeMap<String, VecDeque<GeoFix>>,
    pub supports: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GeoPrecision {
    Exact,
    Coarse,
}

impl GeoPrecision {
    pub fn parse(raw: &str) -> Result<Self> {
        match raw {
            "exact" => Ok(Self::Exact),
            "coarse" => Ok(Self::Coarse),
            other => Err(Error::InvalidInput(format!(
                "geo precision must be exact or coarse, got {other:?}"
            ))),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Exact => "exact",
            Self::Coarse => "coarse",
        }
    }
}

pub fn round_for_precision(
    lat_e7: i64,
    lon_e7: i64,
    accuracy_m: u32,
    precision: GeoPrecision,
) -> (i64, i64, u32) {
    match precision {
        GeoPrecision::Exact => (lat_e7, lon_e7, accuracy_m),
        GeoPrecision::Coarse => (
            round_to_step(lat_e7, COARSE_DEGREES_E7),
            round_to_step(lon_e7, COARSE_DEGREES_E7),
            accuracy_m.max(COARSE_MIN_ACCURACY_M),
        ),
    }
}

pub fn validate_fix(lat_e7: i64, lon_e7: i64) -> Result<()> {
    if !(-900_000_000..=900_000_000).contains(&lat_e7) {
        return Err(Error::InvalidInput(format!(
            "latitude e7 must be between -900000000 and 900000000, got {lat_e7}"
        )));
    }
    if !(-1_800_000_000..=1_800_000_000).contains(&lon_e7) {
        return Err(Error::InvalidInput(format!(
            "longitude e7 must be between -1800000000 and 1800000000, got {lon_e7}"
        )));
    }
    Ok(())
}

pub fn fix_json(fix: &GeoFix) -> String {
    format!(
        r#"{{"lat_e7":{},"lon_e7":{},"accuracy_m":{},"precision":"{}","observed_at":{}}}"#,
        fix.lat_e7, fix.lon_e7, fix.accuracy_m, fix.precision, fix.observed_at
    )
}

fn round_to_step(value: i64, step: i64) -> i64 {
    if value >= 0 {
        ((value + step / 2) / step) * step
    } else {
        ((value - step / 2) / step) * step
    }
}
