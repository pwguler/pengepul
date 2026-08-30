use pengepul::cloaking_versions::{Cache, Version, codex_release, effective, npm_latest};
use serde_json::json;

#[test]
fn a_version_orders_numerically_not_lexically() {
    let a: Version = "2.1.88".parse().expect("semver");
    let b: Version = "2.1.251".parse().expect("semver");
    assert!(a < b, "2.1.251 is newer than 2.1.88");
    assert_eq!(b.to_string(), "2.1.251");
    assert!("v2.1.0".parse::<Version>().is_err());
    assert!("2.1".parse::<Version>().is_err());
    assert!("latest".parse::<Version>().is_err());
}

#[test]
fn npm_latest_reads_the_latest_dist_tag() {
    let body = json!({"dist-tags": {"stable": "2.1.236", "latest": "2.1.251"}});
    assert_eq!(npm_latest(&body), "2.1.251".parse().ok());
    assert_eq!(npm_latest(&json!({"dist-tags": {"latest": "soon"}})), None);
    assert_eq!(npm_latest(&json!({})), None);
}

#[test]
fn codex_release_strips_the_rust_v_prefix() {
    // AC-6
    let body = json!({"tag_name": "rust-v0.151.0"});
    assert_eq!(codex_release(&body), "0.151.0".parse().ok());
    assert_eq!(codex_release(&json!({"tag_name": "nightly"})), None);
    assert_eq!(codex_release(&json!({})), None);
}

#[test]
fn the_configured_version_is_a_floor_under_the_fetched_one() {
    // AC-2
    let fetched: Version = "2.1.251".parse().expect("semver");
    assert_eq!(effective("2.1.88", Some(&fetched)), "2.1.251");
    assert_eq!(effective("2.1.300", Some(&fetched)), "2.1.300");
    assert_eq!(effective("2.1.88", None), "2.1.88");
    // A configured value that is not semver is the operator's explicit choice.
    assert_eq!(effective("custom-build", Some(&fetched)), "custom-build");
}

#[test]
fn the_cache_round_trips_and_a_missing_file_is_empty() {
    // AC-5
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("cloaking-versions.json");

    let empty = Cache::load(&path);
    assert_eq!(empty.claude, None);
    assert_eq!(empty.codex, None);

    let cache = Cache {
        claude: "2.1.251".parse().ok(),
        codex: "0.151.0".parse().ok(),
    };
    cache.save(&path).expect("save");
    let loaded = Cache::load(&path);
    assert_eq!(loaded, cache);
    assert!(
        !tmp.path().join("cloaking-versions.json.tmp").exists(),
        "the temp file is renamed away"
    );

    std::fs::write(&path, "not json").expect("corrupt");
    let corrupt = Cache::load(&path);
    assert_eq!(corrupt.claude, None, "an unreadable cache is ignored");
}
