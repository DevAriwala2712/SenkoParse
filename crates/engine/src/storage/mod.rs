//! Catalog (schema.toml) and CSV storage.
pub mod csv;

pub use csv::stream::StreamInput;

use crate::types::Ty;
use crate::{err, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct TableDef {
    pub name: String,
    pub path: PathBuf,
    pub cols: Vec<(String, Ty)>,
}

pub struct Catalog {
    pub tables: HashMap<String, TableDef>,
}

impl Catalog {
    /// `schema.toml` format (no type sniffing):
    /// ```toml
    /// [table_name]        # file is <dir>/table_name.csv
    /// col = "i64"         # i64 | f64 | decimal(N) | str | date
    /// ```
    pub fn load(dir: &Path) -> Result<Catalog> {
        let p = dir.join("schema.toml");
        let text = std::fs::read_to_string(&p).map_err(|e| err!("cannot read {}: {e}", p.display()))?;
        let mut tables: HashMap<String, TableDef> = HashMap::new();
        let mut cur: Option<String> = None;
        for (ln, raw) in text.lines().enumerate() {
            let line = raw.split('#').next().unwrap().trim();
            if line.is_empty() {
                continue;
            }
            if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
                let name = name.trim().to_ascii_lowercase();
                if tables.contains_key(&name) {
                    return Err(err!("schema.toml:{}: duplicate table '{name}'", ln + 1));
                }
                tables.insert(
                    name.clone(),
                    TableDef { path: dir.join(format!("{name}.csv")), name: name.clone(), cols: vec![] },
                );
                cur = Some(name);
                continue;
            }
            let (k, v) = line.split_once('=').ok_or_else(|| err!("schema.toml:{}: expected `col = \"type\"`", ln + 1))?;
            let v = v.trim().trim_matches('"').trim_matches('\'');
            let ty = Ty::parse(v).ok_or_else(|| err!("schema.toml:{}: unknown type '{v}'", ln + 1))?;
            let t = tables
                .get_mut(cur.as_ref().ok_or_else(|| err!("schema.toml:{}: column outside a [table]", ln + 1))?)
                .unwrap();
            let k = k.trim().trim_matches('"').to_ascii_lowercase();
            if t.cols.iter().any(|(n, _)| *n == k) {
                return Err(err!("schema.toml:{}: duplicate column '{k}'", ln + 1));
            }
            t.cols.push((k, ty));
        }
        Ok(Catalog { tables })
    }

    pub fn table(&self, name: &str) -> Result<&TableDef> {
        self.tables.get(&name.to_ascii_lowercase()).ok_or_else(|| err!("unknown table '{name}'"))
    }
}
