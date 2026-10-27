//! Read-only Marstek request serialization and response validation.

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadRequest {
    Device,
    Battery,
    EnergySystem,
}

#[derive(Serialize)]
struct Request<'a> {
    id: u64,
    method: &'static str,
    params: Parameters<'a>,
}

#[derive(Serialize)]
#[serde(untagged)]
enum Parameters<'a> {
    Discovery { ble_mac: &'a str },
    Component { id: u8 },
}

impl ReadRequest {
    pub fn serialize(self, id: u64) -> Result<Vec<u8>, serde_json::Error> {
        let (method, params) = match self {
            Self::Device => ("Marstek.GetDevice", Parameters::Discovery { ble_mac: "0" }),
            Self::Battery => ("Bat.GetStatus", Parameters::Component { id: 0 }),
            Self::EnergySystem => ("ES.GetStatus", Parameters::Component { id: 0 }),
        };
        serde_json::to_vec(&Request { id, method, params })
    }
}

#[cfg(test)]
mod tests {
    use super::ReadRequest;
    use serde_json::json;

    #[test]
    fn serializes_only_the_three_read_requests() {
        let cases = [
            (
                ReadRequest::Device,
                json!({"id": 7, "method": "Marstek.GetDevice", "params": {"ble_mac": "0"}}),
            ),
            (
                ReadRequest::Battery,
                json!({"id": 8, "method": "Bat.GetStatus", "params": {"id": 0}}),
            ),
            (
                ReadRequest::EnergySystem,
                json!({"id": 9, "method": "ES.GetStatus", "params": {"id": 0}}),
            ),
        ];
        for (index, (request, expected)) in cases.into_iter().enumerate() {
            let bytes = request.serialize(index as u64 + 7).unwrap();
            let actual: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(actual, expected);
        }
    }
}

/// Validates correlation before a response can be used as evidence.
#[derive(Debug, PartialEq, Eq)]
pub enum ResponseError {
    Oversized,
    Malformed,
    WrongRequestId,
    WrongIdentity,
    WrongComponentId,
    InvalidEnvelope,
    InvalidValue,
    DeviceError,
}

pub fn validate_response(
    bytes: &[u8],
    request_id: u64,
    request: ReadRequest,
    expected_identity: Option<&str>,
) -> Result<serde_json::Value, ResponseError> {
    if bytes.len() > 16 * 1024 {
        return Err(ResponseError::Oversized);
    }
    let envelope: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|_| ResponseError::Malformed)?;
    let object = envelope.as_object().ok_or(ResponseError::InvalidEnvelope)?;
    if object.get("id").and_then(|value| value.as_u64()) != Some(request_id) {
        return Err(ResponseError::WrongRequestId);
    }
    let source = object
        .get("src")
        .and_then(|value| value.as_str())
        .filter(|value| !value.trim().is_empty())
        .ok_or(ResponseError::WrongIdentity)?;
    if let Some(identity) = expected_identity {
        if identity.is_empty() || source != identity {
            return Err(ResponseError::WrongIdentity);
        }
    } else if request != ReadRequest::Device {
        return Err(ResponseError::WrongIdentity);
    }
    if object.contains_key("error") {
        return Err(ResponseError::DeviceError);
    }
    let result = object
        .get("result")
        .and_then(|value| value.as_object())
        .ok_or(ResponseError::InvalidEnvelope)?;
    if request == ReadRequest::Device {
        for name in ["device", "ble_mac"] {
            if result
                .get(name)
                .and_then(|value| value.as_str())
                .is_none_or(|value| value.trim().is_empty())
            {
                return Err(ResponseError::WrongIdentity);
            }
        }
    } else if result.get("id").and_then(|value| value.as_u64()) != Some(0) {
        return Err(ResponseError::WrongComponentId);
    }
    let fields: &[(&str, f64, f64)] = match request {
        ReadRequest::Device => &[],
        ReadRequest::Battery => &[
            ("soc", 0.0, 100.0),
            ("bat_temp", f64::MIN, f64::MAX),
            ("bat_capacity", 0.0, f64::MAX),
            ("rated_capacity", 0.0, f64::MAX),
        ],
        ReadRequest::EnergySystem => &[
            ("bat_soc", 0.0, 100.0),
            ("bat_cap", 0.0, f64::MAX),
            ("bat_power", f64::MIN, f64::MAX),
            ("pv_power", f64::MIN, f64::MAX),
            ("ongrid_power", f64::MIN, f64::MAX),
            ("offgrid_power", f64::MIN, f64::MAX),
            ("total_pv_energy", 0.0, f64::MAX),
            ("total_grid_output_energy", 0.0, f64::MAX),
            ("total_grid_input_energy", 0.0, f64::MAX),
            ("total_load_energy", 0.0, f64::MAX),
        ],
    };
    for &(name, minimum, maximum) in fields {
        if let Some(value) = result.get(name).filter(|value| !value.is_null()) {
            let number = value.as_f64().ok_or(ResponseError::InvalidValue)?;
            if !number.is_finite() || number < minimum || number > maximum {
                return Err(ResponseError::InvalidValue);
            }
        }
    }
    if request == ReadRequest::Battery {
        for name in ["charg_flag", "dischrg_flag"] {
            if let Some(value) = result.get(name).filter(|value| !value.is_null())
                && !value.is_boolean()
            {
                return Err(ResponseError::InvalidValue);
            }
        }
    }
    Ok(envelope)
}

