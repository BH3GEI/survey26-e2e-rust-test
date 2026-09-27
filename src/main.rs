//! Deterministic Agent Observer entry, pure Rust, no dependencies.
//! Speaks participant-agent-protocol-v1/v2 over JSON-Lines stdin/stdout.

use std::collections::{HashMap, HashSet};
use std::io::{self, BufRead, Write};

// ---------------------------------------------------------------- JSON ------

#[derive(Clone, Debug)]
enum J {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<J>),
    Obj(Vec<(String, J)>),
}

impl J {
    fn get(&self, key: &str) -> Option<&J> {
        match self {
            J::Obj(m) => m.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }
    fn num(&self) -> Option<f64> {
        match self {
            J::Num(n) => Some(*n),
            J::Str(s) => s.trim().parse::<f64>().ok(),
            J::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
            _ => None,
        }
    }
    fn string(&self) -> Option<&str> {
        match self {
            J::Str(s) => Some(s),
            _ => None,
        }
    }
    fn truthy(&self) -> bool {
        matches!(self, J::Bool(true))
    }
    fn arr(&self) -> &[J] {
        match self {
            J::Arr(a) => a,
            _ => &[],
        }
    }
    fn is_object(&self) -> bool {
        matches!(self, J::Obj(_))
    }
}

struct Parser<'a> {
    b: &'a [u8],
    i: usize,
}

impl<'a> Parser<'a> {
    fn ws(&mut self) {
        while self.i < self.b.len() && matches!(self.b[self.i], b' ' | b'\t' | b'\n' | b'\r') {
            self.i += 1;
        }
    }
    fn lit(&mut self, word: &str) -> Result<(), String> {
        if self.b.len() - self.i >= word.len() && &self.b[self.i..self.i + word.len()] == word.as_bytes() {
            self.i += word.len();
            Ok(())
        } else {
            Err(format!("invalid literal at {}", self.i))
        }
    }
    fn value(&mut self) -> Result<J, String> {
        self.ws();
        match self.b.get(self.i) {
            Some(b'{') => self.object(),
            Some(b'[') => self.array(),
            Some(b'"') => Ok(J::Str(self.string()?)),
            Some(b't') => {
                self.lit("true")?;
                Ok(J::Bool(true))
            }
            Some(b'f') => {
                self.lit("false")?;
                Ok(J::Bool(false))
            }
            Some(b'n') => {
                self.lit("null")?;
                Ok(J::Null)
            }
            Some(_) => self.number(),
            None => Err("unexpected end of input".into()),
        }
    }
    fn number(&mut self) -> Result<J, String> {
        let start = self.i;
        while self.i < self.b.len()
            && matches!(self.b[self.i], b'0'..=b'9' | b'-' | b'+' | b'.' | b'e' | b'E')
        {
            self.i += 1;
        }
        let text = std::str::from_utf8(&self.b[start..self.i]).map_err(|e| e.to_string())?;
        text.parse::<f64>().map(J::Num).map_err(|e| format!("bad number {text:?}: {e}"))
    }
    fn string(&mut self) -> Result<String, String> {
        if self.b.get(self.i) != Some(&b'"') {
            return Err(format!("expected string at {}", self.i));
        }
        self.i += 1;
        let mut out = String::new();
        loop {
            let Some(&c) = self.b.get(self.i) else {
                return Err("unterminated string".into());
            };
            self.i += 1;
            match c {
                b'"' => return Ok(out),
                b'\\' => {
                    let Some(&esc) = self.b.get(self.i) else {
                        return Err("unterminated escape".into());
                    };
                    self.i += 1;
                    match esc {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{0008}'),
                        b'f' => out.push('\u{000C}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let cp = self.hex4()?;
                            if (0xD800..0xDC00).contains(&cp) {
                                if self.b.get(self.i) == Some(&b'\\') && self.b.get(self.i + 1) == Some(&b'u') {
                                    self.i += 2;
                                    let lo = self.hex4()?;
                                    let ch = 0x10000 + ((cp - 0xD800) << 10) + (lo - 0xDC00);
                                    out.push(char::from_u32(ch).unwrap_or('\u{FFFD}'));
                                } else {
                                    out.push('\u{FFFD}');
                                }
                            } else {
                                out.push(char::from_u32(cp).unwrap_or('\u{FFFD}'));
                            }
                        }
                        other => return Err(format!("bad escape \\{other}")),
                    }
                }
                _ => {
                    let len = utf8_len(c);
                    let start = self.i - 1;
                    let end = start + len;
                    if end > self.b.len() {
                        return Err("truncated utf-8".into());
                    }
                    self.i = end;
                    out.push_str(std::str::from_utf8(&self.b[start..end]).map_err(|e| e.to_string())?);
                }
            }
        }
    }
    fn hex4(&mut self) -> Result<u32, String> {
        if self.i + 4 > self.b.len() {
            return Err("truncated \\u escape".into());
        }
        let text = std::str::from_utf8(&self.b[self.i..self.i + 4]).map_err(|e| e.to_string())?;
        self.i += 4;
        u32::from_str_radix(text, 16).map_err(|e| e.to_string())
    }
    fn object(&mut self) -> Result<J, String> {
        self.i += 1; // {
        let mut out = Vec::new();
        self.ws();
        if self.b.get(self.i) == Some(&b'}') {
            self.i += 1;
            return Ok(J::Obj(out));
        }
        loop {
            self.ws();
            let key = self.string()?;
            self.ws();
            if self.b.get(self.i) != Some(&b':') {
                return Err(format!("expected ':' at {}", self.i));
            }
            self.i += 1;
            let val = self.value()?;
            out.push((key, val));
            self.ws();
            match self.b.get(self.i) {
                Some(b',') => {
                    self.i += 1;
                }
                Some(b'}') => {
                    self.i += 1;
                    return Ok(J::Obj(out));
                }
                _ => return Err(format!("expected ',' or '}}' at {}", self.i)),
            }
        }
    }
    fn array(&mut self) -> Result<J, String> {
        self.i += 1; // [
        let mut out = Vec::new();
        self.ws();
        if self.b.get(self.i) == Some(&b']') {
            self.i += 1;
            return Ok(J::Arr(out));
        }
        loop {
            let val = self.value()?;
            out.push(val);
            self.ws();
            match self.b.get(self.i) {
                Some(b',') => {
                    self.i += 1;
                }
                Some(b']') => {
                    self.i += 1;
                    return Ok(J::Arr(out));
                }
                _ => return Err(format!("expected ',' or ']' at {}", self.i)),
            }
        }
    }
}

