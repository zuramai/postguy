//! Hand-written XLSX (SpreadsheetML in a zip): Summary, Requests and Tests sheets with a frozen
//! header row, an auto filter, column widths and coloured result cells. Strings are inline, so
//! there is no shared-strings part to keep in sync.

use std::fmt::Write as _;
use std::io::{Cursor, Write as _};

use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

use super::ReportError;
use super::table::{COL_EXCERPT, COL_FAILURE_DETAILS, Cell, ReportTable, RowStatus, TEST_COL_MESSAGE, TEST_COL_STATUS};

/// Excel's hard limit is 1,048,576 rows including the header.
const MAX_DATA_ROWS: usize = 1_048_575;
const MAX_CELL_CHARS: usize = 32_767;
const WIDTH_SAMPLE_ROWS: usize = 2_000;

const STYLE_HEADER: u8 = 1;
const STYLE_PASS: u8 = 2;
const STYLE_FAIL: u8 = 3;
const STYLE_SKIPPED: u8 = 4;
const STYLE_WRAP: u8 = 5;

const XML_DECL: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n";
const MAIN_NS: &str = "http://schemas.openxmlformats.org/spreadsheetml/2006/main";
const REL_NS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

fn xml_char_allowed(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\r' | '\u{20}'..='\u{D7FF}' | '\u{E000}'..='\u{FFFD}' | '\u{10000}'..='\u{10FFFF}')
}

/// Escape for XML text: strips characters XML 1.0 forbids and caps the length at Excel's limit.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len().min(MAX_CELL_CHARS));
    for c in text.chars().take(MAX_CELL_CHARS) {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            c if xml_char_allowed(c) => out.push(c),
            _ => {}
        }
    }
    out
}

/// `A`, `B`, ... `Z`, `AA`, ... for a zero-based column index.
fn column_name(mut index: usize) -> String {
    let mut letters = Vec::new();
    loop {
        letters.push(b'A' + u8::try_from(index % 26).unwrap_or(0));
        if index < 26 {
            break;
        }
        index = index / 26 - 1;
    }
    letters.reverse();
    String::from_utf8_lossy(&letters).into_owned()
}

fn status_style(status: RowStatus) -> u8 {
    match status {
        RowStatus::Pass => STYLE_PASS,
        RowStatus::Fail | RowStatus::Error => STYLE_FAIL,
        RowStatus::Skipped => STYLE_SKIPPED,
    }
}

fn write_cell(out: &mut String, column: usize, row: usize, cell: &Cell, style: u8) {
    let reference = format!("{}{row}", column_name(column));
    let style_attr = if style == 0 { String::new() } else { format!(" s=\"{style}\"") };
    match cell {
        Cell::Text(text) => {
            let _ = write!(out, "<c r=\"{reference}\"{style_attr} t=\"inlineStr\"><is><t xml:space=\"preserve\">{}</t></is></c>", escape(text));
        }
        Cell::Int(value) => {
            let _ = write!(out, "<c r=\"{reference}\"{style_attr}><v>{value}</v></c>");
        }
        Cell::Float(value) if value.is_finite() => {
            let _ = write!(out, "<c r=\"{reference}\"{style_attr}><v>{value}</v></c>");
        }
        Cell::Float(_) | Cell::Empty => {}
    }
}

struct Sheet<'a> {
    headers: &'a [String],
    rows: &'a [(RowStatus, Vec<Cell>)],
    status_col: usize,
    wrap_cols: &'a [usize],
}

fn widths(headers: &[String], rows: &[(RowStatus, Vec<Cell>)]) -> Vec<usize> {
    let mut widths: Vec<usize> = headers.iter().map(|header| header.chars().count()).collect();
    for (_, row) in rows.iter().take(WIDTH_SAMPLE_ROWS) {
        for (width, cell) in widths.iter_mut().zip(row) {
            *width = (*width).max(cell.text().chars().count());
        }
    }
    widths.iter().map(|width| width.saturating_add(2).clamp(8, 60)).collect()
}

fn cols_xml(out: &mut String, widths: &[usize]) {
    out.push_str("<cols>");
    for (index, width) in widths.iter().enumerate() {
        let n = index + 1;
        let _ = write!(out, "<col min=\"{n}\" max=\"{n}\" width=\"{width}\" customWidth=\"1\"/>");
    }
    out.push_str("</cols>");
}

