//! WiFi softAP + HTTP diagnostic server.
//!
//! The ATOM broadcasts `W230-GEAR` (WPA2, password `w230diag`); join it and
//! open http://192.168.71.1/ for a live dashboard. Endpoints:
//!   GET  /          — auto-refreshing HTML dashboard
//!   GET  /status    — JSON snapshot (gear, rpm, speed, clutch, link, bands)
//!   GET  /hist.csv  — ratio histogram as CSV (offline analysis)
//!   POST /clear     — wipe the learned calibration
//!
//! Handlers run on the HTTP server's own threads; the main poll loop pushes a
//! fresh `Snapshot` into the shared mutex every cycle and honours `clear_req`.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use esp_idf_hal::modem::Modem;
use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::http::server::EspHttpServer;
use esp_idf_svc::http::Method;
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use esp_idf_svc::wifi::{AccessPointConfiguration, AuthMethod, Configuration, EspWifi};
use log::info;

use crate::gear::{Gear, NUM_GEARS};
use crate::learn::RATIO_MIN;

const SSID: &str = "W230-GEAR";
const PASSWORD: &str = "w230diag";

#[derive(Default, Clone)]
pub struct Snapshot {
    pub link_up: bool,
    pub gear: Option<Gear>,
    pub rpm: Option<f32>,
    pub speed: Option<f32>,
    pub clutch: Option<bool>,
    pub samples: u32,
    pub bands: Option<[f32; NUM_GEARS]>,
    pub hist: Vec<u16>, // BINS entries
}

pub struct WebDiag {
    pub shared: Arc<Mutex<Snapshot>>,
    /// Set by POST /clear; the main loop wipes the calibration and clears it.
    pub clear_req: Arc<AtomicBool>,
    _wifi: EspWifi<'static>,
    _server: EspHttpServer<'static>,
}

pub fn start(
    modem: Modem,
    sysloop: EspSystemEventLoop,
    nvs: EspDefaultNvsPartition,
) -> anyhow::Result<WebDiag> {
    let mut wifi = EspWifi::new(modem, sysloop, Some(nvs))?;
    wifi.set_configuration(&Configuration::AccessPoint(AccessPointConfiguration {
        ssid: SSID.try_into().unwrap(),
        password: PASSWORD.try_into().unwrap(),
        auth_method: AuthMethod::WPA2Personal,
        channel: 1,
        ..Default::default()
    }))?;
    wifi.start()?;
    info!("WEB: softAP '{SSID}' up — http://192.168.71.1/");

    let shared = Arc::new(Mutex::new(Snapshot::default()));
    let clear_req = Arc::new(AtomicBool::new(false));

    let mut server = EspHttpServer::new(&esp_idf_svc::http::server::Configuration::default())?;

    server.fn_handler("/", Method::Get, |req| {
        let mut resp = req.into_response(200, Some("OK"), &[("Content-Type", "text/html")])?;
        resp.write(INDEX_HTML.as_bytes())?;
        Ok::<(), anyhow::Error>(())
    })?;

    {
        let shared = shared.clone();
        server.fn_handler("/status", Method::Get, move |req| {
            let s = shared.lock().unwrap().clone();
            let json = status_json(&s);
            let mut resp =
                req.into_response(200, Some("OK"), &[("Content-Type", "application/json")])?;
            resp.write(json.as_bytes())?;
            Ok::<(), anyhow::Error>(())
        })?;
    }

    {
        let shared = shared.clone();
        server.fn_handler("/hist.csv", Method::Get, move |req| {
            let s = shared.lock().unwrap().clone();
            let mut csv = String::from("ratio,count\n");
            for (i, c) in s.hist.iter().enumerate() {
                if *c > 0 {
                    csv.push_str(&format!("{:.1},{c}\n", RATIO_MIN + i as f32 + 0.5));
                }
            }
            let mut resp =
                req.into_response(200, Some("OK"), &[("Content-Type", "text/csv")])?;
            resp.write(csv.as_bytes())?;
            Ok::<(), anyhow::Error>(())
        })?;
    }

    {
        let clear_req = clear_req.clone();
        server.fn_handler("/clear", Method::Post, move |req| {
            clear_req.store(true, Ordering::Relaxed);
            let mut resp = req.into_response(200, Some("OK"), &[("Content-Type", "text/plain")])?;
            resp.write(b"calibration wipe requested\n")?;
            Ok::<(), anyhow::Error>(())
        })?;
    }

    Ok(WebDiag {
        shared,
        clear_req,
        _wifi: wifi,
        _server: server,
    })
}

