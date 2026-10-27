use crate::marstek::protocol::ReadRequest;
use crate::marstek::transport::{Transport, TransportError};
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

fn timestamp() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn field_names(method: &str) -> &'static [&'static str] {
    match method {
        "Marstek.GetDevice" => &["device", "ver", "ble_mac", "wifi_mac", "wifi_name", "ip"],
        "Bat.GetStatus" => &[
            "soc",
            "charg_flag",
            "dischrg_flag",
            "bat_temp",
            "bat_capacity",
            "rated_capacity",
        ],
        "ES.GetStatus" => &[
            "bat_soc",
            "bat_cap",
            "pv_power",
            "ongrid_power",
            "offgrid_power",
            "bat_power",
            "total_pv_energy",
            "total_grid_output_energy",
            "total_grid_input_energy",
            "total_load_energy",
        ],
        _ => &[],
    }
}

fn availability_summary(
    method: &str,
    response: Option<&Value>,
) -> BTreeMap<&'static str, &'static str> {
    let result = response.and_then(|value| value.get("result"));
    field_names(method)
        .iter()
        .map(|&name| {
            let state = match result.and_then(|value| value.get(name)) {
                None => "missing",
                Some(Value::Null) => "null",
                Some(_) => "present",
            };
            (name, state)
        })
        .collect()
}

fn sanitized_outcome(result: &Result<Value, TransportError>) -> &'static str {
    match result {
        Ok(_) => "received",
        Err(TransportError::Timeout) => "timeout",
        Err(TransportError::Invalid(_)) => "invalid_response",
        Err(TransportError::Serialization(_)) => "serialization_error",
        Err(TransportError::Io(_)) => "network_error",
    }
}

fn summary_json(
    method: &str,
    request_id: u64,
    started_unix_ms: u128,
    finished_unix_ms: u128,
    result: &Result<Value, TransportError>,
) -> Value {
    let response = result.as_ref().ok();
    serde_json::json!({
        "method": method,
        "request_id": request_id,
        "started_unix_ms": started_unix_ms,
        "finished_unix_ms": finished_unix_ms,
        "outcome": sanitized_outcome(result),
        "fields": availability_summary(method, response)
    })
}

#[derive(Serialize)]
struct CaptureRecord<'a> {
    method: &'a str,
    request_id: u64,
    started_unix_ms: u128,
    finished_unix_ms: u128,
    outcome: &'static str,
    sender: Option<String>,
    response: Option<&'a Value>,
}

fn save_capture(
    directory: &Path,
    method: &str,
    request_id: u64,
    started_unix_ms: u128,
    finished_unix_ms: u128,
    result: &Result<Value, TransportError>,
    sender: Option<SocketAddr>,
) -> Result<PathBuf, String> {
    fs::create_dir_all(directory)
        .map_err(|error| format!("Unable to create capture directory: {error}"))?;
    let record = CaptureRecord {
        method,
        request_id,
        started_unix_ms,
        finished_unix_ms,
        outcome: sanitized_outcome(result),
        sender: sender.map(|address| address.to_string()),
        response: result.as_ref().ok(),
    };
    let contents = serde_json::to_vec_pretty(&record)
        .map_err(|error| format!("Unable to serialize diagnostic capture: {error}"))?;
    for suffix in 0..100_u8 {
        let path = directory.join(format!(
            "{method}-{request_id}-{finished_unix_ms}-{suffix}.json"
        ));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut file) => {
                file.write_all(&contents)
                    .map_err(|error| format!("Unable to write diagnostic capture: {error}"))?;
                return Ok(path);
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(format!("Unable to create diagnostic capture: {error}")),
        }
    }
    Err("Unable to create unique diagnostic capture after 100 attempts".into())
}

fn report(
    method: &str,
    request_id: u64,
    started_unix_ms: u128,
    result: Result<Value, TransportError>,
    sender: Option<SocketAddr>,
    capture_directory: Option<&Path>,
) -> Result<(), String> {
    let finished = timestamp();
    if let Some(directory) = capture_directory {
        save_capture(
            directory,
            method,
            request_id,
            started_unix_ms,
            finished,
            &result,
            sender,
        )?;
    }
    println!(
        "{}",
        summary_json(method, request_id, started_unix_ms, finished, &result)
    );
    Ok(())
}

