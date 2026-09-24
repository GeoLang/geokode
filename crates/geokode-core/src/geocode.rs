use crate::address::{
    GeoResult, MatchType, directional_mask, normalize_for_match, partial_suffix_keys,
};
use crate::details::{Details, display_name};
use crate::index::{
    ADDRESS_POINTS_FILE, CONTEXT_FILE, Chain, ContextFile, Coverage, DETAILS_FILE, FORMAT_VERSION,
    META_FILE, Meta, NAMES_FILE, POSTINGS_FILE, ROW_BYTES, ROWS_FILE, Row, SETTLEMENT_POINTS_FILE,
    key_importance, posting_offset,
};
use crate::rank::{DirectionalFit, QueryFit, score};
use crate::spatial::{KdTree, distance_km, to_degrees};
use fst::{Automaton, IntoStreamer, Streamer};
use memmap2::Mmap;
use std::collections::{BinaryHeap, HashMap};
use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};
use thiserror::Error;

const EXACT_CANDIDATES: usize = 2000;
const PREFIX_KEYS_SCANNED: usize = 50_000;
const PREFIX_KEYS_KEPT: usize = 64;
const CANDIDATES_PER_PREFIX_KEY: usize = 64;
const FUZZY_KEYS: usize = 64;
const CANDIDATES_PER_FUZZY_KEY: usize = 32;
const FUZZY_MIN_CHARS: usize = 4;
const FUZZY_ONE_EDIT_MAX_CHARS: usize = 5;
const FUZZY_MIN_SCORE: f64 = 0.6;
const IGNORED_QUALIFIER_CONFIDENCE: f64 = 0.6;
const TRAILING_QUALIFIER_MAX_WORDS: usize = 3;
const REVERSE_ADDRESS_MAX_KM: f64 = f64::INFINITY;
const REVERSE_SETTLEMENT_MAX_KM: f64 = 25.0;
const REVERSE_CONFIDENCE_PER_KM: f64 = 0.09;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    pub lon: f64,
    pub lat: f64,
}

#[derive(Debug, Error)]
pub enum OpenError {
    #[error("no geokode index at {0}: meta.json is missing, build one with `geokode build`")]
    Missing(PathBuf),
    #[error(
        "the index at {path} has format version {found}, this geokode reads version {expected}. Rebuild it with `geokode build`"
    )]
    Version {
        path: PathBuf,
        found: u32,
        expected: u32,
    },
    #[error("cannot read the index at {path}: {source}")]
    Io { path: PathBuf, source: io::Error },
    #[error("the index at {path} is damaged: {message}")]
    Damaged { path: PathBuf, message: String },
}

// index files are never written while served
fn map_file(path: &Path) -> io::Result<Mmap> {
    let file = File::open(path)?;
    unsafe { Mmap::map(&file) }
}

pub struct Geocoder {
    rows: Mmap,
    details: Mmap,
    names: fst::Map<Mmap>,
    postings: Mmap,
    areas_by_name: HashMap<String, Vec<u32>>,
    chains: Vec<Chain>,
    area_levels: Vec<u8>,
    labels: Vec<String>,
    addresses: KdTree<Mmap>,
    settlements: KdTree<Mmap>,
    address_coverage: Vec<Coverage>,
}

struct Qualifier {
    areas: Vec<u32>,
}

struct QualifierFit {
    // no area of the record could confirm or contradict a qualifier
    ignored: bool,
    matched_level: u32,
}

struct Query {
    key: String,
    qualifiers: Vec<Qualifier>,
    directionals: u8,
    numbered: bool,
    bias: Option<Point>,
}

