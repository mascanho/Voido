//! Optional weather, fetched from Open-Meteo (no API key, no signup).
//!
//! Enabled by the `weather` key in `config.toml` — a place name (`"Lisbon"`), a
//! `"lat,lon"` pair, or `"auto"` (IP-based). Unset / empty means no network call.
//! The fetch runs on a worker thread; see `App::maybe_refresh_weather`. A compact
//! form shows in the header / Overview; `^w` opens the full modal.

use std::time::Duration;

use chrono::{DateTime, Local, NaiveDate};
use serde::Deserialize;

const FORECAST: &str = "https://api.open-meteo.com/v1/forecast";
const GEOCODE: &str = "https://geocoding-api.open-meteo.com/v1/search";
const IPINFO: &str = "https://ipinfo.io/json";

/// Minimum gap between refreshes.
pub const REFRESH: Duration = Duration::from_secs(30 * 60);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unit {
    Celsius,
    Fahrenheit,
}

impl Unit {
    /// Read the `weather_unit` config value (`"c"` default, `"f"` for imperial).
    pub fn parse(raw: Option<&str>) -> Self {
        match raw.map(|s| s.trim().to_ascii_lowercase()).as_deref() {
            Some("f" | "fahrenheit" | "imperial" | "us") => Unit::Fahrenheit,
            _ => Unit::Celsius,
        }
    }

    fn temp_api(self) -> &'static str {
        match self {
            Unit::Celsius => "celsius",
            Unit::Fahrenheit => "fahrenheit",
        }
    }

    fn wind_api(self) -> &'static str {
        match self {
            Unit::Celsius => "kmh",
            Unit::Fahrenheit => "mph",
        }
    }

    fn precip_api(self) -> &'static str {
        match self {
            Unit::Celsius => "mm",
            Unit::Fahrenheit => "inch",
        }
    }

    pub fn deg(self) -> char {
        match self {
            Unit::Celsius => 'C',
            Unit::Fahrenheit => 'F',
        }
    }

    pub fn wind_label(self) -> &'static str {
        match self {
            Unit::Celsius => "km/h",
            Unit::Fahrenheit => "mph",
        }
    }

    pub fn precip_label(self) -> &'static str {
        match self {
            Unit::Celsius => "mm",
            Unit::Fahrenheit => "in",
        }
    }
}

/// Current conditions.
#[derive(Debug, Clone)]
pub struct Current {
    pub temp: f64,
    pub feels_like: f64,
    pub humidity: u8,
    pub pressure: f64,
    pub wind: f64,
    pub wind_dir: u16,
    pub wind_gust: f64,
    pub cloud_cover: u8,
    pub precip: f64,
    pub code: u8,
    pub is_day: bool,
}

/// One day of the short forecast.
#[derive(Debug, Clone)]
pub struct Day {
    pub date: NaiveDate,
    pub code: u8,
    pub t_max: f64,
    pub t_min: f64,
    pub precip_prob: Option<u8>,
    pub uv_max: Option<f64>,
    pub sunrise: String,
    pub sunset: String,
}

impl Day {
    pub fn glyph(&self) -> &'static str {
        code_glyph(self.code, true)
    }
}

/// A resolved snapshot: current conditions plus a 3-day outlook.
#[derive(Debug, Clone)]
pub struct Weather {
    pub place: String,
    pub unit: Unit,
    pub at: DateTime<Local>,
    pub current: Current,
    pub days: Vec<Day>,
}

impl Weather {
    /// Nerd Font weather glyph for the current sky.
    pub fn glyph(&self) -> &'static str {
        code_glyph(self.current.code, self.current.is_day)
    }

    /// Short human label, e.g. `light rain`.
    pub fn label(&self) -> &'static str {
        code_label(self.current.code)
    }

    /// `C` or `F`.
    pub fn deg(&self) -> char {
        self.unit.deg()
    }

    /// Rounded current temperature.
    pub fn temp_i(&self) -> i64 {
        self.current.temp.round() as i64
    }

    /// 8-point compass name for the current wind direction.
    pub fn wind_compass(&self) -> &'static str {
        compass(self.current.wind_dir)
    }
}

