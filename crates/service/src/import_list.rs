//! Importing a watch list from pasted company codes.
//!
//! Accepts codes with the share class (`ABAN.N`, `ABAN.N0000`, `JKH.X0000`)
//! or bare (`ABAN`, `AEL`), separated by commas, spaces, semicolons or new
//! lines. Codes are checked against today's price list and, for stocks that
//! did not trade today, against the CSE's company lookup.

use std::collections::BTreeSet;

use serde::Deserialize;
use serde_json::{json, Value};

use crate::{parse, AppService, Result, ServiceError};

/// `ABAN.N` -> `ABAN.N0000`; complete symbols pass through.
pub fn normalise_ticker(raw: &str) -> Option<String> {
    let t = raw.trim().to_ascii_uppercase();
    let (base, class) = t.split_once('.')?;
    if !is_code(base) {
        return None;
    }
    let class = match class {
        "N" | "N0000" => "N0000",
        "X" | "X0000" => "X0000",
        other if other.len() == 5 && other.chars().all(|c| c.is_ascii_alphanumeric()) => other,
        _ => return None,
    };
    Some(format!("{base}.{class}"))
}

fn is_code(s: &str) -> bool {
    (2..=8).contains(&s.len())
        && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '&')
        && s.chars().any(|c| c.is_ascii_alphabetic())
}

/// Splits pasted text into candidate symbols (in order, no duplicates).
/// Bare codes become `CODE.N0000`, the ordinary voting share.
pub fn parse_codes(text: &str) -> Vec<String> {
    let mut seen = BTreeSet::new();
    text.split(|c: char| c.is_whitespace() || matches!(c, ',' | ';' | '|'))
        .map(|w| w.trim_matches(|c: char| !(c.is_ascii_alphanumeric() || c == '.' || c == '&')))
        .filter(|w| !w.is_empty())
        .filter_map(|w| {
            normalise_ticker(w).or_else(|| {
                let up = w.to_ascii_uppercase();
                is_code(&up).then(|| format!("{up}.N0000"))
            })
        })
        .filter(|s| seen.insert(s.clone()))
        .collect()
}

#[derive(Deserialize)]
struct ImportArgs {
    name: String,
    text: String,
    /// Replace the list's contents (true) or add to it (false).
    #[serde(default = "yes")]
    replace: bool,
}

fn yes() -> bool {
    true
}

impl AppService {
    /// The `import_watch_list` command.
    pub(crate) async fn import_watch_list(&self, args: Value) -> Result<Value> {
        let a: ImportArgs = parse(args)?;
        let name = a.name.trim().to_string();
        let codes = parse_codes(&a.text);
        if name.is_empty() || codes.is_empty() {
            return Err(ServiceError::BadArgs(
                "enter a list name and at least one company code".into(),
            ));
        }

        // Valid if in today's price list, otherwise ask the CSE directly
        // (stocks that did not trade today are missing from the list).
        let traded: BTreeSet<String> = self
            .state
            .read()
            .unwrap()
            .market
            .quotes
            .keys()
            .cloned()
            .collect();
        let mut valid = Vec::new();
        let mut invalid = Vec::new();
        for code in codes {
            if traded.contains(&code) {
                valid.push(code);
                continue;
            }
            match self.client.quote(&code).await {
                Ok(q) => {
                    self.state
                        .write()
                        .unwrap()
                        .market
                        .quotes
                        .entry(q.symbol.clone())
                        .or_insert(q);
                    valid.push(code);
                }
                Err(_) => invalid.push(code),
            }
        }
        if valid.is_empty() {
            return Err(ServiceError::BadArgs(format!(
                "none of the codes were found on the CSE: {}",
                invalid.join(", ")
            )));
        }

        let mut store = self.store.lock().unwrap();
        let existing = store.watch_lists()?.into_iter().find(|w| w.name == name);
        let (id, current): (i64, BTreeSet<String>) = match existing {
            Some(w) => (w.id, w.symbols.into_iter().collect()),
            None => (store.create_watch_list(&name)?, BTreeSet::new()),
        };
        let incoming: BTreeSet<String> = valid.iter().cloned().collect();
        let added: Vec<&String> = valid.iter().filter(|s| !current.contains(*s)).collect();
        let removed: Vec<&String> = if a.replace {
            current.difference(&incoming).collect()
        } else {
            vec![]
        };
        for s in &added {
            store.add_to_watch_list(id, s)?;
        }
        for s in &removed {
            store.remove_from_watch_list(id, s)?;
        }
        let count = store
            .watch_lists()?
            .into_iter()
            .find(|w| w.id == id)
            .map_or(0, |w| w.symbols.len());
        Ok(json!({
            "id": id,
            "name": name,
            "count": count,
            "added": added,
            "removed": removed,
            "invalid": invalid,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tickers_are_normalised() {
        assert_eq!(normalise_ticker("ABAN.N").as_deref(), Some("ABAN.N0000"));
        assert_eq!(normalise_ticker("jkh.n0000").as_deref(), Some("JKH.N0000"));
        assert_eq!(normalise_ticker("LOLC.X").as_deref(), Some("LOLC.X0000"));
        assert_eq!(normalise_ticker("www.example.com"), None);
        assert_eq!(normalise_ticker("31.91"), None);
    }

    #[test]
    fn parses_codes_in_any_separator_and_form() {
        let codes = parse_codes("ABAN.N, AEL\nacl; JKH.X0000 | ABAN  KZOO.N0000");
        assert_eq!(
            codes,
            [
                "ABAN.N0000",
                "AEL.N0000",
                "ACL.N0000",
                "JKH.X0000",
                "KZOO.N0000"
            ]
        );
    }

    #[test]
    fn ignores_numbers_and_stray_symbols() {
        assert_eq!(parse_codes("1, 2.5, -, ⭐, B&B"), ["B&B.N0000"]);
    }
}
