//! CSV writer: the requests table, UTF-8 with a BOM (Excel opens it as UTF-8), CRLF line
//! endings, RFC 4180 quoting and a guard against spreadsheet formula injection.

use super::table::{Cell, ReportTable};

const BOM: &[u8] = &[0xEF, 0xBB, 0xBF];

/// A text starting with a formula trigger gets a leading `'` so Excel shows it as text.
pub(crate) fn guard(text: &str) -> String {
    if text.starts_with(['=', '+', '-', '@', '\t', '\r']) { format!("'{text}") } else { text.to_string() }
}

fn field(cell: &Cell) -> String {
    match cell {
        Cell::Text(text) => guard(text),
        other => other.text(),
    }
}

/// The requests table as CSV bytes.
pub fn csv(table: &ReportTable) -> Vec<u8> {
    let mut writer = ::csv::WriterBuilder::new().terminator(::csv::Terminator::CRLF).flexible(true).from_writer(BOM.to_vec());
    let header: Vec<String> = table.request_headers.iter().map(|header| guard(header)).collect();
    let _ = writer.write_record(&header);
    for (_, row) in &table.requests {
        let _ = writer.write_record(row.iter().map(field));
    }
    writer.into_inner().unwrap_or_else(|err| err.into_inner().get_ref().clone())
}
