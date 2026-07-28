use super::*;

#[test]
fn level_round_trip_is_bounded() {
    assert_eq!(matter_level(0.0), 1);
    assert_eq!(matter_level(100.0), 254);
    assert_eq!(canonical_level(1), 0.0);
    assert_eq!(canonical_level(254), 100.0);
}

#[test]
fn unique_ids_are_stable() {
    assert_eq!(unique_id("device", "on"), unique_id("device", "on"));
    assert_ne!(unique_id("device", "on"), unique_id("device", "level"));
}

#[test]
fn utf8_truncation_preserves_boundaries() {
    assert!(truncate_utf8(&"é".repeat(20), 32).is_char_boundary(32));
    assert_eq!(truncate_utf8(&"é".repeat(20), 32).len(), 32);
}
