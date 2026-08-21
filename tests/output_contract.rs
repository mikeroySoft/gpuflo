//! Machine-readable output contract: JSON shape of the public [`Snapshot`]
//! serde types, per the 2026-08-20 machine-readable output design.

use gruflo::{
    Health, HealthCategory, Memory, MemoryPool, Observation, ObservationState, Partition,
    PartitionId, PciBdf, PhysicalGpu, PhysicalGpuId, Power, SCHEMA_VERSION, Snapshot, Temperature,
    Timestamp,
};
use serde_json::Value;
use time::macros::datetime;

fn sampled_at() -> Timestamp {
    Timestamp::from_odt(datetime!(2026-08-20 23:45:12.250 UTC))
}

fn observed() -> Timestamp {
    Timestamp::from_odt(datetime!(2026-08-20 23:45:12.247381 UTC))
}

fn last_good() -> Timestamp {
    Timestamp::from_odt(datetime!(2026-08-20 23:45:08 UTC))
}

/// A healthy MI300X-like discrete GPU with one (SPX) partition.
fn discrete_gpu() -> PhysicalGpu {
    PhysicalGpu {
        id: PhysicalGpuId::new("gpu-73fbc1"),
        index: 0,
        bdf: PciBdf::parse("0000:41:00.0").unwrap(),
        name: "AMD Instinct MI300X".to_owned(),
        uuid: Some("af1f4235-0000-1000-8000-000000000000".to_owned()),
        serial: None,
        health: Health {
            category: HealthCategory::NONE,
            message: "no active limits or faults".to_owned(),
            observed_at: observed(),
        },
        temperature: Temperature {
            hotspot_celsius: Observation::value(74.0, observed()),
            limit_celsius: Observation::value(95.0, observed()),
        },
        power: Power {
            socket_watts: Observation::value(318.0, observed()),
            cap_watts: Observation::value(320.0, observed()),
        },
        partitions: vec![Partition {
            id: PartitionId::new("gpu-73fbc1-xcp-0"),
            index: 0,
            is_primary: true,
            activity_percent: Observation::value(97.0, observed()),
            memory: Memory {
                pool: MemoryPool::VRAM,
                used_bytes: Observation::value(195_850_508_697, observed()),
                total_bytes: Observation::value(206_158_430_208, observed()),
                occupancy_percent: Observation::value(95.0, observed()),
            },
            gfx_clock_mhz: Observation::value(1700.0, observed()),
            memory_controller_activity_percent: Observation::value(64.0, observed()),
        }],
    }
}

/// An APU with a shared pool and several unavailable states, including stale.
fn apu_gpu() -> PhysicalGpu {
    PhysicalGpu {
        id: PhysicalGpuId::new("gpu-9a02cc"),
        index: 1,
        bdf: PciBdf::parse("0000:c5:00.0").unwrap(),
        name: "AMD Ryzen AI Max+ 395".to_owned(),
        uuid: None,
        serial: None,
        health: Health {
            category: HealthCategory::TELEMETRY,
            message: "hotspot telemetry is stale".to_owned(),
            observed_at: sampled_at(),
        },
        temperature: Temperature {
            hotspot_celsius: Observation::stale(last_good()),
            limit_celsius: Observation::unavailable(ObservationState::UNSUPPORTED_HARDWARE),
        },
        power: Power {
            socket_watts: Observation::unavailable(ObservationState::PERMISSION_DENIED),
            cap_watts: Observation::unavailable(ObservationState::UNSUPPORTED_DRIVER_VERSION),
        },
        partitions: vec![Partition {
            id: PartitionId::new("gpu-9a02cc-xcp-0"),
            index: 0,
            is_primary: true,
            activity_percent: Observation::unavailable(ObservationState::ASLEEP),
            memory: Memory {
                pool: MemoryPool::SHARED,
                used_bytes: Observation::value(9_663_676_416, observed()),
                total_bytes: Observation::value(34_359_738_368, observed()),
                occupancy_percent: Observation::value(28.125, observed()),
            },
            gfx_clock_mhz: Observation::unavailable(ObservationState::SOURCE_ERROR),
            memory_controller_activity_percent: Observation::unavailable(
                ObservationState::UNSUPPORTED_HARDWARE,
            ),
        }],
    }
}