#[derive(Clone, Copy)]
struct Candidate {
    id: u32,
    match_type: MatchType,
    fuzzy_score: f64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Fallbacks {
    // forward also tries typos and a trailing area without a comma
    Forward,
    Autocomplete,
}

impl Geocoder {
    pub fn open(directory: &Path) -> Result<Self, OpenError> {
        let io_error = |source| OpenError::Io {
            path: directory.to_path_buf(),
            source,
        };
        let damaged = |message: String| OpenError::Damaged {
            path: directory.to_path_buf(),
            message,
        };
        let meta_path = directory.join(META_FILE);
        if !meta_path.exists() {
            return Err(OpenError::Missing(directory.to_path_buf()));
        }
        let meta: Meta = serde_json::from_slice(&std::fs::read(&meta_path).map_err(io_error)?)
            .map_err(|e| damaged(e.to_string()))?;
        if meta.format_version != FORMAT_VERSION {
            return Err(OpenError::Version {
                path: directory.to_path_buf(),
                found: meta.format_version,
                expected: FORMAT_VERSION,
            });
        }
        let map = |name: &str| map_file(&directory.join(name)).map_err(io_error);
        let rows = map(ROWS_FILE)?;
        if rows.len() as u64 != meta.records * ROW_BYTES as u64 {
            return Err(damaged(format!(
                "{ROWS_FILE} holds {} bytes, meta.json promises {} records",
                rows.len(),
                meta.records
            )));
        }
        let names = fst::Map::new(map(NAMES_FILE)?).map_err(|e| damaged(e.to_string()))?;
        let context: ContextFile =
            serde_json::from_slice(&std::fs::read(directory.join(CONTEXT_FILE)).map_err(io_error)?)
                .map_err(|e| damaged(e.to_string()))?;
        let mut areas_by_name: HashMap<String, Vec<u32>> = HashMap::new();
        for (id, names) in context.area_names.into_iter().enumerate() {
            for name in names {
                areas_by_name.entry(name).or_default().push(id as u32);
            }
        }
        Ok(Geocoder {
            rows,
            details: map(DETAILS_FILE)?,
            names,
            postings: map(POSTINGS_FILE)?,
            areas_by_name,
            chains: context.chains,
            area_levels: context.area_levels,
            labels: context.labels,
            addresses: KdTree::new(map(ADDRESS_POINTS_FILE)?),
            settlements: KdTree::new(map(SETTLEMENT_POINTS_FILE)?),
            address_coverage: meta.address_coverage,
        })
    }

    pub fn len(&self) -> usize {
        self.rows.len() / ROW_BYTES
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn address_coverage(&self) -> &[Coverage] {
        &self.address_coverage
    }

    fn row(&self, id: u32) -> Row {
        let start = id as usize * ROW_BYTES;
        Row::decode(&self.rows[start..start + ROW_BYTES])
    }

    fn details(&self, row: &Row) -> Details {
        let start = row.detail_offset as usize;
        let bytes = &self.details[start..start + row.detail_len as usize];
        Details::decode(bytes, &self.labels).expect("details were written by IndexWriter")
    }

    fn posting(&self, value: u64, limit: usize) -> impl Iterator<Item = u32> + '_ {
        let start = posting_offset(value) as usize;
        let word = |at: usize| {
            let bytes = &self.postings[at..at + 4];
            u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
        };
        let count = word(start) as usize;
        (0..count.min(limit)).map(move |i| word(start + 4 + i * 4))
    }

    pub fn forward(&self, query: &str, limit: usize, bias: Option<Point>) -> Vec<GeoResult> {
        self.search(query, limit, bias, Fallbacks::Forward)
    }

    pub fn autocomplete(&self, query: &str, limit: usize, bias: Option<Point>) -> Vec<GeoResult> {
        self.search(query, limit, bias, Fallbacks::Autocomplete)
    }

    fn search(
        &self,
        text: &str,
        limit: usize,
        bias: Option<Point>,
        fallbacks: Fallbacks,
    ) -> Vec<GeoResult> {
        let mut parts = text.split(',').map(str::trim).filter(|p| !p.is_empty());
        let head = parts.next().unwrap_or_default();
        let qualifiers: Vec<Qualifier> = parts.map(|part| self.qualifier(part)).collect();
        let query = Query {
            key: normalize_for_match(head),
            qualifiers,
            directionals: directional_mask(head),
            numbered: head.starts_with(|c: char| c.is_ascii_digit()),
            bias,
        };
        if query.key.is_empty() {
            return Vec::new();
        }
        let mut candidates = self.name_candidates(&query.key, &partial_suffix_keys(head));
        let exact = candidates.iter().any(|c| c.match_type == MatchType::Exact);
        if fallbacks == Fallbacks::Forward && !exact {
            candidates.extend(self.fuzzy_candidates(&query.key));
        }
        let results = self.rank(&query, candidates, limit);
        if !results.is_empty() || fallbacks == Fallbacks::Autocomplete {
            return results;
        }
        if query.qualifiers.is_empty()
            && let Some(results) = self.search_with_trailing_qualifier(&query, limit)
        {
            return results;
        }
        results
    }

