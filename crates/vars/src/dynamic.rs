//! Appendix A dynamic variables (v1 set), resolved freshly per occurrence (PROMPT.md 5.2).
//! Generators use `fastrand` and `uuid`, and word lists written for this project
//! (`words.rs`, contributor-original -- recorded in ATTRIBUTION.toml). Shapes and ranges must
//! hold; the exact strings are ours. The table is the single source of names and generators.

use crate::words;

/// Whether `name` (braces stripped, `$` prefix included) is a known dynamic variable.
pub fn is_known(name: &str) -> bool {
    lookup(name).is_some()
}

/// Generate one value for a dynamic variable name; `None` when the name has no generator (the
/// caller reports it unresolved — see `Resolver::raw_value`).
pub fn try_generate(name: &str) -> Option<String> {
    let generator = lookup(name)?;
    Some(generator(Gen))
}

/// All dynamic variable names (Appendix A v1), for the autocomplete list and tests.
pub const KNOWN_NAMES: &[&str] = &[
    // Common
    "$guid",
    "$randomUUID",
    "$timestamp",
    "$isoTimestamp",
    // Numbers and text
    "$randomInt",
    "$randomAlphaNumeric",
    "$randomBoolean",
    "$randomHexColor",
    "$randomColor",
    "$randomPrice",
    "$randomSemver",
    // Internet
    "$randomIP",
    "$randomIPV6",
    "$randomMACAddress",
    "$randomPassword",
    "$randomUserAgent",
    "$randomProtocol",
    "$randomDomainName",
    "$randomDomainSuffix",
    "$randomDomainWord",
    "$randomUrl",
    "$randomEmail",
    "$randomExampleEmail",
    "$randomUserName",
    // People and places
    "$randomFirstName",
    "$randomLastName",
    "$randomFullName",
    "$randomNamePrefix",
    "$randomNameSuffix",
    "$randomPhoneNumber",
    "$randomCity",
    "$randomStreetName",
    "$randomStreetAddress",
    "$randomCountry",
    "$randomCountryCode",
    "$randomLatitude",
    "$randomLongitude",
    "$randomJobTitle",
    "$randomCompanyName",
    // Dates
    "$randomDateFuture",
    "$randomDatePast",
    "$randomDateRecent",
    "$randomWeekday",
    "$randomMonth",
    // Finance
    "$randomCurrencyCode",
    "$randomCurrencyName",
    "$randomCurrencySymbol",
    "$randomBankAccount",
    "$randomBitcoin",
    // Text
    "$randomWord",
    "$randomWords",
    "$randomPhrase",
    "$randomLoremWord",
    "$randomLoremWords",
    "$randomLoremSentence",
    "$randomLoremSentences",
    "$randomLoremParagraph",
    "$randomLoremParagraphs",
    "$randomLoremText",
    "$randomLoremSlug",
    // Files
    "$randomFileName",
    "$randomFileExt",
    "$randomFileType",
    "$randomMimeType",
    "$randomFilePath",
];

type Generator = fn(Gen) -> String;

