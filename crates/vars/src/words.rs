//! Word lists written for this project (contributor-original; recorded in ATTRIBUTION.toml
//! as such). Volume kept small on purpose: these exist for the dynamic variables, not for
//! realism at scale.
//!
//! Compile-checked against the shapes the Appendix A variables need: every list is
//! non-empty, and `COUNTRIES`/`CURRENCIES` tuples pair (name/code)/(code/name/symbol).

pub const FIRST_NAMES: &[&str] = &[
    "Ada", "Aiko", "Baptiste", "Clara", "Dmitri", "Elena", "Farida", "Giao", "Hana", "Ivo", "Jae", "Kavya", "Luis", "Maya", "Nour", "Piet", "Rin",
    "Sara", "Tomas", "Ute", "Viktor", "Wen", "Yara", "Zoë",
];
pub const LAST_NAMES: &[&str] = &[
    "Almeida", "Berg", "Chen", "Duval", "Eriksen", "Ferreira", "Grau", "Haddad", "Ibrahim", "Jonas", "Kim", "Laurent", "Mehta", "Novak", "Okonkwo",
    "Petrov",
];
pub const NAME_PREFIXES: &[&str] = &["Mr.", "Ms.", "Mrs.", "Dr."];
pub const NAME_SUFFIXES: &[&str] = &["PhD", "Jr", "III", "MD"];

pub const CITIES: &[&str] =
    &["Aarhus", "Auckland", "Bogotá", "Calgary", "Dunedin", "Ghent", "Hiroshima", "Kyoto", "Lyon", "Malmo", "Porto", "Tbilisi"];
pub const STREETS: &[&str] = &[
    "Alder Street",
    "Beech Lane",
    "Cedar Court",
    "Dock Row",
    "Elm Rise",
    "Ferry Lane",
    "Grove Way",
    "Harbour Street",
    "Ivy Walk",
    "Juniper Lane",
    "Kite Street",
    "Mare Road",
];

pub const COUNTRIES: &[(&str, &str)] = &[
    ("Portugal", "PT"),
    ("Japan", "JP"),
    ("Canada", "CA"),
    ("Norway", "NO"),
    ("Georgia", "GE"),
    ("Brazil", "BR"),
    ("Denmark", "DK"),
    ("Chile", "CL"),
    ("Estonia", "EE"),
    ("Morocco", "MA"),
    ("New Zealand", "NZ"),
    ("Austria", "AT"),
    ("Spain", "ES"),
    ("Indonesia", "ID"),
    ("Vietnam", "VN"),
    ("Ghana", "GH"),
    ("Finland", "FI"),
    ("Türkiye", "TR"),
    ("Croatia", "HR"),
    ("Uruguay", "UY"),
    ("Ireland", "IE"),
    ("South Korea", "KR"),
    ("Peru", "PE"),
    ("Czechia", "CZ"),
];

pub const CURRENCIES: &[(&str, &str, &str)] = &[
    ("EUR", "Euro", "€"),
    ("USD", "US Dollar", "$"),
    ("JPY", "Japanese Yen", "¥"),
    ("GBP", "Pound Sterling", "£"),
    ("CHF", "Swiss Franc", "CHF"),
    ("NOK", "Norwegian Krone", "kr"),
    ("CAD", "Canadian Dollar", "CA$"),
    ("BRL", "Brazilian Real", "R$"),
    ("AUD", "Australian Dollar", "A$"),
    ("SEK", "Swedish Krona", "SEK"),
    ("INR", "Indian Rupee", "₹"),
    ("ZAR", "South African Rand", "R"),
];

pub const COLORS: &[&str] = &[
    "aliceblue",
    "black",
    "cadetblue",
    "coral",
    "crimson",
    "darkseagreen",
    "dodgerblue",
    "firebrick",
    "gold",
    "indigo",
    "lavender",
    "moccasin",
    "olivedrab",
    "plum",
    "seagreen",
    "teal",
    "tomato",
    "wheat",
];

pub const WORDS: &[&str] = &[
    "anchor", "beacon", "cascade", "delta", "ember", "flint", "geyser", "harbor", "orbit", "prism", "quarry", "reef", "spark", "tundra", "vortex",
    "willow",
];

pub const LOREM: &[&str] = &[
    "a",
    "ad",
    "amet",
    "consectetur",
    "dolore",
    "do",
    "eiusmod",
    "elit",
    "et",
    "in",
    "labore",
    "lorem",
    "magna",
    "minim",
    "nisi",
    "nostrud",
    "tempor",
    "ullamco",
    "veniam",
    "commodo",
];

pub const JOB_TITLES: &[&str] = &[
    "Platform Engineer",
    "Data Analyst",
    "QA Engineer",
    "Site Reliability Engineer",
    "Product Designer",
    "Engineering Manager",
    "Technical Writer",
    "Solutions Architect",
    "Support Engineer",
    "Security Analyst",
];
pub const COMPANY_PREFIXES: &[&str] = &["Nova", "Aster", "Bitline", "Combe", "Delta Fox", "Ember", "Fjord", "Granite"];
pub const COMPANY_TAILS: &[&str] = &["Labs", "Systems", "Group", "Works", "Studio", "IO"];

pub const DOMAIN_SUFFIXES: &[&str] = &["com", "net", "org", "io", "dev"];
pub const USER_AGENTS: &[&str] = &[
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) Gecko/20100101 Firefox/141.0",
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 14.6) Gecko/20100101 Firefox/141.0",
    "Postguy (test data; list written for this project)",
    "curl/8.9.1",
];

pub const WEEKDAYS: &[&str] = &["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];
pub const MONTHS: &[&str] =
    &["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];

pub const FILE_EXTS: &[&str] = &["txt", "json", "png", "jpg", "pdf", "mp3", "zip", "csv", "md", "svg"];
pub const FILE_TYPES: &[&str] = &["plain-text", "json", "image", "binary", "archive", "audio", "video", "document", "spreadsheet", "vector"];
pub const MIME_TYPES: &[&str] = &[
    "application/json",
    "text/plain",
    "image/png",
    "image/jpeg",
    "application/pdf",
    "audio/mpeg",
    "video/mp4",
    "application/zip",
    "text/csv",
    "image/svg+xml",
];
