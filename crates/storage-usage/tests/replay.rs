//! 与 TS `storageCatalog.ts` / `usageAggregate.ts` / `groupStorageRootsByVolume` 的对拍。

use std::path::Path;

use serde_json::{json, Value};
use storage_usage::{
    classify_storage_path, get_storage_category_cleanability, get_storage_clean_scopes,
    group_storage_roots_by_volume, is_protected_storage_path, plan_storage_clean, ScanJobFailure,
    ScanJobProgressAction, StorageCatalogContext, StorageCategoryId, StorageCleanCandidate,
    StorageCleanParams, StoragePathError, StorageRootId, StorageRootSpec, StorageRootUsage,
    StorageScanEntry, StorageScanJob, StorageScanProgress, StorageUsageAccumulator, StorageVolume,
    STORAGE_CATEGORY_IDS,
};

fn fixture() -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("storage-usage.json");
    let text = std::fs::read_to_string(&path).expect("读取 storage-usage.json");
    serde_json::from_str(&text).expect("解析 fixture")
}

fn category_id(value: &str) -> StorageCategoryId {
    StorageCategoryId::parse(value).unwrap_or_else(|| panic!("未知 categoryId: {value}"))
}

fn root_id(value: &str) -> StorageRootId {
    StorageRootId::parse(value).unwrap_or_else(|| panic!("未知 rootId: {value}"))
}

fn context(value: &Value) -> StorageCatalogContext {
    StorageCatalogContext {
        root_id: root_id(value["rootId"].as_str().expect("rootId")),
        has_custom_data_base_dir: value["hasCustomDataBaseDir"]
            .as_bool()
            .expect("hasCustomDataBaseDir"),
    }
}

fn volume(value: &Value) -> Option<StorageVolume> {
    if value.is_null() {
        return None;
    }
    Some(StorageVolume {
        device_id: value["deviceId"].as_str().expect("deviceId").to_string(),
        mount_point: value["mountPoint"]
            .as_str()
            .expect("mountPoint")
            .to_string(),
        total_bytes: value["totalBytes"].as_i64().expect("totalBytes"),
        free_bytes: value["freeBytes"].as_i64().expect("freeBytes"),
    })
}

fn root_usage(value: &Value) -> StorageRootUsage {
    StorageRootUsage {
        id: root_id(value["id"].as_str().expect("root id")),
        path: value["path"].as_str().expect("root path").to_string(),
        volume: volume(&value["volume"]),
        bytes: value["bytes"].as_i64().expect("root bytes"),
        file_count: value["fileCount"].as_u64().expect("root fileCount"),
        categories: Vec::new(),
    }
}

fn scan_progress(value: &Value) -> StorageScanProgress {
    StorageScanProgress {
        roots: value["roots"]
            .as_array()
            .expect("progress roots")
            .iter()
            .map(root_usage)
            .collect(),
        errors: value["errors"]
            .as_array()
            .expect("progress errors")
            .iter()
            .map(|error| StoragePathError {
                path: error["path"].as_str().expect("error path").to_string(),
                code: error["code"].as_str().expect("error code").to_string(),
            })
            .collect(),
    }
}

fn scan_entry(value: &Value) -> StorageScanEntry {
    StorageScanEntry {
        relative_path: value["relativePath"]
            .as_str()
            .expect("relativePath")
            .to_string(),
        bytes: value["bytes"].as_i64().expect("bytes"),
        mtime_ms: value["mtimeMs"].as_i64().expect("mtimeMs"),
    }
}