    fn qualifier(&self, text: &str) -> Qualifier {
        let key = normalize_for_match(text);
        Qualifier {
            areas: self.areas_by_name.get(&key).cloned().unwrap_or_default(),
        }
    }

    // retries "springfield illinois" as "springfield, illinois"
    fn search_with_trailing_qualifier(
        &self,
        query: &Query,
        limit: usize,
    ) -> Option<Vec<GeoResult>> {
        let words: Vec<&str> = query.key.split(' ').collect();
        for tail_words in 1..=TRAILING_QUALIFIER_MAX_WORDS.min(words.len().saturating_sub(1)) {
            let (head, tail) = words.split_at(words.len() - tail_words);
            let Some(areas) = self.areas_by_name.get(&tail.join(" ")) else {
                continue;
            };
            let retry = Query {
                key: head.join(" "),
                qualifiers: vec![Qualifier {
                    areas: areas.clone(),
                }],
                directionals: query.directionals,
                numbered: query.numbered,
                bias: query.bias,
            };
            let results = self.rank(&retry, self.name_candidates(&retry.key, &[]), limit);
            if !results.is_empty() {
                return Some(results);
            }
        }
        None
    }

    fn name_candidates(&self, key: &str, partial_keys: &[String]) -> Vec<Candidate> {
        let mut candidates = Vec::new();
        let candidate = |id, match_type| Candidate {
            id,
            match_type,
            fuzzy_score: 1.0,
        };
        if let Some(value) = self.names.get(key) {
            candidates.extend(
                self.posting(value, EXACT_CANDIDATES)
                    .map(|id| candidate(id, MatchType::Exact)),
            );
        }
        let mut best_keys = BinaryHeap::new();
        for prefix in std::iter::once(key).chain(partial_keys.iter().map(String::as_str)) {
            let mut stream = self
                .names
                .search(fst::automaton::Str::new(prefix).starts_with())
                .into_stream();
            let mut scanned = 0;
            while let Some((found, value)) = stream.next() {
                scanned += 1;
                if scanned > PREFIX_KEYS_SCANNED {
                    break;
                }
                if found == key.as_bytes() {
                    continue;
                }
                best_keys.push(std::cmp::Reverse((key_importance(value), value)));
                if best_keys.len() > PREFIX_KEYS_KEPT {
                    best_keys.pop();
                }
            }
        }
        for std::cmp::Reverse((_, value)) in best_keys {
            candidates.extend(
                self.posting(value, CANDIDATES_PER_PREFIX_KEY)
                    .map(|id| candidate(id, MatchType::Prefix)),
            );
        }
        candidates
    }

    fn fuzzy_candidates(&self, key: &str) -> Vec<Candidate> {
        let chars = key.chars().count();
        if chars < FUZZY_MIN_CHARS {
            return Vec::new();
        }
        let distance = if chars <= FUZZY_ONE_EDIT_MAX_CHARS {
            1
        } else {
            2
        };
        let Ok(automaton) = fst::automaton::Levenshtein::new(key, distance) else {
            return Vec::new();
        };
        let mut matches = Vec::new();
        let mut stream = self.names.search(automaton).into_stream();
        while let Some((found, value)) = stream.next() {
            let found = String::from_utf8_lossy(found);
            let longest = chars.max(found.chars().count()) as f64;
            let similarity = 1.0 - levenshtein(key, &found) as f64 / longest;
            if similarity >= FUZZY_MIN_SCORE {
                matches.push((similarity, value));
            }
        }
        matches.sort_by(|a, b| b.0.total_cmp(&a.0));
        matches.truncate(FUZZY_KEYS);
        matches
            .into_iter()
            .flat_map(|(similarity, value)| {
                self.posting(value, CANDIDATES_PER_FUZZY_KEY)
                    .map(move |id| Candidate {
                        id,
                        match_type: MatchType::Fuzzy,
                        fuzzy_score: similarity,
                    })
            })
            .collect()
    }

    // None drops the candidate
    fn qualifier_fit(&self, chain: &Chain, qualifiers: &[Qualifier]) -> Option<QualifierFit> {
        let mut fit = QualifierFit {
            ignored: false,
            matched_level: 0,
        };
        for qualifier in qualifiers {
            let matched = qualifier
                .areas
                .iter()
                .filter(|area| chain.areas.contains(area))
                .map(|area| u32::from(self.area_levels.get(*area as usize).copied().unwrap_or(0)))
                .max();
            match matched {
                Some(level) => fit.matched_level += level,
                None if chain.anchored => return None,
                None => fit.ignored = true,
            }
        }
        Some(fit)
    }

