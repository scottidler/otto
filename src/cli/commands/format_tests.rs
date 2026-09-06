#![cfg(test)]

use super::*;

#[test]
fn a_byte_count_under_a_kilobyte_is_printed_in_bytes() {
    assert_eq!(format_size(0), "0 B");
    assert_eq!(format_size(1023), "1023 B");
}

#[test]
fn kilobytes_and_megabytes_carry_one_decimal_place() {
    assert_eq!(format_size(1024), "1.0 KB");
    assert_eq!(format_size(1024 * 1024), "1.0 MB");
    assert_eq!(format_size(1536 * 1024), "1.5 MB");
}

#[test]
fn gigabytes_carry_two_decimal_places_everywhere() {
    // The drift this module exists to remove: `Clean` printed `.1` here while
    // `History` and `Stats` printed `.2`, so the same run reported two sizes.
    assert_eq!(format_size(1024 * 1024 * 1024), "1.00 GB");
    assert_eq!(format_size(1024 * 1024 * 1024 * 3 / 2), "1.50 GB");
}

#[test]
fn a_sub_second_duration_is_printed_in_milliseconds() {
    assert_eq!(format_duration(0.25), "250ms");
    assert_eq!(format_duration(0.999), "999ms");
}

#[test]
fn durations_step_up_through_seconds_minutes_and_hours() {
    assert_eq!(format_duration(1.0), "1.0s");
    assert_eq!(format_duration(59.9), "59.9s");
    assert_eq!(format_duration(90.0), "1m30s");
    assert_eq!(format_duration(3600.0), "1h0m");
    assert_eq!(format_duration(7380.0), "2h3m");
}

#[test]
fn a_timestamp_renders_as_local_wall_clock_time() {
    // Pinned against `chrono`'s own conversion rather than a literal, because
    // the answer depends on the host's zone. What is pinned is that it is
    // LOCAL: `Clean` used to render the same run in UTC.
    let expected = Local
        .timestamp_opt(1_700_000_000, 0)
        .single()
        .expect("1700000000 is representable in every zone")
        .format("%Y-%m-%d %H:%M:%S")
        .to_string();
    assert_eq!(format_timestamp(1_700_000_000), expected);
}

#[test]
fn an_out_of_range_timestamp_falls_back_to_the_epoch() {
    // The fallback needs a value chrono genuinely cannot represent. `u64::MAX`
    // is NOT one: as an i64 it is -1, one second before the epoch, which is
    // perfectly representable -- so the earlier version of this test never
    // reached the fallback at all and asserted only that some string came back.
    let epoch = Local
        .timestamp_opt(0, 0)
        .single()
        .expect("the epoch is representable in every zone")
        .format("%Y-%m-%d %H:%M:%S")
        .to_string();
    assert_eq!(format_timestamp(i64::MAX as u64), epoch);
}

#[test]
fn a_timestamp_that_wraps_negative_renders_as_just_before_the_epoch() {
    // Pins the honest behavior of the wrap rather than pretending it is a
    // fallback: `u64::MAX as i64 == -1`.
    let one_before = Local
        .timestamp_opt(-1, 0)
        .single()
        .expect("-1 is representable in every zone")
        .format("%Y-%m-%d %H:%M:%S")
        .to_string();
    assert_eq!(format_timestamp(u64::MAX), one_before);
}
