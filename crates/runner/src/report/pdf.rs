//! Hand-written PDF 1.4 report in landscape A4. The text is set in the non-embedded Base-14
//! Courier fonts, whose glyphs are all 0.6 em wide, so wrapping needs no metrics table.

use std::fmt::Write as _;

use super::table::{
    COL_ASSERTIONS_FAILED, COL_ASSERTIONS_PASSED, COL_FAILURE_DETAILS, COL_FOLDER, COL_METHOD, COL_NUMBER, COL_REQUEST, COL_RESULT, COL_STATUS_CODE,
    COL_TESTS_FAILED, COL_TESTS_PASSED, COL_TIME_MS, FIRST_CUSTOM_COL, ReportTable, RowStatus,
};

const PAGE_W: f32 = 842.0;
const PAGE_H: f32 = 595.0;
const MARGIN: f32 = 28.0;
const FOOTER_Y: f32 = 16.0;
const BOTTOM: f32 = 40.0;
const FONT: f32 = 7.0;
const GLYPH: f32 = 0.6 * FONT;
const LINE: f32 = 8.5;
const MAX_LINES: usize = 4;
const MAX_ROWS: usize = 20_000;
const MAX_CUSTOM: usize = 4;
const CUSTOM_WIDTH: usize = 14;
const MIN_DETAILS: usize = 40;

const ELLIPSIS: u8 = 0x85;

/// Map one character to its WinAnsi byte; anything outside ASCII, Latin-1 and `…` becomes `?`.
fn winansi(c: char) -> u8 {
    match c {
        '\n' => b'\n',
        '\t' => b' ',
        '\u{20}'..='\u{7E}' => c as u8,
        '\u{A0}'..='\u{FF}' => c as u8,
        '…' => ELLIPSIS,
        _ => b'?',
    }
}

fn map_text(text: &str) -> Vec<u8> {
    text.chars().map(winansi).collect()
}

/// Wrap on whitespace (hard-breaking long words) into at most `max_lines` lines of `width`
/// bytes; leftover text becomes a trailing `…`.
fn wrap(text: &[u8], width: usize, max_lines: usize) -> Vec<Vec<u8>> {
    let width = width.max(2);
    let max_lines = max_lines.max(1);
    let mut end = text.len();
    while end > 0 && matches!(text.get(end - 1), Some(b'\n' | b' ')) {
        end -= 1;
    }
    let text = text.get(..end).unwrap_or(text);
    let mut lines: Vec<Vec<u8>> = Vec::new();
    let mut truncated = false;
    'paragraphs: for paragraph in text.split(|b| *b == b'\n') {
        if lines.len() >= max_lines {
            truncated = true;
            break;
        }
        let mut line: Vec<u8> = Vec::new();
        for word in paragraph.split(|b| *b == b' ').filter(|word| !word.is_empty()) {
            let mut word = word;
            loop {
                let gap = usize::from(!line.is_empty());
                if line.len() + gap + word.len() <= width {
                    if gap == 1 {
                        line.push(b' ');
                    }
                    line.extend_from_slice(word);
                    break;
                }
                if line.is_empty() {
                    let (head, tail) = word.split_at(width.min(word.len()));
                    line.extend_from_slice(head);
                    word = tail;
                    if word.is_empty() {
                        break;
                    }
                }
                if lines.len() + 1 >= max_lines {
                    lines.push(std::mem::take(&mut line));
                    truncated = true;
                    break 'paragraphs;
                }
                lines.push(std::mem::take(&mut line));
            }
        }
        lines.push(line);
    }
    if truncated && let Some(last) = lines.last_mut() {
        last.truncate(width - 1);
        last.push(ELLIPSIS);
    }
    if lines.is_empty() {
        lines.push(Vec::new());
    }
    lines
}

/// Append `bytes` as a PDF literal string; non-ASCII bytes use octal escapes.
fn literal(out: &mut String, bytes: &[u8]) {
    out.push('(');
    for byte in bytes {
        match byte {
            b'(' | b')' | b'\\' => {
                out.push('\\');
                out.push(char::from(*byte));
            }
            0x20..=0x7E => out.push(char::from(*byte)),
            other => {
                let _ = write!(out, "\\{other:03o}");
            }
        }
    }
    out.push(')');
}

fn text_at(out: &mut String, bold: bool, size: f32, x: f32, y: f32, bytes: &[u8]) {
    let font = if bold { "F2" } else { "F1" };
    let _ = write!(out, "BT /{font} {size:.1} Tf 1 0 0 1 {x:.2} {y:.2} Tm ");
    literal(out, bytes);
    out.push_str(" Tj ET\n");
}

fn fill(out: &mut String, rgb: &str, x: f32, y: f32, w: f32, h: f32) {
    let _ = writeln!(out, "{rgb} rg {x:.2} {y:.2} {w:.2} {h:.2} re f 0 g");
}