#[test]
fn matches_typescript_storage_usage() {
    let fixture = fixture();
    let category_ids: Vec<Value> = STORAGE_CATEGORY_IDS
        .iter()
        .map(|id| json!(id.as_str()))
        .collect();
    assert_eq!(json!(category_ids), fixture["categoryIds"]);

    let mut cleanability = serde_json::Map::new();
    let mut clean_scopes = serde_json::Map::new();
    for id in STORAGE_CATEGORY_IDS {
        cleanability.insert(
            id.as_str().to_string(),
            json!(get_storage_category_cleanability(id).as_str()),
        );
        clean_scopes.insert(
            id.as_str().to_string(),
            Value::Array(
                get_storage_clean_scopes(id)
                    .iter()
                    .map(|scope| scope.to_json())
                    .collect(),
            ),
        );
    }
    assert_eq!(Value::Object(cleanability), fixture["cleanability"]);
    assert_eq!(Value::Object(clean_scopes), fixture["cleanScopes"]);

    for case in fixture["classifications"]
        .as_array()
        .expect("classifications")
    {
        let raw_path = case["rawPath"].as_str().expect("rawPath");
        let expected = &case["expected"];
        let actual = classify_storage_path(raw_path, &context(&case["context"])).to_json();
        assert_eq!(&actual, expected, "classification {raw_path:?}");
    }

    for case in fixture["protected"].as_array().expect("protected") {
        let raw_path = case["rawPath"].as_str().expect("protected rawPath");
        assert_eq!(
            json!(is_protected_storage_path(raw_path)),
            case["expected"],
            "protected {raw_path:?}"
        );
    }

    for (index, case) in fixture["cleanPlans"]
        .as_array()
        .expect("cleanPlans")
        .iter()
        .enumerate()
    {
        let candidates: Vec<StorageCleanCandidate> = case["candidates"]
            .as_array()
            .expect("clean candidates")
            .iter()
            .map(|candidate| StorageCleanCandidate {
                relative_path: candidate["relativePath"]
                    .as_str()
                    .expect("candidate relativePath")
                    .to_string(),
                bytes: candidate["bytes"].as_i64().expect("candidate bytes"),
                mtime_ms: candidate["mtimeMs"].as_i64().expect("candidate mtimeMs"),
            })
            .collect();
        let plan = plan_storage_clean(&StorageCleanParams {
            category_id: category_id(case["categoryId"].as_str().expect("categoryId")),
            candidates: &candidates,
            context: context(&case["context"]),
            now: case["now"].as_i64().expect("now"),
            offset_secs: case["offsetSecs"].as_i64().expect("offsetSecs") as i32,
        });
        assert_eq!(plan.to_json(), case["expected"], "cleanPlans[{index}]");
    }

    for (index, case) in fixture["scanJobs"]
        .as_array()
        .expect("scanJobs")
        .iter()
        .enumerate()
    {
        let roots: Vec<StorageRootSpec> = case["roots"]
            .as_array()
            .expect("scan roots")
            .iter()
            .map(|root| StorageRootSpec {
                id: root_id(root["id"].as_str().expect("root id")),
                path: root["path"].as_str().expect("root path").to_string(),
                has_custom_data_base_dir: root["hasCustomDataBaseDir"]
                    .as_bool()
                    .expect("root hasCustomDataBaseDir"),
            })
            .collect();
        let mut job = StorageScanJob::new(
            "scan-fixture".to_string(),
            &roots,
            case["startNow"].as_i64().expect("startNow"),
            case["throttleMs"].as_i64().expect("throttleMs"),
        );
        let mut emissions = Vec::new();
        let mut timer_delays = Vec::new();
        for step in case["steps"].as_array().expect("scan steps") {
            let now = step["now"].as_i64().expect("step now");
            match step["type"].as_str().expect("step type") {
                "progress" => match job.progress(scan_progress(step), now) {
                    ScanJobProgressAction::Emitted(snapshot) => {
                        emissions.push(snapshot.to_json());
                    }
                    ScanJobProgressAction::TimerScheduled { delay_ms } => {
                        timer_delays.push(serde_json::json!(delay_ms));
                    }
                    ScanJobProgressAction::Ignored => {}
                },
                "timer" => {
                    if let Some(snapshot) = job.timer_fired(now) {
                        emissions.push(snapshot.to_json());
                    }
                }
                "finish" => {
                    emissions.push(job.finish_success(scan_progress(step), now).to_json());
                }
                "fail" => {
                    let is_abort_error = step["abort"].as_bool().unwrap_or(false);
                    emissions.push(
                        job.finish_error(
                            ScanJobFailure {
                                is_abort_error,
                                code: step["code"].as_str().expect("failure code").to_string(),
                            },
                            now,
                        )
                        .to_json(),
                    );
                }
                "cancel" => {
                    job.cancel();
                    emissions.push(
                        job.finish_error(
                            ScanJobFailure {
                                is_abort_error: true,
                                code: "AbortError".to_string(),
                            },
                            now,
                        )
                        .to_json(),
                    );
                }
                other => panic!("未知 scan step: {other}"),
            }
        }
        let expected = &case["expected"];
        assert_eq!(
            Value::Array(emissions),
            expected["emissions"],
            "scanJobs[{index}] emissions"
        );
        assert_eq!(
            Value::Array(timer_delays),
            expected["timerDelays"],
            "scanJobs[{index}] timerDelays"
        );
        assert_eq!(
            job.snapshot().to_json(),
            expected["final"],
            "scanJobs[{index}] final"
        );
    }

    for (index, case) in fixture["snapshots"]
        .as_array()
        .expect("snapshots")
        .iter()
        .enumerate()
    {
        let spec = &case["spec"];
        let root_spec = StorageRootSpec {
            id: root_id(spec["id"].as_str().expect("spec id")),
            path: spec["path"].as_str().expect("spec path").to_string(),
            has_custom_data_base_dir: spec["hasCustomDataBaseDir"]
                .as_bool()
                .expect("spec hasCustomDataBaseDir"),
        };
        let max_entries = case["maxEntriesPerCategory"]
            .as_u64()
            .map(|value| value as usize);
        let volume = volume(&case["volume"]);
        let mut accumulator = StorageUsageAccumulator::new(root_spec, max_entries);
        for entry in case["entries"].as_array().expect("entries") {
            accumulator.add(scan_entry(entry));
        }
        let actual = accumulator.snapshot(volume.as_ref()).to_json();
        assert_eq!(actual, case["expected"], "snapshot[{index}]");
    }

    for (index, case) in fixture["volumeGroups"]
        .as_array()
        .expect("volumeGroups")
        .iter()
        .enumerate()
    {
        let roots = case["roots"]
            .as_array()
            .expect("volume group roots")
            .iter()
            .map(root_usage)
            .collect();
        let actual: Vec<Value> = group_storage_roots_by_volume(roots)
            .iter()
            .map(|group| group.to_json())
            .collect();
        assert_eq!(
            Value::Array(actual),
            case["expected"],
            "volumeGroups[{index}]"
        );
    }
}
