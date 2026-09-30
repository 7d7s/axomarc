//! Wave 6 P4: bundled static IANA → fixed UTC-offset table (docs/19).
//!
//! Zero-dep by policy (D-62: `chrono-tz` would be a new external
//! dependency), so offsets are FIXED: DST transitions are not modeled.
//! Zones that observe DST carry their standard-time offset (documented
//! divergence — exact half the year, off by one hour during summer time);
//! zones that never observe DST — including the platform default
//! `Asia/Kolkata` — are exact. An unknown name refuses `register` with a
//! registration error instead of silently running in the wrong zone.

/// UTC offset in seconds for a bundled IANA name (or its common alias),
/// `None` when the name is not in the table.
pub fn timezone_offset_secs(name: &str) -> Option<i32> {
    const HOUR: i32 = 3600;
    Some(match name {
        "UTC" | "Etc/UTC" | "Etc/GMT" | "GMT" => 0,
        // South Asia — no DST.
        "Asia/Kolkata" | "Asia/Calcutta" | "Asia/Colombo" => 5 * HOUR + 1800,
        "Asia/Karachi" => 5 * HOUR,
        "Asia/Kathmandu" => 5 * HOUR + 45 * 60,
        "Asia/Dhaka" => 6 * HOUR,
        // Gulf — no DST.
        "Asia/Dubai" | "Asia/Muscat" | "Asia/Bahrain" | "Asia/Riyadh" | "Asia/Qatar"
        | "Asia/Kuwait" | "Asia/Baghdad" => 4 * HOUR,
        // South-East Asia — no DST.
        "Asia/Bangkok" | "Asia/Jakarta" | "Asia/Ho_Chi_Minh" | "Asia/Saigon"
        | "Asia/Vientiane" | "Asia/Phnom_Penh" => 7 * HOUR,
        "Asia/Singapore" | "Asia/Kuala_Lumpur" | "Asia/Shanghai" | "Asia/Hong_Kong"
        | "Asia/Taipei" | "Asia/Macau" | "Australia/Perth" => 8 * HOUR,
        "Asia/Tokyo" | "Asia/Seoul" => 9 * HOUR,
        // Australia (standard time for the DST states).
        "Australia/Adelaide" => 9 * HOUR + 1800,
        "Australia/Brisbane" => 10 * HOUR,
        "Australia/Sydney" | "Australia/Melbourne" => 10 * HOUR,
        // Africa — mostly no DST (standard time where observed).
        "Africa/Lagos" | "Africa/Accra" | "Africa/Abidjan" => HOUR,
        "Africa/Cairo" | "Africa/Johannesburg" | "Africa/Harare" => 2 * HOUR,
        "Africa/Nairobi" | "Africa/Addis_Ababa" => 3 * HOUR,
        // Europe — standard time (GMT / CET+1 / EET+2; summer time +1 not modeled).
        "Europe/London" | "Europe/Dublin" | "Europe/Lisbon" => 0,
        "Europe/Berlin" | "Europe/Paris" | "Europe/Madrid" | "Europe/Rome"
        | "Europe/Amsterdam" | "Europe/Brussels" | "Europe/Vienna" | "Europe/Prague"
        | "Europe/Warsaw" | "Europe/Stockholm" | "Europe/Oslo" | "Europe/Copenhagen"
        | "Europe/Zurich" | "Europe/Budapest" => HOUR,
        "Europe/Helsinki" | "Europe/Athens" | "Europe/Bucharest" | "Europe/Kyiv"
        | "Europe/Kiev" | "Europe/Sofia" | "Europe/Riga" | "Europe/Tallinn"
        | "Europe/Vilnius" => 2 * HOUR,
        // Moscow/Istanbul/Kaliningrad — no DST at these offsets.
        "Europe/Moscow" | "Europe/Istanbul" | "Europe/Kaliningrad" => 3 * HOUR,
        // Americas — standard time (EST−5 / CST−6 / MST−7 / PST−8; summer
        // time +1 not modeled). Phoenix and Mexico City have no DST today.
        "America/New_York" | "America/Toronto" | "America/Montreal" | "America/Havana"
        | "America/Lima" | "America/Bogota" | "America/Caracas" | "America/Halifax" => -5 * HOUR,
        "America/Chicago" | "America/Mexico_City" | "America/Winnipeg"
        | "America/Guatemala" | "America/Panama" => -6 * HOUR,
        "America/Denver" | "America/Phoenix" | "America/Edmonton" | "America/Tegucigalpa"
        | "America/El_Salvador" | "America/Managua" => -7 * HOUR,
        "America/Los_Angeles" | "America/Vancouver" | "America/Tijuana" => -8 * HOUR,
        "America/Anchorage" => -9 * HOUR,
        "Pacific/Honolulu" => -10 * HOUR,
        // South America — no DST (Argentina/São Paulo abolished it).
        "America/Sao_Paulo" | "America/Argentina/Buenos_Aires" => -3 * HOUR,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_offsets_are_exact_for_non_dst_zones() {
        assert_eq!(timezone_offset_secs("UTC"), Some(0));
        assert_eq!(timezone_offset_secs("Etc/UTC"), Some(0));
        // The platform default (Locale::default()) — no DST, exact.
        assert_eq!(timezone_offset_secs("Asia/Kolkata"), Some(19_800));
        assert_eq!(timezone_offset_secs("Asia/Calcutta"), Some(19_800));
        assert_eq!(timezone_offset_secs("Asia/Dubai"), Some(14_400));
        assert_eq!(timezone_offset_secs("Asia/Kathmandu"), Some(20_700));
        assert_eq!(timezone_offset_secs("Asia/Tokyo"), Some(32_400));
        assert_eq!(timezone_offset_secs("Africa/Lagos"), Some(3_600));
        assert_eq!(timezone_offset_secs("America/Sao_Paulo"), Some(-10_800));
    }

    #[test]
    fn dst_zones_carry_their_standard_time_offset() {
        // Documented divergence: fixed offsets, no DST transitions.
        assert_eq!(timezone_offset_secs("America/New_York"), Some(-18_000));
        assert_eq!(timezone_offset_secs("America/Chicago"), Some(-21_600));
        assert_eq!(timezone_offset_secs("America/Los_Angeles"), Some(-28_800));
        assert_eq!(timezone_offset_secs("Europe/London"), Some(0));
        assert_eq!(timezone_offset_secs("Europe/Berlin"), Some(3_600));
        assert_eq!(timezone_offset_secs("Australia/Sydney"), Some(36_000));
    }

    #[test]
    fn unknown_names_refuse_registration() {
        assert_eq!(timezone_offset_secs("Mars/Olympus_Mons"), None);
        assert_eq!(timezone_offset_secs("asia/kolkata"), None); // case-sensitive
        assert_eq!(timezone_offset_secs(""), None);
    }
}