fn layout(table: &ReportTable) -> (Vec<(String, usize, Option<usize>)>, usize) {
    let total_chars = ((PAGE_W - 2.0 * MARGIN) / GLYPH) as usize;
    let fixed: [(&str, usize); 7] = [("#", 4), ("Request", 38), ("Method", 7), ("Status", 6), ("Result", 7), ("ms", 7), ("Tests", 9)];
    let used: usize = fixed.iter().map(|(_, w)| w + 1).sum();
    let mut details = total_chars.saturating_sub(used);
    let custom_total = table.request_headers.len().saturating_sub(FIRST_CUSTOM_COL);
    let room = details.saturating_sub(MIN_DETAILS) / (CUSTOM_WIDTH + 1);
    let shown = custom_total.min(MAX_CUSTOM).min(room);
    details = details.saturating_sub(shown * (CUSTOM_WIDTH + 1));
    let mut columns: Vec<(String, usize, Option<usize>)> = fixed.iter().map(|(t, w)| ((*t).to_string(), *w, None)).collect();
    columns.push(("Failure details".to_string(), details, None));
    for i in 0..shown {
        let index = FIRST_CUSTOM_COL + i;
        columns.push((table.request_headers.get(index).cloned().unwrap_or_default(), CUSTOM_WIDTH, Some(index)));
    }
    (columns, custom_total.saturating_sub(shown))
}

fn cell_text(row: &[super::table::Cell], index: usize) -> String {
    row.get(index).map(|cell| cell.text()).unwrap_or_default()
}

fn row_values(row: &[super::table::Cell], columns: &[(String, usize, Option<usize>)]) -> Vec<String> {
    let folder = cell_text(row, COL_FOLDER);
    let name = cell_text(row, COL_REQUEST);
    let request = if folder.is_empty() { name } else { format!("{folder} / {name}") };
    let ints = |index: usize| row.get(index).map(|cell| cell.int()).unwrap_or(0);
    let passed = ints(COL_ASSERTIONS_PASSED).saturating_add(ints(COL_TESTS_PASSED));
    let total = passed.saturating_add(ints(COL_ASSERTIONS_FAILED)).saturating_add(ints(COL_TESTS_FAILED));
    let mut values = vec![
        cell_text(row, COL_NUMBER),
        request,
        cell_text(row, COL_METHOD),
        cell_text(row, COL_STATUS_CODE),
        cell_text(row, COL_RESULT),
        cell_text(row, COL_TIME_MS),
        format!("{passed}/{total}"),
        cell_text(row, COL_FAILURE_DETAILS),
    ];
    for (_, _, custom) in columns.iter().skip(8) {
        values.push(custom.map(|index| cell_text(row, index)).unwrap_or_default());
    }
    values
}

struct PageBuilder {
    pages: Vec<String>,
    current: String,
    y: f32,
}

