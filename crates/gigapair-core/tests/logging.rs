//! The log file is created and records panics (own test binary: logging
//! is process-wide).

use gigapair_core::dirs::AppDirs;

#[test]
fn writes_daily_file_and_records_panics() {
    let tmp = tempfile::tempdir().unwrap();
    let dirs = AppDirs::under(tmp.path());
    dirs.create_all().unwrap();
    let guard = gigapair_core::logging::init(&dirs, "test-app", false);

    tracing::info!("hello from the test");
    let result = std::thread::Builder::new()
        .name("doomed".into())
        .spawn(|| panic!("boom for the log"))
        .unwrap()
        .join();
    assert!(result.is_err());
    drop(guard); // flushes

    let files: Vec<_> = std::fs::read_dir(&dirs.logs)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    assert_eq!(files.len(), 1, "{files:?}");
    let name = files[0].file_name().unwrap().to_string_lossy().into_owned();
    assert!(
        name.starts_with("test-app.") && name.ends_with(".log"),
        "{name}"
    );
    let text = std::fs::read_to_string(&files[0]).unwrap();
    assert!(text.contains("hello from the test"), "{text}");
    assert!(text.contains("panic: "), "{text}");
    assert!(text.contains("boom for the log"), "{text}");
    assert!(text.contains("doomed"), "{text}");
}
