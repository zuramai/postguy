use postguy_util::json::{Caps, parse_json};
use serde_json::Value;

use crate::error::RunError;

/// Largest data file the runner reads.
pub const DATA_FILE_CAP: usize = 10 << 20;
pub const MAX_ROWS: usize = 100_000;
pub const MAX_COLUMNS: usize = 1_000;

/// The rows of a data file: each row is column name and value, in column order.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DataSet {
    pub rows: Vec<Vec<(String, Value)>>,
}

/// Parse a CSV or JSON data file. The name picks the format: `.json` (or content that starts
/// with `[`) is JSON, anything else is CSV with a header row.
pub fn parse_data(bytes: &[u8], file_name: &str) -> Result<DataSet, RunError> {
    if bytes.len() > DATA_FILE_CAP {
        return Err(RunError::Data(format!("the data file is {} bytes, over the {DATA_FILE_CAP}-byte cap", bytes.len())));
    }
    let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    let first = bytes.iter().copied().find(|byte| !byte.is_ascii_whitespace());
    if file_name.to_ascii_lowercase().ends_with(".json") || first == Some(b'[') { parse_json_rows(bytes) } else { parse_csv_rows(bytes) }
}

fn parse_json_rows(bytes: &[u8]) -> Result<DataSet, RunError> {
    let value = parse_json(bytes, &Caps::default()).map_err(|err| RunError::Data(format!("the JSON data file is not valid JSON: {err}")))?;
    let Value::Array(items) = value else {
        return Err(RunError::Data("a JSON data file must be an array of objects".to_string()));
    };
    if items.len() > MAX_ROWS {
        return Err(RunError::Data(format!("data file has more than {MAX_ROWS} rows")));
    }
    let mut rows = Vec::with_capacity(items.len());
    for (index, item) in items.into_iter().enumerate() {
        let Value::Object(map) = item else {
            return Err(RunError::Data(format!("row {}: a JSON data file must be an array of objects", index + 1)));
        };
        if map.len() > MAX_COLUMNS {
            return Err(RunError::Data(format!("row {}: more than {MAX_COLUMNS} columns", index + 1)));
        }
        rows.push(map.into_iter().collect());
    }
    Ok(DataSet { rows })
}

fn parse_csv_rows(bytes: &[u8]) -> Result<DataSet, RunError> {
    let mut reader = csv::ReaderBuilder::new().has_headers(true).flexible(false).from_reader(bytes);
    let headers: Vec<String> = reader.headers().map_err(|err| RunError::Data(format!("row 0: {err}")))?.iter().map(str::to_string).collect();
    if headers.len() > MAX_COLUMNS {
        return Err(RunError::Data(format!("data file has more than {MAX_COLUMNS} columns")));
    }
    let mut rows = Vec::new();
    for record in reader.records() {
        let record = record.map_err(|err| {
            let row = err.position().map(|position| position.record()).unwrap_or(0);
            RunError::Data(format!("row {row}: {err}"))
        })?;
        if rows.len() >= MAX_ROWS {
            return Err(RunError::Data(format!("data file has more than {MAX_ROWS} rows")));
        }
        rows.push(headers.iter().cloned().zip(record.iter()).map(|(name, value)| (name, Value::String(value.to_string()))).collect());
    }
    Ok(DataSet { rows })
}
