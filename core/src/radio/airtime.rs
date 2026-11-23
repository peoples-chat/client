//! Airtime (#51): how long a packet occupies the channel, and how much of it the app may use.
//!
//! **Time on air** follows Semtech's LoRa formula (SX127x and SX126x datasheets, AN1200.13),
//! for an explicit header and a CRC:
//!
//! ```text
//! symbol    = 2^SF / bandwidth
//! preamble  = (preamble symbols + 4.25) × symbol
//! payload   = 8 + max(ceil((8·bytes − 4·SF + 28 + 16) / (4·(SF − 2·LDRO))) × CR, 0) symbols
//! ```
//!
//! where CR is the coding rate's denominator (5 to 8 for 4/5 to 4/8), and LDRO, low data rate
//! optimisation, is on when a symbol lasts longer than 16 ms.
//!
//! **A MeshCore text message** on air holds a header, a path length, the path, the two hashes
//! and MAC, and the encrypted timestamp, flags and text in 16-byte blocks. Hardware day checks
//! these figures against measurements (#36).
//!
//! **The cap.** EU rules allow a duty cycle of 10% in 869.4 to 869.65 MHz (CEPT ERC
//! Recommendation 70-03, Annex 1): 360 seconds an hour. The app takes a tenth of that for what
//! the user sends, so that the radio's own traffic, its acknowledgements and the shared mesh have
//! room: [`HOURLY_AIRTIME`].

use std::time::Duration;

use super::RadioSettings;

/// The airtime the app may use for the user's messages in any hour: 1% of the hour.
pub const HOURLY_AIRTIME: Duration = Duration::from_secs(36);
/// The preamble MeshCore's radios send, in symbols. A longer preamble than LoRa's usual 8, to
/// err on the side of more airtime until hardware day measures it.
const PREAMBLE_SYMBOLS: f64 = 16.0;

/// How long a LoRa packet of `bytes` takes on air with `settings`.
pub fn time_on_air(settings: &RadioSettings, bytes: usize) -> Duration {
    let sf = f64::from(settings.spreading_factor);
    let bandwidth_hz = settings.bandwidth_khz * 1000.0;
    if bandwidth_hz <= 0.0 {
        return Duration::MAX;
    }
    let symbol = 2f64.powf(sf) / bandwidth_hz;
    let low_data_rate = if symbol > 0.016 { 1.0 } else { 0.0 };
    let coding_rate = f64::from(settings.coding_rate.clamp(5, 8));
    let bits = 8.0 * bytes as f64 - 4.0 * sf + 28.0 + 16.0;
    let per_block = 4.0 * (sf - 2.0 * low_data_rate);
    let blocks = if per_block > 0.0 {
        (bits / per_block).ceil().max(0.0)
    } else {
        0.0
    };
    let symbols = PREAMBLE_SYMBOLS + 4.25 + 8.0 + blocks * coding_rate;
    Duration::from_secs_f64(symbols * symbol)
}

/// How many bytes a MeshCore text message with `text_bytes` of text takes on air, sent directly
/// or as the start of a flood: header, path length, the two hashes and MAC, and the encrypted
/// timestamp, flags and text in 16-byte blocks.
pub fn meshcore_text_bytes(text_bytes: usize) -> usize {
    let encrypted = (4 + 1 + text_bytes).div_ceil(16) * 16;
    1 + 1 + 1 + 1 + 2 + encrypted
}

#[cfg(test)]
mod tests {
    use super::*;

    /// MeshCore's EU/UK narrow preset.
    const NARROW: RadioSettings = RadioSettings {
        frequency_mhz: 869.618,
        bandwidth_khz: 62.5,
        spreading_factor: 8,
        coding_rate: 8,
        tx_power_dbm: 22,
    };

    fn close(actual: Duration, expected_ms: f64) {
        let ms = actual.as_secs_f64() * 1000.0;
        assert!(
            (ms - expected_ms).abs() < 0.5,
            "{ms} ms, not {expected_ms} ms"
        );
    }

    #[test]
    fn matches_the_formula_worked_by_hand() {
        // SF8 at 62.5 kHz: a symbol lasts 4.096 ms, so no low data rate optimisation.
        // 10 bytes: ceil((80 - 32 + 44) / 32) = 3 blocks of 8 symbols, plus 8, plus a preamble
        // of 20.25: 52.25 symbols.
        close(time_on_air(&NARROW, 10), 52.25 * 4.096);
        // The longest packet the app sends: 160 characters of base64 in a text message.
        assert_eq!(meshcore_text_bytes(160), 182);
        // ceil((1456 - 32 + 44) / 32) = 46 blocks: 8 + 368 + 20.25 symbols.
        close(time_on_air(&NARROW, 182), 396.25 * 4.096);
    }

    #[test]
    fn slow_settings_use_low_data_rate_optimisation() {
        // SF12 at 125 kHz: a symbol lasts 32.768 ms, over 16 ms.
        let slow = RadioSettings {
            spreading_factor: 12,
            bandwidth_khz: 125.0,
            coding_rate: 5,
            ..NARROW
        };
        // ceil((80 - 48 + 44) / (4 × 10)) = 2 blocks of 5 symbols.
        close(time_on_air(&slow, 10), (20.25 + 8.0 + 10.0) * 32.768);
    }

    #[test]
    fn the_cap_holds_a_few_dozen_messages_an_hour() {
        let longest = time_on_air(&NARROW, meshcore_text_bytes(160));
        let short = time_on_air(&NARROW, meshcore_text_bytes(60));
        let fit = |each: Duration| (HOURLY_AIRTIME.as_secs_f64() / each.as_secs_f64()) as u32;
        assert_eq!(fit(longest), 22);
        assert!(fit(short) >= 40, "{}", fit(short));
    }
}