/// Blocking fetch — call from a worker thread.
pub fn fetch(location: &str, unit: Unit) -> Result<Weather, String> {
    let client = reqwest::blocking::Client::builder()
        .user_agent("voido-tui")
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| e.to_string())?;

    let (lat, lon, place) = resolve(&client, location)?;

    #[derive(Deserialize)]
    struct FcResp {
        current: RawCurrent,
        daily: RawDaily,
    }
    #[derive(Deserialize)]
    struct RawCurrent {
        temperature_2m: f64,
        apparent_temperature: f64,
        relative_humidity_2m: f64,
        surface_pressure: f64,
        weather_code: u8,
        wind_speed_10m: f64,
        wind_direction_10m: f64,
        wind_gusts_10m: f64,
        cloud_cover: f64,
        precipitation: f64,
        is_day: u8,
    }
    #[derive(Deserialize)]
    struct RawDaily {
        time: Vec<String>,
        weather_code: Vec<u8>,
        temperature_2m_max: Vec<f64>,
        temperature_2m_min: Vec<f64>,
        #[serde(default)]
        precipitation_probability_max: Vec<Option<u8>>,
        #[serde(default)]
        uv_index_max: Vec<Option<f64>>,
        #[serde(default)]
        sunrise: Vec<String>,
        #[serde(default)]
        sunset: Vec<String>,
    }

    let (lat_s, lon_s) = (lat.to_string(), lon.to_string());
    let resp: FcResp = client
        .get(FORECAST)
        .query(&[
            ("latitude", lat_s.as_str()),
            ("longitude", lon_s.as_str()),
            (
                "current",
                "temperature_2m,apparent_temperature,relative_humidity_2m,surface_pressure,\
                 weather_code,wind_speed_10m,wind_direction_10m,wind_gusts_10m,cloud_cover,\
                 precipitation,is_day",
            ),
            (
                "daily",
                "weather_code,temperature_2m_max,temperature_2m_min,\
                 precipitation_probability_max,uv_index_max,sunrise,sunset",
            ),
            ("forecast_days", "3"),
            ("temperature_unit", unit.temp_api()),
            ("wind_speed_unit", unit.wind_api()),
            ("precipitation_unit", unit.precip_api()),
            ("timezone", "auto"),
        ])
        .send()
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?
        .json()
        .map_err(|e| e.to_string())?;

    let c = resp.current;
    let d = resp.daily;
    let days = d
        .time
        .iter()
        .enumerate()
        .filter_map(|(i, t)| {
            let date = NaiveDate::parse_from_str(t, "%Y-%m-%d").ok()?;
            Some(Day {
                date,
                code: *d.weather_code.get(i)?,
                t_max: *d.temperature_2m_max.get(i)?,
                t_min: *d.temperature_2m_min.get(i)?,
                precip_prob: d.precipitation_probability_max.get(i).copied().flatten(),
                uv_max: d.uv_index_max.get(i).copied().flatten(),
                sunrise: d.sunrise.get(i).map(|s| hhmm(s)).unwrap_or_default(),
                sunset: d.sunset.get(i).map(|s| hhmm(s)).unwrap_or_default(),
            })
        })
        .collect();

    Ok(Weather {
        place,
        unit,
        at: Local::now(),
        current: Current {
            temp: c.temperature_2m,
            feels_like: c.apparent_temperature,
            humidity: c.relative_humidity_2m.round() as u8,
            pressure: c.surface_pressure,
            wind: c.wind_speed_10m,
            wind_dir: c.wind_direction_10m.round().rem_euclid(360.0) as u16,
            wind_gust: c.wind_gusts_10m,
            cloud_cover: c.cloud_cover.round() as u8,
            precip: c.precipitation,
            code: c.weather_code,
            is_day: c.is_day == 1,
        },
        days,
    })
}

/// `"2026-08-30T06:45"` → `"06:45"`.
fn hhmm(iso: &str) -> String {
    iso.split_once('T')
        .map(|(_, t)| t.chars().take(5).collect())
        .unwrap_or_else(|| iso.to_string())
}

/// Turn the config value into `(lat, lon, display name)`.
fn resolve(client: &reqwest::blocking::Client, location: &str) -> Result<(f64, f64, String), String> {
    let loc = location.trim();
    if loc.is_empty() || loc.eq_ignore_ascii_case("auto") {
        return ip_locate(client);
    }
    // A bare "lat,lon" pair (both halves parse as numbers).
    if let Some((a, b)) = loc.split_once(',')
        && let (Ok(lat), Ok(lon)) = (a.trim().parse::<f64>(), b.trim().parse::<f64>())
    {
        return Ok((lat, lon, format!("{lat:.2}, {lon:.2}")));
    }
    geocode(client, loc)
}

fn geocode(client: &reqwest::blocking::Client, name: &str) -> Result<(f64, f64, String), String> {
    #[derive(Deserialize)]
    struct GeoResp {
        results: Option<Vec<GeoHit>>,
    }
    #[derive(Deserialize)]
    struct GeoHit {
        latitude: f64,
        longitude: f64,
        name: String,
        #[serde(default)]
        country_code: Option<String>,
    }

    let resp: GeoResp = client
        .get(GEOCODE)
        .query(&[("name", name), ("count", "1"), ("language", "en")])
        .send()
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?
        .json()
        .map_err(|e| e.to_string())?;

    let hit = resp
        .results
        .and_then(|v| v.into_iter().next())
        .ok_or_else(|| format!("no place matched “{name}”"))?;
    let place = match hit.country_code {
        Some(cc) if !cc.is_empty() => format!("{}, {cc}", hit.name),
        _ => hit.name,
    };
    Ok((hit.latitude, hit.longitude, place))
}