pub fn run(arguments: &[String]) -> Result<bool, String> {
    let Some(mode) = arguments.first().map(String::as_str) else {
        return Ok(false);
    };
    if mode != "--discover" && mode != "--probe" {
        return Ok(false);
    }
    let mut port = 30000u16;
    let mut host: Option<IpAddr> = None;
    let mut capture_directory: Option<PathBuf> = None;
    let mut index = 1;
    while index < arguments.len() {
        let value = arguments.get(index + 1).ok_or("missing argument value")?;
        match arguments[index].as_str() {
            "--port" => port = value.parse::<u16>().map_err(|_| "invalid port")?,
            "--host" if mode == "--probe" => {
                host = Some(value.parse().map_err(|_| "invalid IP address")?)
            }
            "--capture-dir" => capture_directory = Some(PathBuf::from(value)),
            _ => return Err("invalid diagnostic argument".into()),
        }
        index += 2;
    }
    if port == 0 {
        return Err("invalid port".into());
    }
    let mut transport = Transport::bind().map_err(|error| error.to_string())?;
    if mode == "--discover" {
        let started = timestamp();
        let request_id = transport.last_request_id() + 1;
        let results = transport
            .discover(port)
            .map_err(|error| error.to_string())?;
        if results.is_empty() {
            let result = Err(TransportError::Timeout);
            report(
                "Marstek.GetDevice",
                request_id,
                started,
                result,
                None,
                capture_directory.as_deref(),
            )?;
        } else {
            for (sender, response) in results {
                report(
                    "Marstek.GetDevice",
                    request_id,
                    started,
                    Ok(response),
                    Some(sender),
                    capture_directory.as_deref(),
                )?;
            }
        }
        return Ok(true);
    }
    let address = host.ok_or("--probe requires --host")?;
    if !address.is_ipv4() {
        return Err("IPv4 address required".into());
    }
    let endpoint = SocketAddr::new(address, port);
    let started = timestamp();
    let identity_response = transport.exchange(endpoint, ReadRequest::Device, None);
    let identity_id = transport.last_request_id();
    let identity = match &identity_response {
        Ok(value) => value["src"].as_str().ok_or("identity missing")?.to_owned(),
        Err(_) => {
            report(
                "Marstek.GetDevice",
                identity_id,
                started,
                identity_response,
                Some(endpoint),
                capture_directory.as_deref(),
            )?;
            return Err("identity request failed".into());
        }
    };
    report(
        "Marstek.GetDevice",
        identity_id,
        started,
        identity_response,
        Some(endpoint),
        capture_directory.as_deref(),
    )?;
    let first_cycle = Instant::now();
    for cycle in 0..3 {
        let target = first_cycle + Duration::from_secs(cycle * 30);
        if let Some(delay) = target.checked_duration_since(Instant::now()) {
            std::thread::sleep(delay);
        }
        for (request, method) in [
            (ReadRequest::Battery, "Bat.GetStatus"),
            (ReadRequest::EnergySystem, "ES.GetStatus"),
        ] {
            let started = timestamp();
            let response = transport.exchange(endpoint, request, Some(&identity));
            report(
                method,
                transport.last_request_id(),
                started,
                response,
                Some(endpoint),
                capture_directory.as_deref(),
            )?;
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_summary_excludes_private_response_values_and_endpoint() {
        let response = serde_json::json!({
            "id": 7,
            "src": "Venus-SECRET",
            "result": {
                "device": "DEVICE-SECRET",
                "ble_mac": "AA:BB:CC:DD:EE:FF",
                "wifi_name": "PRIVATE-SSID",
                "ip": "192.0.2.19",
                "soc": 0
            }
        });
        let summary = summary_json("Marstek.GetDevice", 7, 100, 200, &Ok(response));
        let text = summary.to_string();
        for secret in [
            "Venus-SECRET",
            "DEVICE-SECRET",
            "AA:BB:CC:DD:EE:FF",
            "PRIVATE-SSID",
            "192.0.2.19",
        ] {
            assert!(!text.contains(secret));
        }
        assert_eq!(summary["fields"]["device"], "present");
        assert_eq!(summary["fields"]["wifi_mac"], "missing");
    }

    #[test]
    fn explicit_capture_preserves_evidence_and_never_overwrites() {
        let directory = std::env::temp_dir().join(format!("marstek-capture-test-{}", timestamp()));
        let response =
            serde_json::json!({"src":"synthetic-device","result":{"wifi_name":"synthetic-ssid"}});
        let result = Ok(response);
        let first = save_capture(
            &directory,
            "Marstek.GetDevice",
            7,
            100,
            200,
            &result,
            Some("127.0.0.1:30000".parse().unwrap()),
        )
        .unwrap();
        let second = save_capture(
            &directory,
            "Marstek.GetDevice",
            7,
            100,
            200,
            &result,
            Some("127.0.0.1:30000".parse().unwrap()),
        )
        .unwrap();
        assert_ne!(first, second);
        let contents = fs::read_to_string(&first).unwrap();
        for evidence in ["synthetic-device", "synthetic-ssid", "127.0.0.1:30000"] {
            assert!(contents.contains(evidence));
        }
        fs::remove_dir_all(directory).unwrap();
    }
}