fn utf8_len(first: u8) -> usize {
    if first < 0x80 {
        1
    } else if first < 0xE0 {
        2
    } else if first < 0xF0 {
        3
    } else {
        4
    }
}

fn parse_json(text: &str) -> Result<J, String> {
    let mut p = Parser { b: text.as_bytes(), i: 0 };
    let v = p.value()?;
    p.ws();
    Ok(v)
}

fn escape_json_string(s: &str, out: &mut String) {
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

// ------------------------------------------------------------- time ---------

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// Parse an ISO-8601 timestamp into epoch seconds (f64).
fn parse_utc(s: Option<&J>) -> Option<f64> {
    let s = s?.string()?;
    let b = s.as_bytes();
    if b.len() < 19 {
        return None;
    }
    let year: i64 = s.get(0..4)?.parse().ok()?;
    if b[4] != b'-' || b[7] != b'-' {
        return None;
    }
    let month: i64 = s.get(5..7)?.parse().ok()?;
    let day: i64 = s.get(8..10)?.parse().ok()?;
    let hour: i64 = s.get(11..13)?.parse().ok()?;
    let minute: i64 = s.get(14..16)?.parse().ok()?;
    let second: i64 = s.get(17..19)?.parse().ok()?;
    let mut idx = 19usize;
    let mut frac = 0.0f64;
    if b.get(idx) == Some(&b'.') {
        let mut end = idx + 1;
        while end < b.len() && b[end].is_ascii_digit() {
            end += 1;
        }
        frac = format!("0.{}", &s[idx + 1..end]).parse().unwrap_or(0.0);
        idx = end;
    }
    let mut offset = 0.0f64;
    match b.get(idx) {
        Some(b'Z') | None => {}
        Some(b'+') | Some(b'-') => {
            let sign = if b[idx] == b'-' { -1.0 } else { 1.0 };
            let oh: f64 = s.get(idx + 1..idx + 3)?.parse().ok()?;
            let om: f64 = s.get(idx + 4..idx + 6)?.parse().ok()?;
            offset = sign * (oh * 3600.0 + om * 60.0);
        }
        _ => {}
    }
    let days = days_from_civil(year, month, day);
    Some(days as f64 * 86400.0 + (hour * 3600 + minute * 60 + second) as f64 + frac - offset)
}

// ---------------------------------------------------------- contract --------

struct Contract {
    dark: f64,
    bright: f64,
    bonus_dark: f64,
    bonus_bright: f64,
    bonus_backup: f64,
    required_miss: f64,
    flexible_shortfall: f64,
    flexible_quota: i64,
    airmass_exponent: f64,
    max_weather_quality: f64,
}

impl Contract {
    fn from_initialize(init: &J) -> Contract {
        let d = Contract {
            dark: 0.65,
            bright: 0.40,
            bonus_dark: 0.25,
            bonus_bright: 0.15,
            bonus_backup: 0.08,
            required_miss: 1000.0,
            flexible_shortfall: 100.0,
            flexible_quota: 4,
            airmass_exponent: 1.0,
            max_weather_quality: 1.0,
        };
        let Some(contract) = init.get("scoring_contract") else { return d };
        let Some(score_config) = contract.get("score_config") else { return d };
        let num = |node: Option<&J>, fallback: f64| node.and_then(J::num).unwrap_or(fallback);
        let thresholds = score_config.get("quality_thresholds");
        let bonuses = score_config.get("program_bonus");
        let penalties = score_config.get("penalties");
        let weather = contract.get("weather_score_interface");
        Contract {
            dark: num(thresholds.and_then(|t| t.get("dark")), d.dark),
            bright: num(thresholds.and_then(|t| t.get("bright")), d.bright),
            bonus_dark: num(bonuses.and_then(|t| t.get("DARK")), d.bonus_dark),
            bonus_bright: num(bonuses.and_then(|t| t.get("BRIGHT")), d.bonus_bright),
            bonus_backup: num(bonuses.and_then(|t| t.get("BACKUP")), d.bonus_backup),
            required_miss: num(penalties.and_then(|t| t.get("required_miss")), d.required_miss),
            flexible_shortfall: num(
                penalties.and_then(|t| t.get("flexible_shortfall_per_tile")),
                d.flexible_shortfall,
            ),
            flexible_quota: score_config
                .get("flexible_quota_per_region")
                .and_then(J::num)
                .unwrap_or(d.flexible_quota as f64) as i64,
            airmass_exponent: num(weather.and_then(|t| t.get("airmass_exponent")), d.airmass_exponent),
            max_weather_quality: num(
                weather.and_then(|t| t.get("maximum_weather_quality")),
                d.max_weather_quality,
            ),
        }
    }
    fn band(&self, quality: f64) -> &'static str {
        if quality >= self.dark {
            "DARK"
        } else if quality >= self.bright {
            "BRIGHT"
        } else {
            "BACKUP"
        }
    }
    fn bonus(&self, band: &str) -> f64 {
        match band {
            "DARK" => self.bonus_dark,
            "BRIGHT" => self.bonus_bright,
            _ => self.bonus_backup,
        }
    }
}