fn snapshot(sequence: Option<u64>) -> Snapshot {
    Snapshot::new(sampled_at(), sequence, vec![discrete_gpu(), apu_gpu()])
}

fn to_json(sequence: Option<u64>) -> Value {
    serde_json::to_value(snapshot(sequence)).unwrap()
}

/// Applies `check` to every value in the JSON tree, tracking its path.
fn walk(value: &Value, path: &str, check: &mut impl FnMut(&str, &Value)) {
    check(path, value);
    match value {
        Value::Object(map) => {
            for (key, child) in map {
                walk(child, &format!("{path}.{key}"), check);
            }
        }
        Value::Array(items) => {
            for (i, child) in items.iter().enumerate() {
                walk(child, &format!("{path}[{i}]"), check);
            }
        }
        _ => {}
    }
}

fn assert_rfc3339_utc(text: &Value, path: &str) {
    let text = text
        .as_str()
        .unwrap_or_else(|| panic!("{path} not a string"));
    assert!(text.ends_with('Z'), "{path} must be UTC `Z`: {text}");
    time::OffsetDateTime::parse(text, &time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|e| panic!("{path} is not RFC 3339: {text}: {e}"));
}

#[test]
fn envelope_has_schema_version_1_and_rfc3339_utc_sampled_at() {
    let json = to_json(None);
    assert_eq!(json["schema_version"], serde_json::json!(SCHEMA_VERSION));
    assert_eq!(json["schema_version"], serde_json::json!(1));
    assert!(json["gruflo_version"].is_string());
    assert_rfc3339_utc(&json["sampled_at"], "sampled_at");
    assert_eq!(json["gpus"].as_array().unwrap().len(), 2);
}

#[test]
fn every_observation_is_exactly_one_tagged_form() {
    let json = to_json(None);
    walk(&json, "$", &mut |path, value| {
        let Some(map) = value.as_object() else { return };
        let has_value = map.contains_key("value");
        let has_state = map.contains_key("state");
        assert!(
            !(has_value && has_state),
            "{path} mixes value and state forms"
        );
        if has_value {
            assert!(
                map.contains_key("observed_at"),
                "{path} value form lacks observed_at"
            );
        }
        if let Some(at) = map.get("observed_at") {
            assert_rfc3339_utc(at, &format!("{path}.observed_at"));
        }
    });
}

#[test]
fn scope_puts_socket_metrics_on_gpus_and_xcp_metrics_on_partitions() {
    let json = to_json(None);
    for (i, gpu) in json["gpus"].as_array().unwrap().iter().enumerate() {
        let gpu = gpu.as_object().unwrap();
        for socket_field in ["temperature", "power", "health"] {
            assert!(gpu.contains_key(socket_field), "gpus[{i}].{socket_field}");
        }
        for xcp_field in ["activity_percent", "memory", "gfx_clock_mhz"] {
            assert!(
                !gpu.contains_key(xcp_field),
                "gpus[{i}] must not own XCP-scoped {xcp_field}"
            );
        }
        let partitions = gpu["partitions"].as_array().unwrap();
        assert!(!partitions.is_empty(), "gpus[{i}] owns >= 1 partition");
        for (p, partition) in partitions.iter().enumerate() {
            let partition = partition.as_object().unwrap();
            for xcp_field in [
                "id",
                "index",
                "is_primary",
                "activity_percent",
                "memory",
                "gfx_clock_mhz",
                "memory_controller_activity_percent",
            ] {
                assert!(
                    partition.contains_key(xcp_field),
                    "gpus[{i}].partitions[{p}].{xcp_field}"
                );
            }
            for socket_field in ["temperature", "power", "health"] {
                assert!(
                    !partition.contains_key(socket_field),
                    "gpus[{i}].partitions[{p}] must not own socket-scoped {socket_field}"
                );
            }
        }
    }
}

