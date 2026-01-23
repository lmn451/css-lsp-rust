use css_variable_lsp::workspace::ScanStats;

#[tokio::test]
async fn test_workspace_scan_stats_methods() {
    let mut stats = ScanStats::default();

    assert_eq!(stats.summary(), "0 files scanned");
    assert_eq!(stats.error_details(), None);

    stats.files_matched = 10;
    stats.files_parsed = 7;
    stats.read_errors = 2;
    stats.parse_errors = 1;
    stats.add_error("Failed to parse CSS: unexpected token".to_string());
    stats.add_error("Failed to read file: permission denied".to_string());

    let summary = stats.summary();
    assert!(summary.contains("10 files scanned"));
    assert!(summary.contains("2 read errors"));
    assert!(summary.contains("1 parse errors"));

    let error_details = stats.error_details();
    assert!(error_details.is_some());
    let details = error_details.unwrap();
    assert!(details.contains("Failed to parse CSS"));
    assert!(details.contains("Failed to read file"));
}

#[tokio::test]
async fn test_workspace_scan_stats_add_error() {
    let mut stats = ScanStats::default();

    for i in 0..3 {
        stats.add_error(format!("Error {}", i));
    }
    assert_eq!(stats.error_samples.len(), 3);

    for i in 0..10 {
        stats.add_error(format!("Extra error {}", i));
    }
    assert_eq!(stats.error_samples.len(), 5);

    for i in 0..3 {
        assert!(stats.error_samples[i].contains(&format!("Error {}", i)));
    }
}
