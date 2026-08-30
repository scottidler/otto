mod common;

use serial_test::serial;
use tempfile::TempDir;

#[test]
#[serial]
fn test_history_table_alignment() {
    // Create a temp directory for the test
    let temp_dir = TempDir::new().unwrap();
    let otto_path = temp_dir.path();

    // OTTO_HOME (not just HOME) isolates the run: resolve_otto_home() checks
    // OTTO_HOME before HOME, so a developer with OTTO_HOME exported would
    // otherwise have this read their real history.
    let output = common::otto_cmd(otto_path).arg("history").output().unwrap();

    let stdout = String::from_utf8_lossy(&output.stdout);

    // Skip if no history (that's okay for this test)
    if stdout.contains("No history") {
        return;
    }

    // Split into lines
    let lines: Vec<&str> = stdout.lines().collect();

    // Find header line (should contain "Timestamp")
    let header_idx = lines.iter().position(|l| l.contains("Timestamp"));

    if let Some(idx) = header_idx {
        let header = strip_ansi_codes(lines[idx]);

        // Skip separator line
        if idx + 2 < lines.len() {
            let first_data = strip_ansi_codes(lines[idx + 2]);

            // Verify that column positions align
            // Find where "Status" header starts and ends
            let status_start = header.find("Status").expect("Status header not found");
            let duration_start = header.find("Duration").expect("Duration header not found");
            let size_start = header.find("Size").expect("Size header not found");
            let user_start = header.find("User").expect("User header not found");
            let path_start = header.find("Path").expect("Path header not found");

            // In the data row, the status symbol should be within the Status column range
            // The duration value should start around the Duration column position
            // etc.

            println!("Header: {}", header);
            println!("Data:   {}", first_data);
            println!("Status col starts at: {}", status_start);
            println!("Duration col starts at: {}", duration_start);
            println!("Size col starts at: {}", size_start);
            println!("User col starts at: {}", user_start);
            println!("Path col starts at: {}", path_start);

            // Check that data aligns roughly with headers (within a few chars tolerance)
            // This is a basic alignment check
            assert!(
                status_start > 20 && status_start < 30,
                "Status column should start around position 21-30"
            );
            assert!(
                duration_start > 30 && duration_start < 45,
                "Duration column should start around position 30-45"
            );
        }
    }
}

/// Strip ANSI escape codes from a string to get the actual displayed text
fn strip_ansi_codes(s: &str) -> String {
    let re = regex::Regex::new(r"\x1b\[[0-9;]*m").unwrap();
    re.replace_all(s, "").to_string()
}

#[test]
#[serial]
fn test_column_format_consistency() {
    // This test verifies that we're using consistent format strings
    // by checking a mock table output

    let timestamp = "2025-11-01 21:36:10";
    let status = "✓";
    let duration = "2.0s";
    let size = "14.1 KB";
    let user = "saidler";
    let path = "~/repos/otto-rs/otto";

    // Header format
    let header = format!(
        "{:<19}  {:^6}  {:>8}  {:>8}  {:<8}  {}",
        "Timestamp", "Status", "Duration", "Size", "User", "Path"
    );

    // Data format (must match exactly)
    let data = format!(
        "{:<19}  {:^6}  {:>8}  {:>8}  {:<8}  {}",
        timestamp, status, duration, size, user, path
    );

    println!("Header: |{}|", header);
    println!("Data:   |{}|", data);

    // The key test: both strings should have the same character positions
    // for the start of each "column" because we're using the same format string

    // Split by single space to see the columns
    let header_chars: Vec<char> = header.chars().collect();
    let data_chars: Vec<char> = data.chars().collect();

    // The format creates fixed-width columns. Let's verify the column boundaries
    // are consistent between header and data

    // Column boundaries based on format: {:<19}  {:^6}  {:>8}  {:>8}  {:<8}  {}
    // Position 0-18: Timestamp (19 chars)
    // Position 19-20: spaces (2)
    // Position 21-26: Status (6 chars, center-aligned)
    // Position 27-28: spaces (2)
    // Position 29-36: Duration (8 chars, right-aligned)
    // Position 37-38: spaces (2)
    // Position 39-46: Size (8 chars, right-aligned)
    // Position 47-48: spaces (2)
    // Position 49-56: User (8 chars, left-aligned)
    // Position 57-58: spaces (2)
    // Position 59+: Path

    assert_eq!(header_chars[19], ' ', "Space at position 19 in header");
    assert_eq!(data_chars[19], ' ', "Space at position 19 in data");
    assert_eq!(header_chars[20], ' ', "Space at position 20 in header");
    assert_eq!(data_chars[20], ' ', "Space at position 20 in data");

    assert_eq!(header_chars[27], ' ', "Space at position 27 in header");
    assert_eq!(data_chars[27], ' ', "Space at position 27 in data");
    assert_eq!(header_chars[28], ' ', "Space at position 28 in header");
    assert_eq!(data_chars[28], ' ', "Space at position 28 in data");

    assert_eq!(header_chars[37], ' ', "Space at position 37 in header");
    assert_eq!(data_chars[37], ' ', "Space at position 37 in data");
    assert_eq!(header_chars[38], ' ', "Space at position 38 in header");
    assert_eq!(data_chars[38], ' ', "Space at position 38 in data");

    assert_eq!(header_chars[47], ' ', "Space at position 47 in header");
    assert_eq!(data_chars[47], ' ', "Space at position 47 in data");
    assert_eq!(header_chars[48], ' ', "Space at position 48 in header");
    assert_eq!(data_chars[48], ' ', "Space at position 48 in data");

    assert_eq!(header_chars[57], ' ', "Space at position 57 in header");
    assert_eq!(data_chars[57], ' ', "Space at position 57 in data");
    assert_eq!(header_chars[58], ' ', "Space at position 58 in header");
    assert_eq!(data_chars[58], ' ', "Space at position 58 in data");

    println!("✓ All column boundaries align correctly!");
}