fn table_sheet(sheet: &Sheet<'_>) -> String {
    let mut out = String::from(XML_DECL);
    let _ = write!(out, "<worksheet xmlns=\"{MAIN_NS}\" xmlns:r=\"{REL_NS}\">");
    out.push_str("<sheetViews><sheetView workbookViewId=\"0\"><pane ySplit=\"1\" topLeftCell=\"A2\" activePane=\"bottomLeft\" state=\"frozen\"/><selection pane=\"bottomLeft\"/></sheetView></sheetViews>");
    out.push_str("<sheetFormatPr defaultRowHeight=\"15\"/>");
    cols_xml(&mut out, &widths(sheet.headers, sheet.rows));
    out.push_str("<sheetData><row r=\"1\">");
    for (column, header) in sheet.headers.iter().enumerate() {
        write_cell(&mut out, column, 1, &Cell::Text(header.clone()), STYLE_HEADER);
    }
    out.push_str("</row>");
    let shown = sheet.rows.len().min(MAX_DATA_ROWS);
    // A truncated sheet gives its last row to the note.
    let (data, hidden) = if sheet.rows.len() > MAX_DATA_ROWS { (MAX_DATA_ROWS - 1, sheet.rows.len() - (MAX_DATA_ROWS - 1)) } else { (shown, 0) };
    let mut row_no = 1_usize;
    for (status, cells) in sheet.rows.iter().take(data) {
        row_no += 1;
        let _ = write!(out, "<row r=\"{row_no}\">");
        for (column, cell) in cells.iter().enumerate() {
            let style = if column == sheet.status_col {
                status_style(*status)
            } else if sheet.wrap_cols.contains(&column) {
                STYLE_WRAP
            } else {
                0
            };
            write_cell(&mut out, column, row_no, cell, style);
        }
        out.push_str("</row>");
    }
    if hidden > 0 {
        row_no += 1;
        let _ = write!(out, "<row r=\"{row_no}\">");
        write_cell(&mut out, 0, row_no, &Cell::Text(format!("… {hidden} more rows not shown")), 0);
        out.push_str("</row>");
    }
    out.push_str("</sheetData>");
    let last_column = column_name(sheet.headers.len().saturating_sub(1));
    let _ = write!(out, "<autoFilter ref=\"A1:{last_column}{row_no}\"/>");
    out.push_str("</worksheet>");
    out
}

fn summary_sheet(table: &ReportTable) -> String {
    let mut out = String::from(XML_DECL);
    let _ = write!(out, "<worksheet xmlns=\"{MAIN_NS}\" xmlns:r=\"{REL_NS}\">");
    out.push_str("<sheetFormatPr defaultRowHeight=\"15\"/>");
    let label_width = table.summary.iter().map(|(label, _)| label.chars().count()).max().unwrap_or(0).saturating_add(2).clamp(8, 60);
    let value_width = table.summary.iter().map(|(_, value)| value.chars().count()).max().unwrap_or(0).saturating_add(2).clamp(8, 60);
    cols_xml(&mut out, &[label_width, value_width]);
    out.push_str("<sheetData>");
    let _ = write!(out, "<row r=\"1\">");
    write_cell(&mut out, 0, 1, &Cell::Text(table.title.clone()), STYLE_HEADER);
    out.push_str("</row>");
    for (offset, (label, value)) in table.summary.iter().enumerate() {
        let row_no = offset + 2;
        let _ = write!(out, "<row r=\"{row_no}\">");
        write_cell(&mut out, 0, row_no, &Cell::Text(label.clone()), STYLE_HEADER);
        write_cell(&mut out, 1, row_no, &Cell::Text(value.clone()), 0);
        out.push_str("</row>");
    }
    out.push_str("</sheetData></worksheet>");
    out
}

fn last_row(len: usize) -> usize {
    len.min(MAX_DATA_ROWS).saturating_add(1)
}

fn workbook(table: &ReportTable) -> String {
    let mut out = String::from(XML_DECL);
    let _ = write!(out, "<workbook xmlns=\"{MAIN_NS}\" xmlns:r=\"{REL_NS}\"><sheets>");
    out.push_str("<sheet name=\"Summary\" sheetId=\"1\" r:id=\"rId1\"/><sheet name=\"Requests\" sheetId=\"2\" r:id=\"rId2\"/><sheet name=\"Tests\" sheetId=\"3\" r:id=\"rId3\"/></sheets><definedNames>");
    let request_end = format!("{}{}", column_name(table.request_headers.len().saturating_sub(1)), last_row(table.requests.len()));
    let test_end = format!("{}{}", column_name(table.test_headers.len().saturating_sub(1)), last_row(table.tests.len()));
    let _ = write!(
        out,
        "<definedName name=\"_xlnm._FilterDatabase\" localSheetId=\"1\" hidden=\"1\">Requests!$A$1:${}</definedName>",
        absolute(&request_end)
    );
    let _ =
        write!(out, "<definedName name=\"_xlnm._FilterDatabase\" localSheetId=\"2\" hidden=\"1\">Tests!$A$1:${}</definedName>", absolute(&test_end));
    out.push_str("</definedNames></workbook>");
    out
}

/// `AB12` becomes `AB$12`, so `$` + the result is an absolute reference.
fn absolute(reference: &str) -> String {
    let split = reference.find(|c: char| c.is_ascii_digit()).unwrap_or(reference.len());
    let (letters, digits) = reference.split_at(split);
    format!("{letters}${digits}")
}

const CONTENT_TYPES: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\"><Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/><Default Extension=\"xml\" ContentType=\"application/xml\"/><Override PartName=\"/xl/workbook.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml\"/><Override PartName=\"/xl/worksheets/sheet1.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml\"/><Override PartName=\"/xl/worksheets/sheet2.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml\"/><Override PartName=\"/xl/worksheets/sheet3.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml\"/><Override PartName=\"/xl/styles.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml\"/></Types>";

