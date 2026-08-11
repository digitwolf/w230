//! Diagnostic snapshot shared with the WiFi dashboard, and its wire formats.

use crate::gear::Gear;
use crate::learn::RATIO_MIN;

/// One poll-cycle's state, published by the main loop for the HTTP handlers.
#[derive(Default, Clone)]
pub struct Snapshot {
    pub link_up: bool,
    pub gear: Option<Gear>,
    pub rpm: Option<f32>,
    pub speed: Option<f32>,
    pub ecu_neutral: Option<bool>,
    pub samples: u32,
    /// Learned ratio bands, 1st gear first; empty until calibrated (may be a
    /// partial set during progressive calibration).
    pub bands: Vec<f32>,
    /// Histogram counts (BINS entries).
    pub hist: Vec<u16>,
}

/// The `/status` JSON body.
pub fn status_json(s: &Snapshot) -> String {
    let gear = match s.gear {
        Some(Gear::Neutral) => "\"N\"".to_string(),
        Some(Gear::G(n)) => format!("\"{n}\""),
        _ => "\"-\"".to_string(),
    };
    let opt_f = |v: Option<f32>| v.map_or("null".to_string(), |x| format!("{x:.1}"));
    let opt_b = |v: Option<bool>| v.map_or("null".to_string(), |x| x.to_string());
    let bands = if s.bands.is_empty() {
        "null".to_string()
    } else {
        format!(
            "[{}]",
            s.bands
                .iter()
                .map(|x| format!("{x:.1}"))
                .collect::<Vec<_>>()
                .join(",")
        )
    };
    format!(
        "{{\"link\":{},\"gear\":{gear},\"rpm\":{},\"speed\":{},\"ecuNeutral\":{},\"samples\":{},\"bands\":{bands}}}",
        s.link_up,
        opt_f(s.rpm),
        opt_f(s.speed),
        opt_b(s.ecu_neutral),
        s.samples,
    )
}

/// The `/hist.csv` body: one `ratio,count` row per non-empty bin.
pub fn hist_csv(hist: &[u16]) -> String {
    let mut csv = String::from("ratio,count\n");
    for (i, c) in hist.iter().enumerate() {
        if *c > 0 {
            csv.push_str(&format!("{:.1},{c}\n", RATIO_MIN + i as f32 + 0.5));
        }
    }
    csv
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_json_riding() {
        let s = Snapshot {
            link_up: true,
            gear: Some(Gear::G(4)),
            rpm: Some(4000.0),
            speed: Some(40.0),
            ecu_neutral: Some(false),
            samples: 268,
            bands: vec![240.4, 165.4, 125.4, 99.5, 81.6, 69.7],
            hist: vec![],
        };
        assert_eq!(
            status_json(&s),
            "{\"link\":true,\"gear\":\"4\",\"rpm\":4000.0,\"speed\":40.0,\
             \"ecuNeutral\":false,\"samples\":268,\
             \"bands\":[240.4,165.4,125.4,99.5,81.6,69.7]}"
        );
    }

    #[test]
    fn status_json_disconnected_defaults_to_nulls() {
        let s = Snapshot::default();
        assert_eq!(
            status_json(&s),
            "{\"link\":false,\"gear\":\"-\",\"rpm\":null,\"speed\":null,\
             \"ecuNeutral\":null,\"samples\":0,\"bands\":null}"
        );
    }

    #[test]
    fn status_json_neutral() {
        let s = Snapshot {
            gear: Some(Gear::Neutral),
            ..Default::default()
        };
        assert!(status_json(&s).contains("\"gear\":\"N\""));
    }

    #[test]
    fn hist_csv_lists_only_nonempty_bins() {
        let mut hist = vec![0u16; 300];
        hist[80] = 7; // ratio bin centred at 100.5
        assert_eq!(hist_csv(&hist), "ratio,count\n100.5,7\n");
    }
}