#[test]
fn metric_field_names_carry_their_units() {
    let json = to_json(None);
    let gpu = &json["gpus"][0];
    assert!(gpu["temperature"].get("hotspot_celsius").is_some());
    assert!(gpu["temperature"].get("limit_celsius").is_some());
    assert!(gpu["power"].get("socket_watts").is_some());
    assert!(gpu["power"].get("cap_watts").is_some());
    let partition = &gpu["partitions"][0];
    assert!(partition.get("activity_percent").is_some());
    assert!(partition.get("gfx_clock_mhz").is_some());
    assert!(
        partition
            .get("memory_controller_activity_percent")
            .is_some()
    );
    let memory = partition["memory"].as_object().unwrap();
    for field in ["pool", "used_bytes", "total_bytes", "occupancy_percent"] {
        assert!(memory.contains_key(field), "memory.{field}");
    }
    // Bytes stay integer bytes.
    assert_eq!(
        memory["used_bytes"]["value"],
        serde_json::json!(195_850_508_697u64)
    );
}

#[test]
fn no_null_and_no_non_finite_numbers_anywhere() {
    let json = to_json(Some(3));
    walk(&json, "$", &mut |path, value| {
        assert!(!value.is_null(), "{path} is null");
        if let Some(number) = value.as_number() {
            if let Some(float) = number.as_f64() {
                assert!(float.is_finite(), "{path} is non-finite: {number}");
            }
        }
    });
}

#[test]
fn stale_keeps_observed_at_but_never_a_value() {
    let json = to_json(None);
    let hotspot = json["gpus"][1]["temperature"]["hotspot_celsius"]
        .as_object()
        .unwrap();
    assert_eq!(hotspot["state"], serde_json::json!("stale"));
    assert!(
        hotspot.contains_key("observed_at"),
        "stale keeps last-good time"
    );
    assert!(!hotspot.contains_key("value"), "stale never keeps a value");
    // Non-stale unavailable states carry no observed_at.
    let socket = json["gpus"][1]["power"]["socket_watts"]
        .as_object()
        .unwrap();
    assert_eq!(socket["state"], serde_json::json!("permission_denied"));
    assert!(!socket.contains_key("observed_at"));
}

#[test]
fn unknown_state_and_pool_strings_round_trip() {
    let mut snapshot = snapshot(None);
    snapshot.gpus[1].partitions[0].memory.pool = MemoryPool::new("carveout");
    snapshot.gpus[1].partitions[0].gfx_clock_mhz =
        Observation::unavailable(ObservationState::new("thermally_gated"));
    let text = serde_json::to_string(&snapshot).unwrap();
    let parsed: Snapshot = serde_json::from_str(&text).unwrap();
    assert_eq!(parsed, snapshot);
    assert_eq!(
        parsed.gpus[1].partitions[0].memory.pool.as_str(),
        "carveout"
    );
    assert_eq!(
        parsed.gpus[1].partitions[0]
            .gfx_clock_mhz
            .state()
            .unwrap()
            .as_str(),
        "thermally_gated"
    );
}

#[test]
fn sequence_is_omitted_when_none_and_present_when_some() {
    let oneshot = to_json(None);
    assert!(
        !oneshot.as_object().unwrap().contains_key("sequence"),
        "one-shot JSON must omit sequence entirely"
    );
    let streamed = to_json(Some(42));
    assert_eq!(streamed["sequence"], serde_json::json!(42));
    // Round-trip preserves both forms.
    let restored: Snapshot = serde_json::from_value(oneshot).unwrap();
    assert_eq!(restored.sequence, None);
    let restored: Snapshot = serde_json::from_value(streamed).unwrap();
    assert_eq!(restored.sequence, Some(42));
}
