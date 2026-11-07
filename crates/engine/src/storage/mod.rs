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
    /// Types came from sampling the file (ADR-0007), not from `schema.toml`.
    pub inferred: bool,
}

pub struct Catalog {
    pub tables: HashMap<String, TableDef>,
    /// CSV files with no `schema.toml` entry whose inference failed: table name -> error, reported on use.
    pub broken: HashMap<String, String>,
}

impl Catalog {
    /// Tables come from `schema.toml` (optional) and from inference over every other `<name>.csv` in `dir`
    /// (ADR-0007). `schema.toml` always wins. Format:
    /// ```toml
    /// [table_name]        # file is <dir>/table_name.csv
    /// col = "i64"         # i64 | f64 | decimal(N) | str | date
    /// ```
    pub fn load(dir: &Path) -> Result<Catalog> {
        let mut c = Self::load_schema(dir)?;
        let rd = std::fs::read_dir(dir).map_err(|e| err!("cannot read {}: {e}", dir.display()))?;
        let mut files: Vec<(String, PathBuf)> = Vec::new();
        for e in rd.flatten() {
            let path = e.path();
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else { continue };
            let Some(stem) = name.strip_suffix(".csv").or_else(|| name.strip_suffix(".CSV")) else { continue };
            let stem = stem.to_ascii_lowercase();
            let ident = stem.chars().next().is_some_and(|ch| ch.is_ascii_lowercase() || ch == '_')
                && stem.chars().all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_');
            if ident && !c.tables.contains_key(&stem) && path.is_file() {
                files.push((stem, path));
            }
        }
        files.sort();
        for (name, path) in files {
            // Two spellings of one name (`a.csv`, `A.CSV`): the first in sorted order wins, deterministically.
            if c.tables.contains_key(&name) || c.broken.contains_key(&name) {
                continue;
            }
            // A file that cannot be inferred fails only the queries that use it, not the whole directory.
            match csv::infer::infer_columns(&path) {
                Ok(cols) => {
                    c.tables.insert(name.clone(), TableDef { name, path, cols, inferred: true });
                }
                Err(e) => {
                    c.broken.insert(name, e.0);
                }
            }
        }
        Ok(c)
    }

    /// Only the tables declared in `schema.toml`; none if the file does not exist.
    fn load_schema(dir: &Path) -> Result<Catalog> {
        let p = dir.join("schema.toml");
        let text = match std::fs::read_to_string(&p) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(err!("cannot read {}: {e}", p.display())),
        };
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
                    TableDef {
                        path: dir.join(format!("{name}.csv")),
                        name: name.clone(),
                        cols: vec![],
                        inferred: false,
                    },
                );
                cur = Some(name);
                continue;
            }
            let (k, v) =
                line.split_once('=').ok_or_else(|| err!("schema.toml:{}: expected `col = \"type\"`", ln + 1))?;
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
        Ok(Catalog { tables, broken: HashMap::new() })
    }

    pub fn table(&self, name: &str) -> Result<&TableDef> {
        let key = name.to_ascii_lowercase();
        if let Some(e) = self.broken.get(&key) {
            return Err(err!("{e}"));
        }
        self.tables.get(&key).ok_or_else(|| err!("unknown table '{name}'"))
    }
}
