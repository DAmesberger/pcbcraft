//! Drill hits of two files paired one to one: every hit of A with the
//! nearest unused hit of B of the same diameter (within the tolerance), a
//! slot with a slot whose ends match either way round.
use crate::excellon::{DrillFile, Hit};
use crate::{Fallible, Point};

/// Hits one comparison takes, per file (a target bound well above a
/// 10,000-via board).
pub const MAX_HITS: usize = 200_000;

/// One drilled feature in mm.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Feature {
    Round { at: Point },
    Slot { from: Point, to: Point },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DrillHit {
    pub feature: Feature,
    pub diameter: f64,
}

impl DrillHit {
    fn key(&self) -> f64 {
        match self.feature {
            Feature::Round { at } => at[0],
            Feature::Slot { from, to } => from[0].min(to[0]),
        }
    }
}

/// The hits of `file` whose tool `keep` accepts (it sees the tool's
/// aperture function, empty when the file has none).
pub fn hits(file: &DrillFile, keep: impl Fn(&[String]) -> bool) -> Fallible<Vec<DrillHit>> {
    if file.hits.len() > MAX_HITS {
        return Err(format!("drills: more than {MAX_HITS} hits").into());
    }
    let mut out = Vec::new();
    for hit in &file.hits {
        let tool = hit.tool();
        let function = file
            .tool_functions
            .get(&tool)
            .map(Vec::as_slice)
            .unwrap_or_default();
        if !keep(function) {
            continue;
        }
        let diameter = *file
            .tools
            .get(&tool)
            .ok_or_else(|| format!("drills: tool T{tool} is not defined"))?;
        let feature = match *hit {
            Hit::Round { at, .. } => Feature::Round { at },
            Hit::Slot { from, to, .. } => Feature::Slot { from, to },
        };
        out.push(DrillHit { feature, diameter });
    }
    Ok(out)
}

/// Two drill sets paired.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DrillDiff {
    pub hits_a: usize,
    pub hits_b: usize,
    pub matched: usize,
    /// Largest centre (or slot end) distance of a matched pair, µm.
    pub worst_position_um: f64,
    /// Largest diameter difference of a matched pair, µm.
    pub worst_diameter_um: f64,
    /// Hits of A, then of B, left without a partner.
    pub unmatched_a: Vec<DrillHit>,
    pub unmatched_b: Vec<DrillHit>,
}

impl DrillDiff {
    pub fn passes(&self) -> bool {
        self.hits_a == self.hits_b && self.unmatched_a.is_empty() && self.unmatched_b.is_empty()
    }
}

fn distance(a: Point, b: Point) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

/// How far apart two features are, mm (`None` for a hole and a slot).
fn gap(a: &Feature, b: &Feature) -> Option<f64> {
    match (a, b) {
        (Feature::Round { at: p }, Feature::Round { at: q }) => Some(distance(*p, *q)),
        (Feature::Slot { from: a0, to: a1 }, Feature::Slot { from: b0, to: b1 }) => Some(
            distance(*a0, *b0)
                .max(distance(*a1, *b1))
                .min(distance(*a0, *b1).max(distance(*a1, *b0))),
        ),
        _ => None,
    }
}

/// Pair `a` with `b`: a pair is within `tolerance_mm` in every end and in
/// diameter; each hit of A takes the nearest unused partner.
pub fn pair(a: &[DrillHit], b: &[DrillHit], tolerance_mm: f64) -> DrillDiff {
    let mut order: Vec<usize> = (0..b.len()).collect();
    order.sort_by(|i, j| b[*i].key().total_cmp(&b[*j].key()));
    let keys: Vec<f64> = order.iter().map(|i| b[*i].key()).collect();
    let mut used = vec![false; b.len()];
    let mut diff = DrillDiff {
        hits_a: a.len(),
        hits_b: b.len(),
        ..DrillDiff::default()
    };
    for hit in a {
        let key = hit.key();
        let first = keys.partition_point(|k| *k < key - tolerance_mm);
        let mut best: Option<(f64, usize)> = None;
        for (slot, index) in order.iter().enumerate().skip(first) {
            if keys[slot] > key + tolerance_mm {
                break;
            }
            let other = &b[*index];
            if used[*index] || (other.diameter - hit.diameter).abs() > tolerance_mm {
                continue;
            }
            if let Some(g) = gap(&hit.feature, &other.feature)
                && g <= tolerance_mm
                && best.is_none_or(|(d, _)| g < d)
            {
                best = Some((g, *index));
            }
        }
        match best {
            Some((g, index)) => {
                used[index] = true;
                diff.matched += 1;
                diff.worst_position_um = diff.worst_position_um.max(g * 1e3);
                diff.worst_diameter_um = diff
                    .worst_diameter_um
                    .max((b[index].diameter - hit.diameter).abs() * 1e3);
            }
            None => diff.unmatched_a.push(*hit),
        }
    }
    diff.unmatched_b = b
        .iter()
        .zip(&used)
        .filter(|(_, u)| !**u)
        .map(|(h, _)| *h)
        .collect();
    diff
}
