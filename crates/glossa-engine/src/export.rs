//! Writing the history out to a file.
//!
//! The classic program did this with small script plugins, one per format;
//! the same four formats are built in here.

use std::fmt::Write as _;

use glossa_core::history::HistoryEntry;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    Text,
    Csv,
    Html,
    Json,
}

impl ExportFormat {
    pub const ALL: [ExportFormat; 4] = [ExportFormat::Text, ExportFormat::Csv, ExportFormat::Html, ExportFormat::Json];

    pub fn extension(self) -> &'static str {
        match self {
            ExportFormat::Text => "txt",
            ExportFormat::Csv => "csv",
            ExportFormat::Html => "html",
            ExportFormat::Json => "json",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            ExportFormat::Text => "Text file",
            ExportFormat::Csv => "CSV file",
            ExportFormat::Html => "HTML file",
            ExportFormat::Json => "JSON file",
        }
    }

    pub fn from_extension(ext: &str) -> Option<ExportFormat> {
        ExportFormat::ALL.into_iter().find(|f| f.extension().eq_ignore_ascii_case(ext.trim_start_matches('.')))
    }
}

pub fn render(entries: &[&HistoryEntry], format: ExportFormat) -> String {
    match format {
        ExportFormat::Text => text(entries),
        ExportFormat::Csv => csv(entries),
        ExportFormat::Html => html(entries),
        ExportFormat::Json => serde_json::to_string_pretty(entries).unwrap_or_else(|_| "[]".into()),
    }
}

fn text(entries: &[&HistoryEntry]) -> String {
    let mut out = String::new();
    for e in entries {
        out.push_str(&e.source);
        out.push('\n');
        for r in &e.results {
            let _ = writeln!(
                out,
                "  [{} {} → {}] {}",
                r.provider_id,
                r.from.english_name(),
                r.to.english_name(),
                r.text.replace('\n', "\n  ")
            );
        }
        out.push('\n');
    }
    out
}

fn csv_field(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}

fn csv(entries: &[&HistoryEntry]) -> String {
    let mut out = String::from("source,from,translation,to,service,favorite\n");
    for e in entries {
        for r in &e.results {
            let _ = writeln!(
                out,
                "{},{},{},{},{},{}",
                csv_field(&e.source),
                csv_field(r.from.bcp47()),
                csv_field(&r.text),
                csv_field(r.to.bcp47()),
                csv_field(&r.provider_id),
                e.favorite
            );
        }
    }
    out
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\n', "<br>")
}

fn html(entries: &[&HistoryEntry]) -> String {
    let mut out = String::from(
        "<!DOCTYPE html>\n<html><head><meta charset=\"utf-8\"><title>Glossa history</title><style>\n\
         body{font:14px/1.5 system-ui,sans-serif;margin:2em;color:#1e1e1f}\n\
         table{border-collapse:collapse;margin-bottom:1.2em;width:100%}\n\
         td{border:1px solid #c8c8cc;padding:6px 10px;vertical-align:top}\n\
         td.k{background:#eeeef0;width:14em;color:#555}\n.fav{color:#d4a017}\n\
         </style></head><body>\n",
    );
    for e in entries {
        let star = if e.favorite { " <span class=\"fav\">★</span>" } else { "" };
        let _ = writeln!(out, "<table><tr><td class=\"k\">source{star}</td><td>{}</td></tr>", escape(&e.source));
        for r in &e.results {
            let _ = writeln!(
                out,
                "<tr><td class=\"k\">{} ({} → {})</td><td>{}</td></tr>",
                escape(&r.provider_id),
                escape(r.from.english_name()),
                escape(r.to.english_name()),
                escape(&r.text)
            );
        }
        out.push_str("</table>\n");
    }
    out.push_str("</body></html>\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use glossa_core::history::HistoryResult;
    use glossa_core::LangId;

    fn entry() -> HistoryEntry {
        HistoryEntry {
            id: 1,
            source: "say \"hi\" <now>".into(),
            results: vec![HistoryResult {
                provider_id: "google".into(),
                from: LangId::ENGLISH,
                to: LangId::RUSSIAN,
                text: "скажи \"привет\"".into(),
            }],
            favorite: true,
            created: 0,
        }
    }

    #[test]
    fn csv_quotes_and_escapes() {
        let e = entry();
        let out = render(&[&e], ExportFormat::Csv);
        assert!(out.starts_with("source,from,translation,to,service,favorite\n"));
        assert!(out.contains("\"say \"\"hi\"\" <now>\",\"en\",\"скажи \"\"привет\"\"\",\"ru\",\"google\",true"));
    }

    #[test]
    fn html_escapes_markup() {
        let e = entry();
        let out = render(&[&e], ExportFormat::Html);
        assert!(out.contains("say &quot;hi&quot; &lt;now&gt;"));
        assert!(out.contains("★"));
        assert!(!out.contains("<now>"));
    }

    #[test]
    fn json_round_trips() {
        let e = entry();
        let out = render(&[&e], ExportFormat::Json);
        let back: Vec<HistoryEntry> = serde_json::from_str(&out).unwrap();
        assert_eq!(back[0], e);
    }

    #[test]
    fn formats_by_extension() {
        assert_eq!(ExportFormat::from_extension(".CSV"), Some(ExportFormat::Csv));
        assert_eq!(ExportFormat::from_extension("htm"), None);
        for f in ExportFormat::ALL {
            assert_eq!(ExportFormat::from_extension(f.extension()), Some(f));
        }
    }
}