fn lookup(name: &str) -> Option<Generator> {
    const TABLE: &[(&str, Generator)] = &[
        // Common (Appendix A: Common)
        ("$guid", guid),
        ("$randomUUID", guid),
        ("$timestamp", timestamp),
        ("$isoTimestamp", iso_now),
        // Numbers and text
        ("$randomInt", |g| g.range(0, 1001).to_string()),
        ("$randomAlphaNumeric", |g| g.alnum().to_string()),
        ("$randomBoolean", |g| (g.range(0, 2) == 0).to_string()),
        ("$randomHexColor", |g| format!("#{:02x}", g.range(0, 0x1000000))),
        ("$randomColor", |g| g.pick(words::COLORS).to_string()),
        ("$randomPrice", |g| format!("{:.2}", g.range(0, 100_001) as f64 / 100.0)),
        ("$randomSemver", |g| format!("{}.{}.{}", g.range(0, 10), g.range(0, 20), g.range(0, 30))),
        // Internet
        ("$randomIP", random_ip),
        ("$randomIPV6", |g| (0..8).map(|_i| format!("{:04x}", g.range(0, 0x10000))).collect::<Vec<String>>().join(":")),
        ("$randomMACAddress", random_mac),
        ("$randomPassword", |g| (0..15).map(|_| g.alnum()).collect::<String>()),
        ("$randomUserAgent", |g| g.pick(words::USER_AGENTS).to_string()),
        ("$randomProtocol", |g| if g.range(0, 2) == 0 { "http".into() } else { "https".into() }),
        ("$randomDomainName", |g| format!("{}.{}", g.pick(words::WORDS), g.pick(words::DOMAIN_SUFFIXES))),
        ("$randomDomainSuffix", |g| g.pick(words::DOMAIN_SUFFIXES).to_string()),
        ("$randomDomainWord", |g| g.pick(words::WORDS).to_string()),
        ("$randomUrl", |g| format!("https://www.example.{}/{}", g.pick(words::DOMAIN_SUFFIXES), g.pick(words::WORDS))),
        ("$randomEmail", |g| format!("{}.{}@example{}", user_name(g), g.range(0, 1000), g.pick(words::DOMAIN_SUFFIXES))),
        ("$randomExampleEmail", |g| format!("{}.{}@gmail.com", user_name(g), g.range(0, 1000))),
        ("$randomUserName", |g| format!("{}_{}", user_name(g), g.range(1, 10_000))),
        // People and places
        ("$randomFirstName", |g| g.pick(words::FIRST_NAMES).to_string()),
        ("$randomLastName", |g| g.pick(words::LAST_NAMES).to_string()),
        ("$randomFullName", |g| format!("{} {}", g.pick(words::FIRST_NAMES), g.pick(words::LAST_NAMES))),
        ("$randomNamePrefix", |g| g.pick(words::NAME_PREFIXES).to_string()),
        ("$randomNameSuffix", |g| g.pick(words::NAME_SUFFIXES).to_string()),
        ("$randomPhoneNumber", |g| format!("+1-555-{:04}-{:04}", g.range(0, 10_000), g.range(0, 10_000))),
        ("$randomCity", |g| g.pick(words::CITIES).to_string()),
        ("$randomStreetName", |g| g.pick(words::STREETS).to_string()),
        ("$randomStreetAddress", |g| format!("{} {}", g.range(1, 9999), g.pick(words::STREETS))),
        ("$randomCountry", |g| g.pick_t2(words::COUNTRIES).0.to_string()),
        ("$randomCountryCode", |g| g.pick_t2(words::COUNTRIES).1.to_string()),
        ("$randomLatitude", |g| format!("{:.6}", (g.range(0, 180_000_001) as f64 / 1_000_000.0) - 90.0)),
        ("$randomLongitude", |g| format!("{:.6}", (g.range(0, 360_000_001) as f64 / 1_000_000.0) - 180.0)),
        ("$randomJobTitle", |g| g.pick(words::JOB_TITLES).to_string()),
        ("$randomCompanyName", |g| format!("{} {}{}", g.pick(words::COMPANY_PREFIXES), g.pick(words::WORDS), g.pick(words::COMPANY_TAILS))),
        // Dates
        ("$randomDateFuture", |g| iso_of_offset_seconds(offset_seconds(g, 60, 365 * 86_400))),
        ("$randomDatePast", |g| iso_of_offset_seconds(-(offset_seconds(g, 60, 365 * 86_400)))),
        ("$randomDateRecent", |g| iso_of_offset_seconds(-(offset_seconds(g, 5, 3600)))),
        ("$randomWeekday", |g| g.pick(words::WEEKDAYS).to_string()),
        ("$randomMonth", |g| g.pick(words::MONTHS).to_string()),
        // Finance
        ("$randomCurrencyCode", |g| g.pick_t3(words::CURRENCIES).0.to_string()),
        ("$randomCurrencyName", |g| g.pick_t3(words::CURRENCIES).1.to_string()),
        ("$randomCurrencySymbol", |g| g.pick_t3(words::CURRENCIES).2.to_string()),
        ("$randomBankAccount", |g| (0..9).map(|_| g.range(0, 10).to_string()).collect::<String>()),
        ("$randomBitcoin", |g| format!("{:.8} BTC", g.range(0, 100_000_000_000) as f64 / 1e8)),
        // Text
        ("$randomWord", |g| g.pick(words::WORDS).to_string()),
        ("$randomWords", |g| lorem_words(g, 2, 6).join(" ")),
        ("$randomPhrase", |g| lorem_words(g, 2, 5).join(" ")),
        ("$randomLoremWord", |g| g.pick(words::LOREM).to_string()),
        ("$randomLoremWords", |g| lorem_words(g, 2, 6).join(" ")),
        ("$randomLoremSentence", lorem_sentence),
        ("$randomLoremSentences", |g| (0..g.range(2, 6)).map(|_| lorem_sentence(g)).collect::<Vec<String>>().join(" ")),
        ("$randomLoremParagraph", lorem_paragraph),
        ("$randomLoremParagraphs", |g| (0..g.range(2, 5)).map(|_| lorem_paragraph(g)).collect::<Vec<String>>().join("\n\n")),
        ("$randomLoremText", lorem_paragraph),
        ("$randomLoremSlug", |g| lorem_words(g, 2, 3).join("-")),
        // Files
        ("$randomFileName", |g| format!("{}.{}", g.pick(words::WORDS), g.pick(words::FILE_EXTS))),
        ("$randomFileExt", |g| g.pick(words::FILE_EXTS).to_string()),
        ("$randomFileType", |g| g.pick(words::FILE_TYPES).to_string()),
        ("$randomMimeType", |g| g.pick(words::MIME_TYPES).to_string()),
        ("$randomFilePath", |g| format!("/home/user/{}/{}.{}", g.pick(words::WORDS), g.pick(words::WORDS), g.pick(words::FILE_EXTS))),
    ];
    TABLE.iter().find(|(known, _)| *known == name).map(|(_, generator)| *generator)
}
/// THE pseudo-random interface the closures share; Copy because generators get it by value.
#[derive(Clone, Copy)]
pub struct Gen;