// ----------------------------------------------------------- preview --------

#[derive(Clone)]
struct Preview {
    tile_id: String,
    program: String,
    request_id: String,
    region_id: String,
    scheduling_class: String,
    exposure: f64,
    combined_quality: f64,
    quality_band: String,
    tile_science_value: f64,
    total_gain: f64,
    gain_per_second: f64,
}

fn round6(x: f64) -> f64 {
    (x * 1.0e6).round() / 1.0e6
}
fn round9(x: f64) -> f64 {
    (x * 1.0e9).round() / 1.0e9
}

fn preview_actions(snapshot: &J, contract: &Contract, bests: Option<&HashMap<String, f64>>) -> Vec<Preview> {
    let mut result: Vec<Preview> = Vec::new();
    let flexible_progress = snapshot.get("progress").and_then(|p| p.get("flexible_completed_by_region"));
    let cursor_start = parse_utc(snapshot.get("cursor").and_then(|c| c.get("timestamp_utc")));
    for candidate in snapshot.get("candidate_tiles").map(J::arr).unwrap_or(&[]) {
        let weather = candidate.get("effective_weather");
        let observable = weather.and_then(|w| w.get("is_observable")).map(J::truthy).unwrap_or(false);
        if !observable {
            continue;
        }
        let exposure = candidate.get("nominal_exptime_seconds").and_then(J::num).unwrap_or(900.0);
        if exposure <= 0.0 {
            continue;
        }
        let window_end = parse_utc(candidate.get("window_end_utc"));
        match (cursor_start, window_end) {
            (Some(start), Some(end)) => {
                if start + exposure > end + 1e-9 {
                    continue;
                }
            }
            _ => continue,
        }
        let tile_id = match candidate.get("tile_id").and_then(J::string) {
            Some(t) => t.to_string(),
            None => continue,
        };
        let already_completed = candidate.get("already_completed").map(J::truthy).unwrap_or(false);

        let mut request_options: Vec<(String, f64)> = Vec::new();
        for request in snapshot.get("active_requests").map(J::arr).unwrap_or(&[]) {
            if request.get("is_complete").map(J::truthy).unwrap_or(false) {
                continue;
            }
            let mut matched = false;
            for req_tile in request.get("tile_requirements").map(J::arr).unwrap_or(&[]) {
                let rt = req_tile.get("tile_id").and_then(J::string).unwrap_or("");
                let remaining = req_tile
                    .get("remaining_visits")
                    .or_else(|| req_tile.get("required_visits"))
                    .and_then(J::num)
                    .unwrap_or(0.0);
                if rt == tile_id && remaining > 0.0 {
                    matched = true;
                    break;
                }
            }
            if !matched {
                continue;
            }
            let required_tile_count = request.get("required_tile_count").and_then(J::num).unwrap_or(1.0);
            let satisfied = request.get("satisfied_tile_count").and_then(J::num).unwrap_or(0.0);
            let remaining_tiles = (required_tile_count - satisfied).max(1.0);
            let delta = request.get("completion_reward").and_then(J::num).unwrap_or(0.0)
                + request.get("miss_penalty").and_then(J::num).unwrap_or(0.0);
            let request_id = request.get("request_id").and_then(J::string).unwrap_or("").to_string();
            request_options.push((request_id, delta / remaining_tiles));
        }
        if already_completed && bests.is_none() && request_options.is_empty() {
            continue;
        }
        let action_options: Vec<(String, f64)> = if request_options.is_empty() {
            vec![(String::new(), 0.0)]
        } else {
            request_options
        };

        let w = weather.unwrap();
        let transparency = w.get("transparency").and_then(J::num).unwrap_or(1.0);
        let sky = w.get("sky_quality").and_then(J::num).unwrap_or(1.0);
        let seeing = w.get("seeing_arcsec").and_then(J::num).unwrap_or(1.0);
        let geometry = candidate.get("geometry");
        let airmass = geometry.and_then(|g| g.get("airmass")).and_then(J::num).unwrap_or(1.0);
        if airmass <= 0.0 || seeing <= 0.0 {
            continue;
        }
        let lunar = geometry.and_then(|g| g.get("lunar_quality_factor")).and_then(J::num).unwrap_or(1.0);
        let mut atmospheric = transparency * sky / (seeing * airmass.powf(contract.airmass_exponent));
        if atmospheric > contract.max_weather_quality {
            atmospheric = contract.max_weather_quality;
        }
        let combined = atmospheric * lunar;
        let band = contract.band(combined);
        let tile_value = candidate.get("tile_science_value").and_then(J::num).unwrap_or(0.0);
        let potential = tile_value * combined * (1.0 + contract.bonus(band));
        let science = if already_completed {
            match bests.and_then(|m| m.get(&tile_id)) {
                Some(banked) => (potential - banked).max(0.0),
                None => 0.0,
            }
        } else {
            potential
        };
        let region_id = candidate.get("region_id").and_then(J::string).unwrap_or("").to_string();
        let scheduling_class = candidate.get("scheduling_class").and_then(J::string).unwrap_or("").to_string();
        let mut avoidance = 0.0;
        if !already_completed && scheduling_class == "REQUIRED" {
            avoidance = contract.required_miss;
        } else if !already_completed && scheduling_class == "FLEXIBLE" {
            let done = flexible_progress
                .and_then(|p| p.get(&region_id))
                .and_then(J::num)
                .unwrap_or(0.0) as i64;
            if done < contract.flexible_quota {
                avoidance = contract.flexible_shortfall;
            }
        }
        let combined_r = round6(combined);
        let tile_value_r = round6(tile_value);
        for (request_id, request_value) in action_options {
            let total = round6(science + avoidance + request_value);
            result.push(Preview {
                tile_id: tile_id.clone(),
                program: band.to_string(),
                request_id,
                region_id: region_id.clone(),
                scheduling_class: scheduling_class.clone(),
                exposure,
                combined_quality: combined_r,
                quality_band: band.to_string(),
                tile_science_value: tile_value_r,
                total_gain: total,
                gain_per_second: round9(total / exposure),
            });
        }
    }
    result.sort_by(|a, b| {
        b.gain_per_second
            .partial_cmp(&a.gain_per_second)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(b.total_gain.partial_cmp(&a.total_gain).unwrap_or(std::cmp::Ordering::Equal))
            .then(a.tile_id.cmp(&b.tile_id))
            .then(a.request_id.cmp(&b.request_id))
            .then(a.program.cmp(&b.program))
    });
    result
}

