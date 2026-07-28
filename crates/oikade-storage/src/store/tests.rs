use super::*;

#[test]
fn creates_marker_and_namespaced_state() {
    let root = tempfile::tempdir().unwrap();
    let state = root.path().join("state");
    let storage = Storage::open(&state).unwrap();
    assert_eq!(storage.path(), state.join(DATABASE_FILENAME));
    assert!(state.join(MARKER_FILENAME).is_file());

    let bucket = storage.bucket(Namespace::Plugins).scope("example").unwrap();
    bucket.set("b.state.json", b"b").unwrap();
    bucket.set("a.state.json", b"a").unwrap();
    assert_eq!(bucket.get("a.state.json").unwrap(), b"a");
    assert_eq!(
        bucket.keys_with_suffix(".state.json").unwrap(),
        vec!["a.state.json", "b.state.json"]
    );
    bucket.delete("a.state.json").unwrap();
    assert!(matches!(
        bucket.get("a.state.json"),
        Err(StorageError::NotFound)
    ));
}

#[test]
fn refuses_database_without_marker() {
    let root = tempfile::tempdir().unwrap();
    let state = root.path().join("state");
    fs::create_dir(&state).unwrap();
    fs::write(state.join(DATABASE_FILENAME), b"unknown").unwrap();
    assert!(matches!(
        Storage::open(&state),
        Err(StorageError::AmbiguousState(_))
    ));
}