/// The report as PDF bytes.
pub fn pdf(table: &ReportTable) -> Vec<u8> {
    let (columns, hidden_custom) = layout(table);
    let mut builder = PageBuilder { pages: Vec::new(), current: String::new(), y: PAGE_H - MARGIN };

    // Title and summary (first page only).
    let title_chars = ((PAGE_W - 2.0 * MARGIN) / (0.6 * 14.0)) as usize;
    let mut title = map_text(&table.title);
    if title.len() > title_chars {
        title.truncate(title_chars.saturating_sub(1));
        title.push(ELLIPSIS);
    }
    builder.y -= 14.0;
    text_at(&mut builder.current, true, 14.0, MARGIN, builder.y, &title);
    builder.y -= 8.0;
    let per_column = table.summary.len().div_ceil(2);
    let column_w = (PAGE_W - 2.0 * MARGIN) / 2.0;
    let label_chars = 22_usize;
    let value_chars = ((column_w / (0.6 * 8.0)) as usize).saturating_sub(label_chars + 1);
    let top = builder.y;
    for (index, (label, value)) in table.summary.iter().enumerate() {
        let (col, row) = (index / per_column.max(1), index % per_column.max(1));
        let x = MARGIN + col as f32 * column_w;
        let y = top - 10.0 * (row + 1) as f32;
        let mut label = map_text(label);
        label.extend_from_slice(b":");
        text_at(&mut builder.current, true, 8.0, x, y, &label);
        let mut value = wrap(&map_text(value), value_chars, 1).into_iter().next().unwrap_or_default();
        value.truncate(value_chars);
        text_at(&mut builder.current, false, 8.0, x + (label_chars + 1) as f32 * 0.6 * 8.0, y, &value);
    }
    builder.y = top - 10.0 * per_column as f32 - 12.0;

    let header_row = |page: &mut PageBuilder| {
        let height = LINE + 3.0;
        fill(&mut page.current, "0.898 0.906 0.922", MARGIN, page.y - height, PAGE_W - 2.0 * MARGIN, height);
        let mut x = MARGIN;
        for (title, width, _) in &columns {
            text_at(&mut page.current, true, FONT, x, page.y - 7.0, &map_text(title));
            x += (*width + 1) as f32 * GLYPH;
        }
        page.y -= height;
    };
    header_row(&mut builder);

    let shown = table.requests.len().min(MAX_ROWS);
    for (status, cells) in table.requests.iter().take(shown) {
        let values = row_values(cells, &columns);
        let wrapped: Vec<Vec<Vec<u8>>> =
            values.iter().zip(&columns).map(|(value, (_, width, _))| wrap(&map_text(value), *width, MAX_LINES)).collect();
        let lines = wrapped.iter().map(Vec::len).max().unwrap_or(1).max(1);
        let height = lines as f32 * LINE + 3.0;
        if builder.y - height < BOTTOM {
            builder.pages.push(std::mem::take(&mut builder.current));
            builder.y = PAGE_H - MARGIN;
            header_row(&mut builder);
        }
        match status {
            RowStatus::Fail | RowStatus::Error => {
                fill(&mut builder.current, "0.996 0.886 0.886", MARGIN, builder.y - height, PAGE_W - 2.0 * MARGIN, height)
            }
            RowStatus::Skipped => fill(&mut builder.current, "0.996 0.953 0.78", MARGIN, builder.y - height, PAGE_W - 2.0 * MARGIN, height),
            RowStatus::Pass => {}
        }
        let mut x = MARGIN;
        for (cell_lines, (_, width, _)) in wrapped.iter().zip(&columns) {
            for (k, line) in cell_lines.iter().enumerate() {
                if !line.is_empty() {
                    text_at(&mut builder.current, false, FONT, x, builder.y - 7.0 - k as f32 * LINE, line);
                }
            }
            x += (*width + 1) as f32 * GLYPH;
        }
        builder.y -= height;
    }
    let mut notes = Vec::new();
    if table.requests.len() > shown {
        notes.push(format!("... {} more rows are in the CSV/XLSX report", table.requests.len() - shown));
    }
    if hidden_custom > 0 {
        notes.push(format!("{hidden_custom} more custom columns are in the CSV/XLSX report"));
    }
    for note in notes {
        if builder.y - 12.0 < BOTTOM {
            builder.pages.push(std::mem::take(&mut builder.current));
            builder.y = PAGE_H - MARGIN;
        }
        builder.y -= 12.0;
        text_at(&mut builder.current, false, FONT, MARGIN, builder.y, &map_text(&note));
    }
    builder.pages.push(std::mem::take(&mut builder.current));

    // Footers need the page count.
    let count = builder.pages.len();
    let finished = table.summary.iter().find(|(label, _)| label == "Finished at").map(|(_, value)| value.as_str()).unwrap_or("");
    for (index, page) in builder.pages.iter_mut().enumerate() {
        let footer = format!("Page {} of {count}   Run finished {finished}", index + 1);
        text_at(page, false, FONT, MARGIN, FOOTER_Y, &map_text(&footer));
    }

    assemble(&builder.pages)
}

fn assemble(pages: &[String]) -> Vec<u8> {
    let mut out: Vec<u8> = b"%PDF-1.4\n%\xE2\xE3\xCF\xD3\n".to_vec();
    let mut offsets: Vec<usize> = Vec::new();
    let mut object = |out: &mut Vec<u8>, id: usize, body: &str| {
        offsets.push(out.len());
        out.extend_from_slice(format!("{id} 0 obj\n{body}\nendobj\n").as_bytes());
    };
    let kids: String = (0..pages.len()).map(|i| format!("{} 0 R", 5 + 2 * i)).collect::<Vec<_>>().join(" ");
    object(&mut out, 1, "<< /Type /Catalog /Pages 2 0 R >>");
    object(&mut out, 2, &format!("<< /Type /Pages /Kids [{kids}] /Count {} >>", pages.len()));
    object(&mut out, 3, "<< /Type /Font /Subtype /Type1 /BaseFont /Courier /Encoding /WinAnsiEncoding >>");
    object(&mut out, 4, "<< /Type /Font /Subtype /Type1 /BaseFont /Courier-Bold /Encoding /WinAnsiEncoding >>");
    for (i, content) in pages.iter().enumerate() {
        let page_id = 5 + 2 * i;
        object(
            &mut out,
            page_id,
            &format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {PAGE_W} {PAGE_H}] /Resources << /Font << /F1 3 0 R /F2 4 0 R >> >> /Contents {} 0 R >>",
                page_id + 1
            ),
        );
        object(&mut out, page_id + 1, &format!("<< /Length {} >>\nstream\n{content}endstream", content.len()));
    }
    let xref = out.len();
    let size = offsets.len() + 1;
    let mut tail = format!("xref\n0 {size}\n0000000000 65535 f \n");
    for offset in &offsets {
        let _ = writeln!(tail, "{offset:010} 00000 n ");
    }
    let _ = write!(tail, "trailer\n<< /Size {size} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n");
    out.extend_from_slice(tail.as_bytes());
    out
}