fn ip_locate(client: &reqwest::blocking::Client) -> Result<(f64, f64, String), String> {
    #[derive(Deserialize)]
    struct IpResp {
        #[serde(default)]
        loc: Option<String>,
        #[serde(default)]
        city: Option<String>,
    }

    let resp: IpResp = client
        .get(IPINFO)
        .send()
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?
        .json()
        .map_err(|e| e.to_string())?;

    let loc = resp.loc.ok_or("IP lookup returned no coordinates")?;
    let (a, b) = loc.split_once(',').ok_or("IP lookup coordinates malformed")?;
    let lat: f64 = a.trim().parse().map_err(|_| "IP lookup latitude malformed")?;
    let lon: f64 = b.trim().parse().map_err(|_| "IP lookup longitude malformed")?;
    Ok((lat, lon, resp.city.unwrap_or_else(|| "current location".into())))
}

/// 8-point compass name for a bearing in degrees.
fn compass(deg: u16) -> &'static str {
    const POINTS: [&str; 8] = ["N", "NE", "E", "SE", "S", "SW", "W", "NW"];
    POINTS[(((deg as f64 + 22.5) / 45.0) as usize) % 8]
}

/// WMO weather-code → Nerd Font glyph (day/night aware for clear & partly-cloudy).
fn code_glyph(code: u8, day: bool) -> &'static str {
    match code {
        0 => {
            if day {
                "\u{e30d}" // wi-day-sunny
            } else {
                "\u{e32b}" // wi-night-clear
            }
        }
        1 | 2 => {
            if day {
                "\u{e302}" // wi-day-cloudy
            } else {
                "\u{e37e}" // wi-night-alt-cloudy
            }
        }
        3 => "\u{e33d}",                      // wi-cloudy
        45 | 48 => "\u{e313}",                // wi-fog
        51 | 53 | 55 | 56 | 57 => "\u{e319}", // wi-showers (drizzle)
        61 | 63 | 65 | 66 | 67 => "\u{e318}", // wi-rain
        71 | 73 | 75 | 77 => "\u{e31a}",      // wi-snow
        80..=82 => "\u{e318}",                // wi-rain (rain showers)
        85 | 86 => "\u{e31a}",                // wi-snow (snow showers)
        95 | 96 | 99 => "\u{e31d}",           // wi-thunderstorm
        _ => "\u{e374}",                      // wi-na
    }
}

fn code_label(code: u8) -> &'static str {
    match code {
        0 => "clear",
        1 => "mainly clear",
        2 => "partly cloudy",
        3 => "overcast",
        45 | 48 => "fog",
        51 => "light drizzle",
        53 => "drizzle",
        55 => "heavy drizzle",
        56 | 57 => "freezing drizzle",
        61 => "light rain",
        63 => "rain",
        65 => "heavy rain",
        66 | 67 => "freezing rain",
        71 => "light snow",
        73 => "snow",
        75 => "heavy snow",
        77 => "snow grains",
        80 => "light showers",
        81 => "showers",
        82 => "violent showers",
        85 => "light snow showers",
        86 => "snow showers",
        95 => "thunderstorm",
        96 | 99 => "thunderstorm w/ hail",
        _ => "—",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_parses_loosely() {
        assert_eq!(Unit::parse(None), Unit::Celsius);
        assert_eq!(Unit::parse(Some("c")), Unit::Celsius);
        assert_eq!(Unit::parse(Some("  F ")), Unit::Fahrenheit);
        assert_eq!(Unit::parse(Some("imperial")), Unit::Fahrenheit);
        assert_eq!(Unit::parse(Some("kelvin?")), Unit::Celsius);
    }

    #[test]
    fn glyphs_cover_the_code_families() {
        assert_ne!(code_glyph(0, true), code_glyph(0, false));
        assert_eq!(code_glyph(3, true), code_glyph(3, false));
        assert_eq!(code_glyph(200, true), code_glyph(255, true)); // unknown -> n/a
    }

    #[test]
    fn compass_wraps() {
        assert_eq!(compass(0), "N");
        assert_eq!(compass(90), "E");
        assert_eq!(compass(180), "S");
        assert_eq!(compass(350), "N");
        assert_eq!(compass(45), "NE");
    }

    #[test]
    fn hhmm_trims_iso() {
        assert_eq!(hhmm("2026-08-30T06:45"), "06:45");
        assert_eq!(hhmm("bogus"), "bogus");
    }
}