// ---------------------------------------------------------- detector --------

const NOVA_MIN: f64 = 1.30;
const NOVA_MAX: f64 = 1.65;
const REDD_MIN: f64 = 0.70;
const REDD_MAX: f64 = 0.87;
const FAULT_MAX: f64 = 0.60;
const FAULT_MIN_EVIDENCE: usize = 2;
const FAULT_WINDOW: usize = 6;
const TAG_MIN_READS: usize = 5;
const TAG_MIN_FRACTION: f64 = 0.8;

#[derive(Clone)]
enum Report {
    Tag { kind: &'static str, tile_id: String },
    Fault,
}

struct Detector {
    bests: HashMap<String, f64>,
    pending: Option<(String, f64, bool)>,
    last_feedback: Option<(String, u64)>,
    reported: HashSet<(String, &'static str)>,
    tag_reads: HashMap<String, Vec<(Option<&'static str>, String)>>,
    recent_ratios: Vec<f64>,
    fault_pending: bool,
    fault_scope: Option<(String, J)>,
    fault_repair_until: Option<f64>,
    forecasts: Vec<J>,
    tile_coords: HashMap<String, (f64, f64)>,
    contract_bonus: (f64, f64, f64),
}

impl Detector {
    fn new(init: &J, contract: &Contract) -> Detector {
        let mut coords = HashMap::new();
        if let Some(tiles) = init.get("tile_catalog").and_then(|c| c.get("tiles")) {
            for tile in tiles.arr() {
                let id = tile.get("tile_id").and_then(J::string).unwrap_or("");
                let ra = tile.get("ra_deg").and_then(J::num);
                let dec = tile.get("dec_deg").and_then(J::num);
                if let (Some(ra), Some(dec)) = (ra, dec) {
                    coords.insert(id.to_string(), (ra, dec));
                }
            }
        }
        Detector {
            bests: HashMap::new(),
            pending: None,
            last_feedback: None,
            reported: HashSet::new(),
            tag_reads: HashMap::new(),
            recent_ratios: Vec::new(),
            fault_pending: false,
            fault_scope: None,
            fault_repair_until: None,
            forecasts: Vec::new(),
            tile_coords: coords,
            contract_bonus: (contract.bonus_dark, contract.bonus_bright, contract.bonus_backup),
        }
    }

    fn potential_of(&self, row: &Preview) -> f64 {
        let bonus = match row.quality_band.as_str() {
            "DARK" => self.contract_bonus.0,
            "BRIGHT" => self.contract_bonus.1,
            _ => self.contract_bonus.2,
        };
        row.tile_science_value * row.combined_quality * (1.0 + bonus)
    }

    fn note_observation(&mut self, tile_id: Option<&str>, expected: Option<f64>, cold_wave: bool) {
        match (tile_id, expected) {
            (Some(t), Some(e)) if e > 0.0 => {
                self.pending = Some((t.to_string(), e, cold_wave));
                let entry = self.bests.entry(t.to_string()).or_insert(0.0);
                if e > *entry {
                    *entry = e;
                }
            }
            _ => {
                self.pending = None;
            }
        }
    }

    fn under_cold_wave(&mut self, snapshot: &J) -> bool {
        if let Some(weekly) = snapshot.get("weekly") {
            if let Some(forecast) = weekly.get("weather_forecast") {
                if matches!(forecast, J::Arr(_)) {
                    self.forecasts = forecast.arr().to_vec();
                }
            }
        }
        let Some(now) = parse_utc(snapshot.get("cursor").and_then(|c| c.get("timestamp_utc"))) else {
            return false;
        };
        for forecast in &self.forecasts {
            if forecast.get("condition").and_then(J::string) != Some("cold_wave") {
                continue;
            }
            let start = parse_utc(forecast.get("predicted_start_utc"));
            let end = parse_utc(forecast.get("predicted_end_utc"));
            if let (Some(start), Some(end)) = (start, end) {
                if start <= now && now < end {
                    return true;
                }
            }
        }
        false
    }

    fn process_snapshot(&mut self, snapshot: &J) -> Vec<Report> {
        let mut reports = Vec::new();
        if let Some(fault_status) = snapshot.get("fault_status") {
            if fault_status.is_object() {
                match fault_status.get("status").and_then(J::string) {
                    Some("fault") => {
                        self.fault_pending = false;
                        self.recent_ratios.clear();
                        let scope_type = fault_status
                            .get("spatial_scope_type")
                            .and_then(J::string)
                            .unwrap_or("")
                            .to_string();
                        let payload = fault_status
                            .get("spatial_scope_payload")
                            .cloned()
                            .unwrap_or(J::Obj(Vec::new()));
                        self.fault_scope = Some((scope_type, payload));
                        self.fault_repair_until = parse_utc(fault_status.get("repair_complete_utc"));
                    }
                    Some("normal") => {
                        self.fault_pending = false;
                        self.recent_ratios.clear();
                    }
                    _ => {}
                }
            }
        }
        if let Some(feedback) = snapshot.get("tile_last_finished") {
            if feedback.is_object() {
                if let Some(tile_id) = feedback.get("tile_id").and_then(J::string) {
                    let score = feedback.get("score").and_then(J::num).unwrap_or(0.0);
                    let key = (tile_id.to_string(), score.to_bits());
                    if self.last_feedback.as_ref() != Some(&key) {
                        self.last_feedback = Some(key);
                        let pending = self.pending.take();
                        if let Some((ptile, expected, cold_wave)) = pending {
                            if ptile == tile_id && expected > 0.0 {
                                let realized = score;
                                let entry = self.bests.entry(tile_id.to_string()).or_insert(0.0);
                                if realized > *entry {
                                    *entry = realized;
                                }
                                if realized > 0.0 && !cold_wave {
                                    let ratio = realized / expected;
                                    reports.extend(self.classify(tile_id, ratio, snapshot));
                                }
                            }
                        }
                    }
                }
            }
        }
        reports
    }

    fn classify(&mut self, tile_id: &str, ratio: f64, snapshot: &J) -> Vec<Report> {
        let mut reports = Vec::new();
        let band: Option<&'static str> = if (NOVA_MIN..=NOVA_MAX).contains(&ratio) {
            Some("NOVA")
        } else if (REDD_MIN..=REDD_MAX).contains(&ratio) {
            Some("Reddening")
        } else {
            None
        };
        let night = snapshot
            .get("cursor")
            .and_then(|c| c.get("night_id"))
            .and_then(J::string)
            .unwrap_or("")
            .to_string();
        {
            let reads = self.tag_reads.entry(tile_id.to_string()).or_default();
            reads.push((band, night));
        }
        if let Some(band) = band {
            if !self.reported.contains(&(tile_id.to_string(), band)) {
                let reads = &self.tag_reads[tile_id];
                let hits: Vec<&String> = reads
                    .iter()
                    .filter(|(b, _)| *b == Some(band))
                    .map(|(_, n)| n)
                    .collect();
                let distinct: HashSet<&String> = hits.iter().copied().collect();
                if reads.len() >= TAG_MIN_READS
                    && hits.len() as f64 / reads.len() as f64 >= TAG_MIN_FRACTION
                    && distinct.len() >= 2
                {
                    self.reported.insert((tile_id.to_string(), band));
                    reports.push(Report::Tag { kind: band, tile_id: tile_id.to_string() });
                }
            }
        }
        self.recent_ratios.push(ratio);
        if self.recent_ratios.len() > FAULT_WINDOW {
            self.recent_ratios.remove(0);
        }
        let collapses = self.recent_ratios.iter().filter(|r| **r <= FAULT_MAX).count();
        let now = parse_utc(snapshot.get("cursor").and_then(|c| c.get("timestamp_utc")));
        let repair_active = self
            .fault_repair_until
            .map(|until| now.map(|n| n < until).unwrap_or(true))
            .unwrap_or(false);
        if collapses >= FAULT_MIN_EVIDENCE && !self.fault_pending && !repair_active {
            self.fault_pending = true;
            self.recent_ratios.clear();
            reports.push(Report::Fault);
        }
        reports
    }

    fn top_suspect<'a>(&self, previews: &'a [Preview]) -> Option<&'a Preview> {
        previews.iter().find(|row| {
            if let Some(reads) = self.tag_reads.get(&row.tile_id) {
                if let Some((Some(band), _)) = reads.last() {
                    return !self.reported.contains(&(row.tile_id.clone(), band));
                }
            }
            false
        })
    }