    fn rank(&self, query: &Query, candidates: Vec<Candidate>, limit: usize) -> Vec<GeoResult> {
        let mut best: HashMap<u32, Candidate> = HashMap::new();
        for candidate in candidates {
            best.entry(candidate.id)
                .and_modify(|seen| {
                    let better = (candidate.match_type, -candidate.fuzzy_score)
                        < (seen.match_type, -seen.fuzzy_score);
                    if better {
                        *seen = candidate;
                    }
                })
                .or_insert(candidate);
        }
        let mut scored: Vec<(f64, Candidate, Row, bool)> = best
            .into_values()
            .filter_map(|candidate| {
                let row = self.row(candidate.id);
                let chain = self.chains.get(row.chain as usize)?;
                let qualifier = self.qualifier_fit(chain, &query.qualifiers)?;
                let fit = QueryFit {
                    match_type: candidate.match_type,
                    numbered_query: query.numbered,
                    directional: directional_fit(query.directionals, row.directionals),
                    distance_km: query.bias.map(|bias| {
                        distance_km(bias.lon, bias.lat, to_degrees(row.lon), to_degrees(row.lat))
                    }),
                    ignored_qualifier: qualifier.ignored,
                    qualifier_level: qualifier.matched_level,
                };
                let value = score(row.importance, row.kind, &fit);
                Some((value, candidate, row, qualifier.ignored))
            })
            .collect();
        scored.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.id.cmp(&b.1.id)));
        scored.truncate(limit);
        scored
            .into_iter()
            .map(|(_, candidate, row, ignored_qualifier)| {
                let mut confidence = candidate.fuzzy_score;
                if ignored_qualifier {
                    confidence = confidence.min(IGNORED_QUALIFIER_CONFIDENCE);
                }
                self.result(&row, confidence, candidate.match_type)
            })
            .collect()
    }

    fn result(&self, row: &Row, confidence: f64, match_type: MatchType) -> GeoResult {
        let details = self.details(row);
        GeoResult {
            display_name: display_name(details.name.as_deref(), &details.address),
            name: details.name,
            address: details.address,
            country_code: details.country_code,
            lat: to_degrees(row.lat),
            lon: to_degrees(row.lon),
            bbox: row.bbox_degrees(),
            kind: row.kind,
            osm_type: details.osm.map(|(osm_type, _)| osm_type),
            osm_id: details.osm.map(|(_, id)| id),
            osm_key: details.osm_key,
            osm_value: details.osm_value,
            admin_level: details.admin_level,
            population: details.population,
            confidence,
            match_type,
        }
    }

    pub fn reverse(&self, lon: f64, lat: f64, limit: usize) -> Vec<GeoResult> {
        let covered = self.address_coverage.iter().any(|c| c.contains(lon, lat));
        let neighbours = if covered {
            self.addresses
                .nearest(lon, lat, limit, REVERSE_ADDRESS_MAX_KM)
        } else {
            self.settlements
                .nearest(lon, lat, limit, REVERSE_SETTLEMENT_MAX_KM)
        };
        neighbours
            .into_iter()
            .map(|neighbour| {
                let confidence =
                    (1.0 - neighbour.distance_km * REVERSE_CONFIDENCE_PER_KM).clamp(0.0, 1.0);
                self.result(&self.row(neighbour.id), confidence, MatchType::Exact)
            })
            .collect()
    }
}

fn directional_fit(query: u8, record: u8) -> Option<DirectionalFit> {
    if query == 0 {
        return None;
    }
    Some(if record == 0 {
        DirectionalFit::Absent
    } else if record & query != 0 {
        DirectionalFit::Same
    } else {
        DirectionalFit::Different
    })
}

fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut previous: Vec<usize> = (0..=b.len()).collect();
    let mut current = vec![0; b.len() + 1];
    for (i, a_char) in a.iter().enumerate() {
        current[0] = i + 1;
        for (j, b_char) in b.iter().enumerate() {
            let substitution = previous[j] + usize::from(a_char != b_char);
            current[j + 1] = substitution.min(previous[j + 1] + 1).min(current[j] + 1);
        }
        std::mem::swap(&mut previous, &mut current);
    }
    previous[b.len()]
}