impl Gen {
    /// `lo .. hi` exclusive high end; `hi <= lo` yields `lo` (never panics).
    pub fn range(&self, lo: u64, hi: u64) -> u64 {
        if hi <= lo {
            return lo;
        }
        lo + fastrand::u64(0..(hi - lo))
    }

    /// An element of a list (empty lists yield `""`).
    pub fn pick<'a>(&self, list: &'a [&'a str]) -> &'a str {
        if list.is_empty() {
            return "";
        }
        let at = self.range(0, list.len() as u64) as usize;
        list[at]
    }

    /// An element of a list of 2-tuples (COUNTRIES shape).
    pub fn pick_t2(&self, list: &[(&'static str, &'static str)]) -> (&'static str, &'static str) {
        let at = self.range(0, list.len() as u64) as usize;
        list[at]
    }

    /// An element of a list of 3-tuples (CURRENCIES shape).
    pub fn pick_t3<'a>(&self, list: &'a [(&'a str, &'a str, &'a str)]) -> (&'a str, &'a str, &'a str) {
        let at = self.range(0, list.len() as u64) as usize;
        let (a, b, c) = list[at];
        (a, b, c)
    }

    /// A lowercase alphanum char.
    pub fn alnum(&self) -> char {
        const ALNUM: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
        ALNUM[self.range(0, ALNUM.len() as u64) as usize] as char
    }
}

fn guid(_g: Gen) -> String {
    uuid::Uuid::new_v4().to_string()
}

fn timestamp(_g: Gen) -> String {
    unix_secs().to_string()
}

/// ISO 8601 UTC with milliseconds (PROMPT.md Appendix A: `2026-10-19T08:10:36.177Z`).
fn iso_now(_g: Gen) -> String {
    iso_of_offset_seconds(0)
}

/// ISO 8601 UTC stamp `now + offset` seconds.
fn iso_of_offset_seconds(offset: i64) -> String {
    let secs = unix_secs() + offset;
    let date_among = secs.div_euclid(86_400);
    let day_secs = secs.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(date_among);
    let (hour, minute, second) = (day_secs / 3600, (day_secs % 3600) / 60, day_secs % 60);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.000Z")
}

/// Howard Hinnant's days-to-civil algorithm (no chrono dep), Gregorian proleptic.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097); // [0, 146096]
    let year_of_era = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = doe - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let mp = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { year + 1 } else { year };
    (year, month as u32, day as u32)
}

fn unix_secs() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// A positive random offset in `[min, max)` seconds, deduped in one place.
fn offset_seconds(g: Gen, min: u64, max: u64) -> i64 {
    g.range(min, max) as i64
}

fn random_ip(g: Gen) -> String {
    let mut octets = Vec::with_capacity(4);
    for _ in 0..4 {
        octets.push(format!("{}", g.range(0, 256)));
    }
    octets.join(".")
}

fn random_mac(g: Gen) -> String {
    let mut pairs = Vec::with_capacity(6);
    for _ in 0..6 {
        pairs.push(format!("{:02X}", g.range(0, 256)));
    }
    pairs.join(":")
}

fn user_name(g: Gen) -> String {
    g.pick(words::FIRST_NAMES).to_lowercase()
}

fn lorem_words(g: Gen, min: usize, max: usize) -> Vec<String> {
    let count = g.range(min as u64, max as u64 + 1) as usize;
    (0..count).map(|_| g.pick(words::LOREM).to_string()).collect()
}

fn lorem_sentence(g: Gen) -> String {
    let mut words = lorem_words(g, 5, 12);
    if let Some(first) = words.first_mut()
        && let Some(c) = first.chars().next().and_then(|c| c.to_uppercase().next())
    {
        let rest: String = first.chars().skip(1).collect();
        *first = format!("{c}{rest}");
    }
    let mut sentence = words.join(" ");
    sentence.push('.');
    sentence
}

fn lorem_paragraph(g: Gen) -> String {
    (0..g.range(3, 8)).map(|_| lorem_sentence(g)).collect::<Vec<String>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_known_name_generates_runnable_values() {
        for name in KNOWN_NAMES {
            let value = try_generate(name).unwrap_or_else(|| panic!("{name} must generate"));
            assert!(!value.is_empty(), "{name} produced an empty value");
        }
    }

    #[test]
    fn unknown_dollar_names_report_unresolved() {
        assert!(try_generate("$notARealDynamic").is_none());
    }

    #[test]
    fn shapes_match_the_appendix() {
        assert!(try_generate("$guid").is_some_and(|v| v.len() == 36 && v.contains('-')), "{:?}", try_generate("$guid"));
        assert!(
            try_generate("$timestamp").is_some_and(|v| v.parse::<i64>().is_ok_and(|sec| sec > 1_700_000_000)),
            "{:?}",
            try_generate("$timestamp")
        );
        let iso = try_generate("$isoTimestamp").unwrap();
        assert_eq!(iso.len(), "2026-10-19T08:10:36.177Z".len(), "{iso}");
        assert!(iso.ends_with('Z') && iso.starts_with("20"));
        assert!(try_generate("$randomHexColor").unwrap().starts_with('#'));
        // Ranges: int 0..=1000, price up to 1000.00 with two decimals.
        let int: i64 = try_generate("$randomInt").unwrap().parse().unwrap();
        assert!((0..=1000).contains(&int));
        let price: f64 = try_generate("$randomPrice").unwrap().parse().unwrap();
        assert!((0.0..=1000.0).contains(&price));
        let ip = try_generate("$randomIP").unwrap();
        assert_eq!(ip.split('.').count(), 4);
        let lat: f64 = try_generate("$randomLatitude").unwrap().parse().unwrap();
        assert!((-90.0..=90.0).contains(&lat));
        let long: f64 = try_generate("$randomLongitude").unwrap().parse().unwrap();
        assert!((-180.0..=180.0).contains(&long));
        let cur = try_generate("$randomCurrencyCode").unwrap();
        assert!((2..=4).contains(&cur.len()), "ISO 4217 codes are 2 or 3 letters: {cur}");
    }

    #[test]
    fn two_consecutive_timestamps_are_monotonic() {
        let a = try_generate("$timestamp").unwrap().parse::<i64>().unwrap();
        let b = try_generate("$timestamp").unwrap().parse::<i64>().unwrap();
        assert!(b >= a);
    }
}