    fn in_fault_scope(&self, candidate: &J) -> bool {
        let Some((scope_type, payload)) = &self.fault_scope else {
            return false;
        };
        match scope_type.as_str() {
            "REGION_SET" => {
                let region = candidate.get("region_id").and_then(J::string).unwrap_or("");
                payload
                    .get("region_ids")
                    .map(J::arr)
                    .unwrap_or(&[])
                    .iter()
                    .any(|r| r.string() == Some(region))
            }
            "SKY_CAP_ICRS" => {
                let tile_id = candidate.get("tile_id").and_then(J::string).unwrap_or("");
                let Some((ra1, dec1)) = self.tile_coords.get(tile_id).copied() else {
                    return false;
                };
                let ra2 = payload.get("ra_deg").and_then(J::num).unwrap_or(0.0);
                let dec2 = payload.get("dec_deg").and_then(J::num).unwrap_or(0.0);
                let radius = payload.get("radius_deg").and_then(J::num).unwrap_or(0.0);
                separation_deg(ra1, dec1, ra2, dec2) <= radius
            }
            _ => false,
        }
    }

    fn filter_fault_scope(&self, snapshot: &J) -> J {
        let now = parse_utc(snapshot.get("cursor").and_then(|c| c.get("timestamp_utc")));
        let active = match (self.fault_scope.is_some(), self.fault_repair_until, now) {
            (true, Some(until), Some(now)) => now < until,
            _ => false,
        };
        if !active {
            return snapshot.clone();
        }
        let candidates = snapshot.get("candidate_tiles").map(J::arr).unwrap_or(&[]);
        let kept: Vec<J> = candidates
            .iter()
            .filter(|c| !self.in_fault_scope(c))
            .cloned()
            .collect();
        if kept.is_empty() || kept.len() == candidates.len() {
            return snapshot.clone();
        }
        let mut out = snapshot.clone();
        if let J::Obj(ref mut map) = out {
            for (k, v) in map.iter_mut() {
                if k == "candidate_tiles" {
                    *v = J::Arr(kept);
                    break;
                }
            }
        }
        out
    }
}