fn status_json(s: &Snapshot) -> String {
    let gear = match s.gear {
        Some(Gear::Neutral) => "\"N\"".to_string(),
        Some(Gear::G(n)) => format!("\"{n}\""),
        _ => "\"-\"".to_string(),
    };
    let opt_f = |v: Option<f32>| v.map_or("null".to_string(), |x| format!("{x:.1}"));
    let opt_b = |v: Option<bool>| v.map_or("null".to_string(), |x| x.to_string());
    let bands = s.bands.map_or("null".to_string(), |b| {
        format!(
            "[{}]",
            b.iter()
                .map(|x| format!("{x:.1}"))
                .collect::<Vec<_>>()
                .join(",")
        )
    });
    format!(
        "{{\"link\":{},\"gear\":{gear},\"rpm\":{},\"speed\":{},\"clutch\":{},\"samples\":{},\"bands\":{bands}}}",
        s.link_up,
        opt_f(s.rpm),
        opt_f(s.speed),
        opt_b(s.clutch),
        s.samples,
    )
}

const INDEX_HTML: &str = r#"<!DOCTYPE html>
<html><head><meta name=viewport content="width=device-width,initial-scale=1">
<title>W230 gear diag</title>
<style>
 body{font-family:system-ui,sans-serif;background:#111;color:#eee;margin:1rem}
 h1{font-size:1.1rem;color:#8cf}
 #gear{font-size:5rem;text-align:center;margin:.2em 0;color:#0f6}
 table{border-collapse:collapse;width:100%;max-width:22rem}
 td{padding:.25rem .5rem;border-bottom:1px solid #333}
 td:last-child{text-align:right;font-variant-numeric:tabular-nums}
 .off{color:#f55}.on{color:#5f5}
 a,button{color:#8cf;background:none;border:1px solid #8cf;border-radius:4px;
   padding:.4rem .8rem;text-decoration:none;font-size:1rem;margin-right:.5rem}
</style></head><body>
<h1>W230 gear indicator</h1>
<div id=gear>-</div>
<table>
 <tr><td>K-line</td><td id=link>?</td></tr>
 <tr><td>RPM</td><td id=rpm>-</td></tr>
 <tr><td>Speed</td><td id=speed>-</td></tr>
 <tr><td>Clutch</td><td id=clutch>-</td></tr>
 <tr><td>Learned samples</td><td id=samples>-</td></tr>
 <tr><td>Ratio bands</td><td id=bands>-</td></tr>
</table>
<p>
 <a href=/hist.csv download>histogram.csv</a>
 <button onclick="if(confirm('Wipe learned calibration?'))fetch('/clear',{method:'POST'})">wipe calibration</button>
</p>
<script>
async function tick(){
 try{
  const s=await (await fetch('/status')).json();
  document.getElementById('gear').textContent=s.gear;
  document.getElementById('link').innerHTML=s.link?'<span class=on>up</span>':'<span class=off>down</span>';
  document.getElementById('rpm').textContent=s.rpm??'-';
  document.getElementById('speed').textContent=s.speed??'-';
  document.getElementById('clutch').textContent=s.clutch==null?'-':(s.clutch?'pulled':'out');
  document.getElementById('samples').textContent=s.samples;
  document.getElementById('bands').textContent=s.bands?s.bands.join(' '):'not calibrated';
 }catch(e){document.getElementById('link').textContent='?'}
}
setInterval(tick,1000);tick();
</script></body></html>
"#;