const ROOT_RELS: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" Target=\"xl/workbook.xml\"/></Relationships>";

const WORKBOOK_RELS: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet\" Target=\"worksheets/sheet1.xml\"/><Relationship Id=\"rId2\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet\" Target=\"worksheets/sheet2.xml\"/><Relationship Id=\"rId3\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet\" Target=\"worksheets/sheet3.xml\"/><Relationship Id=\"rId4\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles\" Target=\"styles.xml\"/></Relationships>";

const STYLES: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<styleSheet xmlns=\"http://schemas.openxmlformats.org/spreadsheetml/2006/main\"><fonts count=\"2\"><font><sz val=\"11\"/><name val=\"Calibri\"/></font><font><b/><sz val=\"11\"/><name val=\"Calibri\"/></font></fonts><fills count=\"6\"><fill><patternFill patternType=\"none\"/></fill><fill><patternFill patternType=\"gray125\"/></fill><fill><patternFill patternType=\"solid\"><fgColor rgb=\"FFE5E7EB\"/><bgColor indexed=\"64\"/></patternFill></fill><fill><patternFill patternType=\"solid\"><fgColor rgb=\"FFDCFCE7\"/><bgColor indexed=\"64\"/></patternFill></fill><fill><patternFill patternType=\"solid\"><fgColor rgb=\"FFFEE2E2\"/><bgColor indexed=\"64\"/></patternFill></fill><fill><patternFill patternType=\"solid\"><fgColor rgb=\"FFFEF3C7\"/><bgColor indexed=\"64\"/></patternFill></fill></fills><borders count=\"1\"><border><left/><right/><top/><bottom/><diagonal/></border></borders><cellStyleXfs count=\"1\"><xf numFmtId=\"0\" fontId=\"0\" fillId=\"0\" borderId=\"0\"/></cellStyleXfs><cellXfs count=\"6\"><xf numFmtId=\"0\" fontId=\"0\" fillId=\"0\" borderId=\"0\" xfId=\"0\"/><xf numFmtId=\"0\" fontId=\"1\" fillId=\"2\" borderId=\"0\" xfId=\"0\" applyFont=\"1\" applyFill=\"1\"/><xf numFmtId=\"0\" fontId=\"0\" fillId=\"3\" borderId=\"0\" xfId=\"0\" applyFill=\"1\"/><xf numFmtId=\"0\" fontId=\"0\" fillId=\"4\" borderId=\"0\" xfId=\"0\" applyFill=\"1\"/><xf numFmtId=\"0\" fontId=\"0\" fillId=\"5\" borderId=\"0\" xfId=\"0\" applyFill=\"1\"/><xf numFmtId=\"0\" fontId=\"0\" fillId=\"0\" borderId=\"0\" xfId=\"0\" applyAlignment=\"1\"><alignment wrapText=\"1\" vertical=\"top\"/></xf></cellXfs><cellStyles count=\"1\"><cellStyle name=\"Normal\" xfId=\"0\" builtinId=\"0\"/></cellStyles></styleSheet>";

/// The report as an XLSX workbook.
pub fn xlsx(table: &ReportTable) -> Result<Vec<u8>, ReportError> {
    let zip_error = |err: &dyn std::fmt::Display| ReportError::Zip(err.to_string());
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    let requests = table_sheet(&Sheet {
        headers: &table.request_headers,
        rows: &table.requests,
        status_col: super::table::COL_RESULT,
        wrap_cols: &[COL_FAILURE_DETAILS, COL_EXCERPT],
    });
    let tests = table_sheet(&Sheet { headers: &table.test_headers, rows: &table.tests, status_col: TEST_COL_STATUS, wrap_cols: &[TEST_COL_MESSAGE] });
    let parts: [(&str, String); 9] = [
        ("[Content_Types].xml", CONTENT_TYPES.to_string()),
        ("_rels/.rels", ROOT_RELS.to_string()),
        ("xl/workbook.xml", workbook(table)),
        ("xl/_rels/workbook.xml.rels", WORKBOOK_RELS.to_string()),
        ("xl/styles.xml", STYLES.to_string()),
        ("xl/worksheets/sheet1.xml", summary_sheet(table)),
        ("xl/worksheets/sheet2.xml", requests),
        ("xl/worksheets/sheet3.xml", tests),
        (
            "docProps/app.xml",
            format!(
                "{XML_DECL}<Properties xmlns=\"http://schemas.openxmlformats.org/officeDocument/2006/extended-properties\"><Application>Postguy</Application></Properties>"
            ),
        ),
    ];
    for (name, body) in &parts {
        zip.start_file(*name, options).map_err(|err| zip_error(&err))?;
        zip.write_all(body.as_bytes()).map_err(|err| zip_error(&err))?;
    }
    let cursor = zip.finish().map_err(|err| zip_error(&err))?;
    Ok(cursor.into_inner())
}