fn separation_deg(ra1: f64, dec1: f64, ra2: f64, dec2: f64) -> f64 {
    let (ra1, dec1, ra2, dec2) = (ra1.to_radians(), dec1.to_radians(), ra2.to_radians(), dec2.to_radians());
    let cosine = dec1.sin() * dec2.sin() + dec1.cos() * dec2.cos() * (ra1 - ra2).cos();
    cosine.clamp(-1.0, 1.0).acos().to_degrees()
}

// -------------------------------------------------------------- main --------

struct Decision {
    action: &'static str,
    tile_id: String,
    program: String,
    request_id: String,
    reason: String,
    source: &'static str,
}

fn run() -> Result<(), String> {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut out = io::BufWriter::new(stdout.lock());
    let mut contract: Option<Contract> = None;
    let mut detector: Option<Detector> = None;

    let mut reader = stdin.lock();
    let mut line = String::with_capacity(1 << 20);
    loop {
        line.clear();
        let n = reader.read_line(&mut line).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        if line.trim().is_empty() {
            continue;
        }
        let message = parse_json(line.trim())?;
        let version = message.get("protocol_version").and_then(J::string).unwrap_or("");
        if version != "participant-agent-protocol-v1" && version != "participant-agent-protocol-v2" {
            return Err(format!("unsupported participant protocol_version {version:?}"));
        }
        let message_type = message.get("message_type").and_then(J::string).unwrap_or("");
        let payload = message.get("payload").ok_or("missing payload")?.clone();
        match message_type {
            "initialize" => {
                let sv = payload.get("schema_version").and_then(J::string).unwrap_or("");
                if sv != "initial-publication-v2" && sv != "initial-publication-v1" {
                    return Err(format!("unsupported initial publication schema_version {sv:?}"));
                }
                let c = Contract::from_initialize(&payload);
                detector = Some(Detector::new(&payload, &c));
                contract = Some(c);
                eprintln!("rust-agent provider=deterministic");
            }
            "decision_request" => {
                let (Some(contract), Some(detector)) = (&contract, &mut detector) else {
                    return Err("decision_request received before initialize".into());
                };
                let schema = payload.get("schema_version").and_then(J::string).unwrap_or("");
                let mechanics = schema == "decision-snapshot-v3";
                if schema != "decision-snapshot-v2" && !mechanics && schema != "decision-snapshot-v1" {
                    return Err(format!("unsupported decision snapshot schema_version {schema:?}"));
                }
                let sequence = message.get("decision_sequence").and_then(J::num).unwrap_or(-1.0) as i64;

                let mut reports: Vec<Report> = Vec::new();
                let mut snapshot = payload.clone();
                if mechanics {
                    reports = detector.process_snapshot(&snapshot);
                    snapshot = detector.filter_fault_scope(&snapshot);
                }
                let previews = preview_actions(
                    &snapshot,
                    contract,
                    if mechanics { Some(&detector.bests) } else { None },
                );

                let mut decision: Decision = match previews.first() {
                    Some(best) => Decision {
                        action: "observe",
                        tile_id: best.tile_id.clone(),
                        program: best.program.clone(),
                        request_id: best.request_id.clone(),
                        reason: format!(
                            "highest estimated gain per second ({:.6}/s over {}s)",
                            best.gain_per_second, best.exposure as i64
                        ),
                        source: "deterministic",
                    },
                    None => Decision {
                        action: "wait",
                        tile_id: String::new(),
                        program: String::new(),
                        request_id: String::new(),
                        reason: "no legal candidate can improve the score".to_string(),
                        source: "deterministic",
                    },
                };

                if mechanics {
                    let suspect = detector.top_suspect(&previews);
                    if let Some(suspect) = suspect {
                        if previews.first().map(|p| p.gain_per_second <= 0.0).unwrap_or(true) {
                            decision = Decision {
                                action: "observe",
                                tile_id: suspect.tile_id.clone(),
                                program: suspect.program.clone(),
                                request_id: suspect.request_id.clone(),
                                reason: "repeat observation to confirm an anomalous realized-score deviation".into(),
                                source: "detector",
                            };
                        }
                    }
                    if decision.action == "observe" {
                        let row = previews.iter().find(|p| {
                            p.tile_id == decision.tile_id
                                && p.program == decision.program
                                && p.request_id == decision.request_id
                        });
                        let potential = row.map(|r| detector.potential_of(r));
                        let cold = detector.under_cold_wave(&snapshot);
                        detector.note_observation(Some(&decision.tile_id), potential, cold);
                    } else {
                        detector.note_observation(None, None, false);
                    }
                }

                let mut response = String::with_capacity(512);
                response.push_str(r#"{"protocol_version":"participant-agent-protocol-v2","message_type":"decision_response","decision_sequence":"#);
                response.push_str(&sequence.to_string());
                response.push_str(r#","action":"#);
                escape_json_string(decision.action, &mut response);
                response.push_str(r#","tile_id":"#);
                escape_json_string(&decision.tile_id, &mut response);
                response.push_str(r#","program":"#);
                escape_json_string(&decision.program, &mut response);
                response.push_str(r#","request_id":"#);
                escape_json_string(&decision.request_id, &mut response);
                response.push_str(r#","reason":"#);
                escape_json_string(&decision.reason, &mut response);
                response.push_str(r#","decision_source":"#);
                escape_json_string(decision.source, &mut response);
                if !reports.is_empty() {
                    response.push_str(r#","reports":["#);
                    for (i, report) in reports.iter().enumerate() {
                        if i > 0 {
                            response.push(',');
                        }
                        match report {
                            Report::Tag { kind, tile_id } => {
                                response.push_str(r#"{"kind":"#);
                                escape_json_string(kind, &mut response);
                                response.push_str(r#","tile_id":"#);
                                escape_json_string(tile_id, &mut response);
                                response.push('}');
                            }
                            Report::Fault => {
                                response.push_str(r#"{"kind":"Instrument_Failure"}"#);
                            }
                        }
                    }
                    response.push(']');
                }
                response.push('}');
                response.push('\n');
                out.write_all(response.as_bytes()).map_err(|e| e.to_string())?;
                out.flush().map_err(|e| e.to_string())?;
            }
            other => {
                return Err(format!("unsupported platform message_type {other:?}"));
            }
        }
    }
    Ok(())
}

fn main() {
    if let Err(err) = run() {
        eprintln!("rust-agent fatal: {err}");
        std::process::exit(2);
    }
}