#[cfg(test)]
mod response_tests {
    use super::{ReadRequest, ResponseError, validate_response};
    use serde_json::json;

    fn check(
        request: ReadRequest,
        result: serde_json::Value,
    ) -> Result<serde_json::Value, ResponseError> {
        let packet = json!({"id": 1, "src": "synthetic", "result": result});
        validate_response(
            &serde_json::to_vec(&packet).unwrap(),
            1,
            request,
            Some("synthetic"),
        )
    }

    #[test]
    fn validates_values_and_preserves_null_and_zero() {
        for (request, field) in [
            (ReadRequest::Battery, "soc"),
            (ReadRequest::EnergySystem, "bat_soc"),
        ] {
            for invalid in [json!(-1), json!(101), json!("50"), json!(true)] {
                assert_eq!(
                    check(request, json!({"id": 0, field: invalid})),
                    Err(ResponseError::InvalidValue)
                );
            }
            assert!(check(request, json!({"id": 0, field: 0})).is_ok());
            assert!(check(request, json!({"id": 0, field: null})).is_ok());
        }
        let result = check(
            ReadRequest::EnergySystem,
            json!({"id":0,"bat_power":-25,"bat_cap":null,"unknown":"ignored"}),
        )
        .unwrap();
        assert!(result["result"]["bat_cap"].is_null());
        assert!(result["result"].get("bat_soc").is_none());
        assert_eq!(
            check(ReadRequest::Battery, json!({"id":0,"charg_flag":3})),
            Err(ResponseError::InvalidValue)
        );
        assert_eq!(
            check(ReadRequest::Battery, json!({"id":0,"bat_capacity":-1})),
            Err(ResponseError::InvalidValue)
        );
        assert_eq!(
            check(ReadRequest::Battery, json!({"id":0,"bat_temp":"hot"})),
            Err(ResponseError::InvalidValue)
        );
    }
    #[test]
    fn requires_discovery_and_selected_identity() {
        for result in [
            json!({}),
            json!({"device":"", "ble_mac":"synthetic"}),
            json!({"device":2,"ble_mac":"synthetic"}),
            json!({"device":"model","ble_mac":null}),
        ] {
            assert_eq!(
                check(ReadRequest::Device, result),
                Err(ResponseError::WrongIdentity)
            );
        }
        for source in [json!(null), json!(""), json!(1)] {
            let packet =
                json!({"id":1,"src":source,"result":{"device":"model","ble_mac":"synthetic"}});
            assert_eq!(
                validate_response(
                    &serde_json::to_vec(&packet).unwrap(),
                    1,
                    ReadRequest::Device,
                    None
                ),
                Err(ResponseError::WrongIdentity)
            );
        }
        let packet = br#"{"id":1,"src":"synthetic","result":{"id":0}}"#;
        assert_eq!(
            validate_response(packet, 1, ReadRequest::Battery, None),
            Err(ResponseError::WrongIdentity)
        );
        assert_eq!(
            validate_response(packet, 1, ReadRequest::Battery, Some("other")),
            Err(ResponseError::WrongIdentity)
        );
    }
    #[test]
    fn rejects_bad_envelopes() {
        assert_eq!(
            validate_response(b"{", 1, ReadRequest::Device, None),
            Err(ResponseError::Malformed)
        );
        assert_eq!(
            validate_response(&vec![0; 16385], 1, ReadRequest::Device, None),
            Err(ResponseError::Oversized)
        );
        let packet = br#"{"id":2,"src":"synthetic","result":{"id":0}}"#;
        assert_eq!(
            validate_response(packet, 1, ReadRequest::Battery, Some("synthetic")),
            Err(ResponseError::WrongRequestId)
        );
        let packet = br#"{"id":1,"src":"synthetic","result":{"id":1}}"#;
        assert_eq!(
            validate_response(packet, 1, ReadRequest::Battery, Some("synthetic")),
            Err(ResponseError::WrongComponentId)
        );
        let packet = br#"{"id":1,"src":"synthetic","error":{"code":-1}}"#;
        assert_eq!(
            validate_response(packet, 1, ReadRequest::Device, None),
            Err(ResponseError::DeviceError)
        );
    }
}
