//! Pure parsers for AACP packets the AirPods send. Every function takes `payload = packet[4..]`
//! (so `payload[0]` is the opcode) and returns `None` instead of panicking on short or
//! malformed input.

use crate::bluetooth::aacp::{
    AirPodsLEKeys, BatteryComponent, BatteryInfo, BatteryStatus, ConnectedDevice,
    EarDetectionStatus,
};
use crate::devices::airpods::AirPodsInformation;
use log::error;

pub fn parse_battery(payload: &[u8]) -> Option<Vec<BatteryInfo>> {
    let count = *payload.get(2)? as usize;
    let entries = payload.get(3..3 + count * 5)?;
    let batteries = entries
        .as_chunks::<5>()
        .0
        .iter()
        .filter_map(|entry| {
            let component = match entry[0] {
                0x01 => BatteryComponent::Headphone,
                0x02 => BatteryComponent::Right,
                0x04 => BatteryComponent::Left,
                0x08 => BatteryComponent::Case,
                other => {
                    error!("Unknown battery component: {:#04x}", other);
                    return None;
                }
            };
            let status = match entry[3] {
                0x01 => BatteryStatus::Charging,
                0x02 => BatteryStatus::NotCharging,
                0x04 => BatteryStatus::Disconnected,
                0x05 => BatteryStatus::OptimizedCharging,
                _ => BatteryStatus::Unknown,
            };
            Some(BatteryInfo {
                component,
                level: entry[2],
                status,
            })
        })
        .collect();
    Some(batteries)
}

fn ear_status(byte: u8) -> EarDetectionStatus {
    match byte {
        0x00 => EarDetectionStatus::InEar,
        0x01 => EarDetectionStatus::OutOfEar,
        0x02 => EarDetectionStatus::InCase,
        0x03 => EarDetectionStatus::Disconnected,
        other => {
            error!("Unknown ear detection status: {:#04x}", other);
            EarDetectionStatus::OutOfEar
        }
    }
}

pub fn parse_ear_detection(payload: &[u8]) -> Option<[EarDetectionStatus; 2]> {
    let primary = *payload.get(2)?;
    let secondary = *payload.get(3)?;
    Some([ear_status(primary), ear_status(secondary)])
}

pub fn parse_information(payload: &[u8]) -> Option<AirPodsInformation> {
    let data = payload.get(4..)?;
    // The first token (before the first NUL) is not part of the information strings.
    let first_nul = data.iter().position(|&b| b == 0x00)?;
    let mut strings: Vec<String> = data[first_nul..]
        .split(|&b| b == 0x00)
        .filter(|s| !s.is_empty())
        .filter_map(|s| std::str::from_utf8(s).ok().map(str::to_string))
        .collect();
    if strings.is_empty() {
        return None;
    }
    strings.remove(0);
    if strings.is_empty() {
        return None;
    }
    let field = |i: usize| strings.get(i).cloned().unwrap_or_default();
    Some(AirPodsInformation {
        name: field(0),
        model_number: field(1),
        manufacturer: field(2),
        serial_number: field(3),
        version1: field(4),
        version2: field(5),
        hardware_revision: field(6),
        updater_identifier: field(7),
        left_serial_number: field(8),
        right_serial_number: field(9),
        version3: field(10),
        le_keys: AirPodsLEKeys {
            irk: String::new(),
            enc_key: String::new(),
        },
    })
}

pub fn parse_connected_devices(payload: &[u8]) -> Option<Vec<ConnectedDevice>> {
    let count = *payload.get(4)? as usize;
    let entries = payload.get(5..5 + count * 8)?;
    Some(
        entries
            .as_chunks::<8>()
            .0
            .iter()
            .map(|entry| ConnectedDevice {
                mac: entry[..6]
                    .iter()
                    .map(|b| format!("{:02X}", b))
                    .collect::<Vec<_>>()
                    .join(":"),
                info1: entry[6],
                info2: entry[7],
                r#type: None,
            })
            .collect(),
    )
}

