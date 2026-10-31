//! What the screen draws: per-engine time series, final results, and run-wide facts. Live runs and replays
//! both fill the same structures, so the renderer cannot tell them apart.
use crate::data::Recorded;
use crate::json::{self, esc};

#[derive(Clone, Debug, Default)]
pub struct Sample {
    pub t_ms: u64,
    pub bytes: u64,
    pub rss_kib: u64,
    pub temp: u64,
    pub rows: u64,
}

#[derive(Clone, Debug)]
pub struct End {
    pub t_ms: u64,
    pub ok: bool,
    pub peak_rss_kib: u64,
    pub peak_temp: u64,
    pub rows: u64,
    pub digest: String,
}

#[derive(Clone, Debug)]
pub enum Source {
    /// Measured by this process, now.
    Live,
    /// Replayed from a recorded time series (same fields as live).
    Replay,
    /// Only recorded summary numbers exist (no live run, no time series).
    Summary(Recorded),
    /// Engine unavailable and no recording for this size.
    Missing(String),
}

pub struct Eng {
    pub key: &'static str,
    pub label: &'static str,
    pub accent: &'static str,
    pub source: Source,
    pub samples: Vec<Sample>,
    pub end: Option<End>,
}

impl Eng {
    pub fn new(key: &'static str, label: &'static str, accent: &'static str, source: Source) -> Eng {
        Eng { key, label, accent, source, samples: vec![], end: None }
    }
    pub fn last(&self) -> Sample {
        self.samples.last().cloned().unwrap_or_default()
    }
    pub fn peak_rss_kib(&self) -> u64 {
        self.end.as_ref().map(|e| e.peak_rss_kib).unwrap_or_else(|| self.samples.iter().map(|s| s.rss_kib).max().unwrap_or(0))
    }
    pub fn peak_temp(&self) -> u64 {
        self.end.as_ref().map(|e| e.peak_temp).unwrap_or_else(|| self.samples.iter().map(|s| s.temp).max().unwrap_or(0))
    }
    pub fn done(&self) -> bool {
        match &self.source {
            Source::Live | Source::Replay => self.end.is_some(),
            Source::Summary(_) | Source::Missing(_) => true,
        }
    }
}

pub struct Meta {
    pub mode: &'static str, // "LIVE" | "REPLAY"
    pub note: String,       // extra line under the title (e.g. what REPLAY replays)
    pub size: String,
    pub total_bytes: u64,
    pub machine: String,
    pub duckdb: String,
    pub qe_settings: String,
    pub query: String,
}

pub fn sample_json(eng: &str, s: &Sample) -> String {
    format!(r#"{{"kind":"s","eng":"{eng}","t_ms":{},"bytes":{},"rss_kib":{},"temp":{},"rows":{}}}"#, s.t_ms, s.bytes, s.rss_kib, s.temp, s.rows)
}

pub fn end_json(eng: &str, e: &End) -> String {
    format!(
        r#"{{"kind":"end","eng":"{eng}","t_ms":{},"ok":{},"peak_rss_kib":{},"peak_temp":{},"rows":{},"digest":"{}"}}"#,
        e.t_ms, e.ok as u8, e.peak_rss_kib, e.peak_temp, e.rows, esc(&e.digest)
    )
}

pub fn meta_json(m: &Meta) -> String {
    format!(
        r#"{{"kind":"meta","size":"{}","total_bytes":{},"machine":"{}","duckdb":"{}","qe_settings":"{}","query":"{}"}}"#,
        esc(&m.size), m.total_bytes, esc(&m.machine), esc(&m.duckdb), esc(&m.qe_settings), esc(&m.query)
    )
}

/// Parsed recording: meta plus per-engine samples and ends, or None if it holds no time series.
pub struct Recording {
    pub meta: Meta,
    pub qe: (Vec<Sample>, Option<End>),
    pub duck: (Vec<Sample>, Option<End>),
}

pub fn load_recording(text: &str) -> Option<Recording> {
    let mut meta = None;
    let mut qe = (vec![], None);
    let mut duck = (vec![], None);
    for o in text.lines().filter_map(json::parse_obj) {
        let n = |k: &str| json::num(&o, k).unwrap_or(0.0) as u64;
        match json::text(&o, "kind").as_deref() {
            Some("meta") => {
                let t = |k: &str| json::text(&o, k).unwrap_or_default();
                meta = Some(Meta {
                    mode: "REPLAY",
                    note: String::new(),
                    size: t("size"),
                    total_bytes: n("total_bytes"),
                    machine: t("machine"),
                    duckdb: t("duckdb"),
                    qe_settings: t("qe_settings"),
                    query: t("query"),
                })
            }
            Some("s") | Some("end") => {
                let tgt = if json::text(&o, "eng").as_deref() == Some("qe") { &mut qe } else { &mut duck };
                if json::text(&o, "kind").as_deref() == Some("s") {
                    tgt.0.push(Sample { t_ms: n("t_ms"), bytes: n("bytes"), rss_kib: n("rss_kib"), temp: n("temp"), rows: n("rows") });
                } else {
                    tgt.1 = Some(End {
                        t_ms: n("t_ms"),
                        ok: n("ok") == 1,
                        peak_rss_kib: n("peak_rss_kib"),
                        peak_temp: n("peak_temp"),
                        rows: n("rows"),
                        digest: json::text(&o, "digest").unwrap_or_default(),
                    });
                }
            }
            _ => {}
        }
    }
    let meta = meta?;
    (!qe.0.is_empty()).then_some(Recording { meta, qe, duck })
}