pub fn parse_smart_routing_response(payload: &[u8]) -> Option<String> {
    let body = payload.get(2..)?;
    Some(String::from_utf8_lossy(body).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> Vec<u8> {
        hex::decode(s.replace(' ', "")).unwrap()
    }

    fn battery_fixture() -> Vec<u8> {
        hex("04 00 03 02 01 61 02 01 04 01 60 03 01 08 01 00 04 01")
    }

    fn information_fixture() -> Vec<u8> {
        let mut payload = hex("1D 00 AA BB");
        payload.extend_from_slice(
            b"X\0Y\0Test AirPods Pro\0A2084\0Apple Inc.\0SER\0v1\0v2\01.0.0\0upd\0LS\0RS\0v3\0",
        );
        payload
    }

    fn connected_devices_fixture() -> Vec<u8> {
        hex("2E 00 00 00 01 AA BB CC DD EE 02 02 03")
    }

    fn smart_routing_fixture() -> Vec<u8> {
        let mut payload = hex("11 00");
        payload.extend_from_slice(b"\x01\xE3_audioRoutingSetOwnershipToFalse\x01");
        payload
    }

    #[test]
    fn battery_keeps_unknown_status_entries() {
        let batteries = parse_battery(&battery_fixture()).unwrap();
        assert_eq!(
            batteries,
            vec![
                BatteryInfo {
                    component: BatteryComponent::Right,
                    level: 97,
                    status: BatteryStatus::NotCharging
                },
                BatteryInfo {
                    component: BatteryComponent::Left,
                    level: 96,
                    status: BatteryStatus::Unknown
                },
                BatteryInfo {
                    component: BatteryComponent::Case,
                    level: 0,
                    status: BatteryStatus::Disconnected
                },
            ]
        );
    }

    #[test]
    fn battery_headset_only() {
        let batteries = parse_battery(&hex("04 00 01 01 01 50 01 01")).unwrap();
        assert_eq!(
            batteries,
            vec![BatteryInfo {
                component: BatteryComponent::Headphone,
                level: 80,
                status: BatteryStatus::Charging
            }]
        );
    }

    #[test]
    fn ear_detection_two_statuses() {
        assert_eq!(
            parse_ear_detection(&hex("06 00 00 01")),
            Some([EarDetectionStatus::InEar, EarDetectionStatus::OutOfEar])
        );
    }

    #[test]
    fn ear_detection_too_short() {
        assert_eq!(parse_ear_detection(&hex("06 00 02")), None);
    }

    #[test]
    fn information_fields() {
        let info = parse_information(&information_fixture()).unwrap();
        assert_eq!(info.name, "Test AirPods Pro");
        assert_eq!(info.model_number, "A2084");
        assert_eq!(info.version3, "v3");
    }

    #[test]
    fn information_without_strings_is_none() {
        assert!(parse_information(&hex("1D 00 00 00 00 00")).is_none());
    }

    #[test]
    fn connected_devices_count_at_payload_4() {
        let devices = parse_connected_devices(&connected_devices_fixture()).unwrap();
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].mac, "AA:BB:CC:DD:EE:02");
        assert_eq!(devices[0].info1, 2);
        assert_eq!(devices[0].info2, 3);
    }

    #[test]
    fn connected_devices_short_entry_is_none() {
        assert!(parse_connected_devices(&hex("2E 00 00 00 02 AA BB CC DD EE 02 02 03")).is_none());
    }

    #[test]
    fn smart_routing_response_text() {
        let text = parse_smart_routing_response(&smart_routing_fixture()).unwrap();
        assert!(text.contains("SetOwnershipToFalse"));
    }

    #[test]
    fn no_parser_panics_on_truncation() {
        let fixtures: Vec<(Vec<u8>, fn(&[u8]))> = vec![
            (battery_fixture(), |p| {
                let _ = parse_battery(p);
            }),
            (hex("06 00 00 01"), |p| {
                let _ = parse_ear_detection(p);
            }),
            (information_fixture(), |p| {
                let _ = parse_information(p);
            }),
            (connected_devices_fixture(), |p| {
                let _ = parse_connected_devices(p);
            }),
            (smart_routing_fixture(), |p| {
                let _ = parse_smart_routing_response(p);
            }),
        ];
        for (fixture, parse) in fixtures {
            for n in 0..=fixture.len() {
                parse(&fixture[..n]);
            }
        }
    }
}
