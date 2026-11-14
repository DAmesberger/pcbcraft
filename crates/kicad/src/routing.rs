//! Nets, tracks and vias, through the one command engine.
//!
//! **Nets.** Sparksmith's nets come from the schematic (wires and labels;
//! `sparksmith_core::nets`). A board file names each pad's net, so every
//! pad terminal with a net gets a local label of that net's name on its
//! pin: the labels join the terminals by name on the one sheet, and the
//! net takes the name. Names compare case-insensitively and must be 1–64
//! bytes without spaces, so a name that is not one is spelled anew (and
//! listed); the copper graph then carries the board's connectivity.
//!
//! **Copper.** Tracks and vias are added with `layout.route_apply` in
//! batches, checked by the engine like routed copper. A batch the checks
//! refuse is split until the refused items are found; those are listed
//! and the rest is imported. Arc tracks become chords within
//! [`ARC_CHORD_MM`].
use crate::board::{Board, Nm, PointNm};
use crate::import::{ImportError, Session};
use crate::items::KTrack;
use crate::report::{Case, Report};
use crate::translate::board_point;
use sparksmith_core::geometry::{Point, distance};
use sparksmith_core::layout::posture::MIN_SEGMENT_MM;
use sparksmith_core::layout::{LayoutCommand, RouteEditSet, RouteRequest, ViaRequest};
use sparksmith_core::model::LabelKind;
use sparksmith_core::model::import_review::{
    DisagreementKind, MAX_DISAGREEMENTS, MAX_TERMINALS, NetDisagreement,
};
use sparksmith_core::schematic::SchematicCommand;
use sparksmith_core::{
    CopperLayer, Endpoint, Id, Operation, PackageTerminalId, TerminalId, ViaSpan,
};
use std::collections::{BTreeMap, BTreeSet};

/// Largest distance of an arc track's chords from the arc, mm.
pub const ARC_CHORD_MM: f64 = 0.000_25;
/// Most chords one arc track becomes.
const MAX_ARC_CHORDS: usize = 512;
/// Tracks or vias one `layout.route_apply` adds.
const ROUTE_BATCH: usize = 128;

fn mm(nm: Nm) -> f64 {
    nm as f64 / 1e6
}

/// A label name for a KiCad net name: whitespace and control characters
/// become `_`, cut to 64 bytes on a character boundary.
pub(crate) fn label_name(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for c in name.chars() {
        let c = if c.is_whitespace() || c.is_control() {
            '_'
        } else {
            c
        };
        if out.len() + c.len_utf8() > 64 {
            break;
        }
        out.push(c);
    }
    if out.trim().is_empty() {
        "NET".to_owned()
    } else {
        out
    }
}

/// Unique label names for the nets that have pads, by KiCad net number;
/// names that had to change are counted with their detail.
fn names(board: &Board, numbers: &BTreeSet<u32>, report: &mut Report) -> BTreeMap<u32, String> {
    let mut taken = BTreeSet::new();
    let mut out = BTreeMap::new();
    for n in numbers {
        let original = board.items.nets.get(n).cloned().unwrap_or_default();
        let base = label_name(&original);
        let mut name = base.clone();
        let mut k = 2usize;
        while !taken.insert(name.to_ascii_lowercase()) {
            let suffix = format!("~{k}");
            let keep = base
                .char_indices()
                .map(|(i, c)| i + c.len_utf8())
                .take_while(|end| end + suffix.len() <= 64)
                .last()
                .unwrap_or(0);
            name = format!("{}{suffix}", &base[..keep]);
            k += 1;
        }
        if name != original {
            report.add_detail(Case::NetRenamed, "", "", &original);
        }
        out.insert(*n, name);
    }
    out
}

/// Join the placed footprints' terminals into the board's nets with a
/// label per terminal; the net of each KiCad net number that has a pad.
/// `nets[k]` lists the package terminals of placed footprint `k` with
/// their KiCad net numbers; `instances[k]` is its instance.
/// With a schematic (`labelled` the instances it did not draw), only
/// those get labels; the rest take their nets from the schematic's wiring,
/// and the schematic's nets are compared with the board's, net for net.
pub(crate) fn connect(
    session: &mut Session<'_>,
    board: &Board,
    nets: &[Vec<(PackageTerminalId, u32)>],
    instances: &[Id],
    labelled: Option<&BTreeSet<Id>>,
    report: &mut Report,
) -> Result<BTreeMap<u32, Id>, ImportError> {
    // One net per terminal: pads sharing a number share their terminal.
    let mut terminal_net: BTreeMap<Endpoint, u32> = BTreeMap::new();
    for (pads, instance) in nets.iter().zip(instances) {
        for (terminal, net) in pads {
            let end = Endpoint {
                instance: *instance,
                terminal: TerminalId(terminal.0),
            };
            match terminal_net.get(&end) {
                Some(other) if other != net => {
                    report.add(Case::TerminalOnTwoNets);
                }
                Some(_) => {}
                None => {
                    terminal_net.insert(end, *net);
                }
            }
        }
    }
    let numbers: BTreeSet<u32> = terminal_net.values().copied().collect();
    let names = names(board, &numbers, report);
    let mut labels = Vec::with_capacity(terminal_net.len());
    for (end, net) in &terminal_net {
        if labelled.is_some_and(|only| !only.contains(&end.instance)) {
            continue;
        }
        let Some(name) = names.get(net) else { continue };
        let Some(at) = sparksmith_core::schematic::pin_position(session.engine.project(), *end)
        else {
            // A board-only part has no pin on any sheet to label.
            report.add(Case::BoardOnlyNet);
            continue;
        };
        let position = sparksmith_core::units::PointNm::from_mm_f64_rounded(at).map_err(|e| {
            ImportError::Refused {
                step: "label.place",
                reason: e.to_string(),
            }
        })?;
        labels.push(Operation::Schematic(SchematicCommand::PlaceLabel {
            sheet: None,
            name: name.clone(),
            kind: LabelKind::Local,
            position,
            rotation: Default::default(),
            attach: Some((*end).into()),
        }));
    }
    session.batches("label.place", labels)?;
    let project = session.engine.project();
    let mut out = BTreeMap::new();
    for (end, net) in &terminal_net {
        if let Some(id) = project.net_of(*end).map(|n| n.id) {
            out.entry(*net).or_insert(id);
        }
    }
    report.counts.nets = out.len();
    if labelled.is_some() {
        let disagreements = compare_nets(project, &terminal_net, report);
        if !disagreements.is_empty() {
            session.apply(
                "schematic.set_net_disagreements",
                Operation::Schematic(SchematicCommand::SetNetDisagreements { disagreements }),
            )?;
        }
    }
    Ok(out)
}

/// Where a terminal is: on a net, or alone (on none).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Place<N> {
    Net(N),
    Alone(Endpoint),
}

/// Groups of `ends` by `place`, in order.
fn grouped<N: Ord>(ends: &[Endpoint], place: impl Fn(Endpoint) -> Place<N>) -> Vec<Vec<Endpoint>> {
    let mut groups: BTreeMap<Place<N>, Vec<Endpoint>> = BTreeMap::new();
    for end in ends {
        groups.entry(place(*end)).or_default().push(*end);
    }
    groups.into_values().collect()
}

/// Whether the schematic's nets equal the board's, net for net, and where
/// they disagree. A board net is equal when all its pad terminals are on
/// one schematic net (or, a lone pad, on none) that holds no terminal of
/// another board net; split when the schematic has its terminals apart
/// (the board joins them); joined when their schematic net holds another
/// board net's too. The schematic wins: each disagreement is returned for
/// the project to keep as a finding, a board net the schematic splits and
/// a schematic net the board splits (its terminals on several board nets,
/// or on none).
fn compare_nets(
    project: &sparksmith_core::Project,
    terminal_net: &BTreeMap<Endpoint, u32>,
    report: &mut Report,
) -> Vec<NetDisagreement> {
    let mut members: BTreeMap<u32, Vec<Endpoint>> = BTreeMap::new();
    for (end, net) in terminal_net {
        members.entry(*net).or_default().push(*end);
    }
    let ours = |e: Endpoint| {
        project
            .net_of(e)
            .map_or(Place::Alone(e), |n| Place::Net(n.id))
    };
    let board = |e: Endpoint| {
        terminal_net
            .get(&e)
            .map_or(Place::Alone(e), |n| Place::Net(*n))
    };
    let mut out = Vec::new();
    for ends in members.values() {
        let groups = grouped(ends, ours);
        let alone = ends.len() == 1;
        let place = groups.first().and_then(|g| g.first()).map(|e| ours(*e));
        if groups.len() > 1 {
            report.counts.schematic_nets_split += 1;
            out.push(NetDisagreement {
                kind: DisagreementKind::BoardJoins,
                groups,
            });
            continue;
        }
        let equal = match place {
            Some(Place::Net(id)) => project
                .nets
                .iter()
                .find(|n| n.id == id)
                .is_some_and(|n| grouped(&n.terminals, board).len() == 1),
            _ => alone,
        };
        if equal {
            report.counts.schematic_nets_equal += 1;
        } else {
            report.counts.schematic_nets_merged += 1;
        }
    }
    for net in project.nets.iter() {
        let groups = grouped(&net.terminals, board);
        if groups.len() > 1 {
            out.push(NetDisagreement {
                kind: DisagreementKind::BoardSplits,
                groups,
            });
        }
    }
    // Within the project's bounds: the first ones, the rest counted.
    let mut terminals = 0usize;
    let mut kept = Vec::with_capacity(out.len());
    for d in out {
        let n: usize = d.groups.iter().map(Vec::len).sum();
        let case = match d.kind {
            DisagreementKind::BoardJoins => Case::BoardJoinsNets,
            DisagreementKind::BoardSplits => Case::BoardSplitsNet,
        };
        if kept.len() < MAX_DISAGREEMENTS && terminals + n <= MAX_TERMINALS {
            terminals += n;
            kept.push(d);
            report.add(case);
        } else {
            report.add(Case::NetDisagreementOverLimit);
        }
    }
    kept
}

/// The copper layer a KiCad layer name is, by its place in the board's
/// layer list (top, inner layers, bottom).
fn copper_layer(board: &Board, name: &str) -> Option<CopperLayer> {
    let k = board.copper.iter().position(|l| l == name)?;
    let last = board.copper.len().checked_sub(1)?;
    Some(match k {
        0 => CopperLayer::Top,
        k if k == last => CopperLayer::Bottom,
        k => CopperLayer::Inner(u8::try_from(k).ok()?),
    })
}

/// A track's centre line in our frame: one segment, or an arc's chords.
fn centre_line(board: &Board, track: &KTrack) -> Vec<(Point, Point)> {
    let p = |q: PointNm| board_point(board, q);
    let (a, b) = (p(track.start), p(track.end));
    let Some(mid) = track.mid else {
        return vec![(a, b)];
    };
    let m = p(mid);
    let Some((c, r)) = crate::shape::circle_through(a, m, b) else {
        return vec![(a, b)];
    };
    let angle = |q: Point| (q[1] - c[1]).atan2(q[0] - c[0]);
    let (start, through, end) = (angle(a), angle(m), angle(b));
    let tau = std::f64::consts::TAU;
    // Sweep from start to end through mid, either way round.
    let ccw = (through - start).rem_euclid(tau) < (end - start).rem_euclid(tau);
    let sweep = if ccw {
        (end - start).rem_euclid(tau)
    } else {
        -(start - end).rem_euclid(tau)
    };
    let step = 2.0 * (1.0 - (ARC_CHORD_MM / r).min(1.0)).acos().max(1e-3);
    let n = ((sweep.abs() / step).ceil() as usize).clamp(1, MAX_ARC_CHORDS);
    let points: Vec<Point> = (0..=n)
        .map(|k| {
            if k == 0 {
                a
            } else if k == n {
                b
            } else {
                let t = start + sweep * k as f64 / n as f64;
                [c[0] + r * t.cos(), c[1] + r * t.sin()]
            }
        })
        .collect();
    points.windows(2).map(|w| (w[0], w[1])).collect()
}

/// Tracks and vias of the nets that have pads.
pub(crate) fn copper(
    session: &mut Session<'_>,
    board: &Board,
    nets: &BTreeMap<u32, Id>,
    report: &mut Report,
) -> Result<(), ImportError> {
    if session.engine.project().rules.require().is_err() {
        report.add_n(Case::TrackRefused, board.items.tracks.len());
        report.add_n(Case::ViaRefused, board.items.vias.len());
        return Ok(());
    }
    let mut tracks = Vec::with_capacity(board.items.tracks.len());
    for track in &board.items.tracks {
        let (Some(net), Some(layer)) = (nets.get(&track.net), copper_layer(board, &track.layer))
        else {
            report.add(Case::TrackWithoutNet);
            continue;
        };
        if track.mid.is_some() {
            report.add(Case::ArcTrack);
        }
        for (start, end) in centre_line(board, track) {
            tracks.push(RouteRequest {
                net: *net,
                start,
                end,
                width: mm(track.width),
                layer,
            });
        }
    }
    let tented = session.engine.project().rules.effective().mask.tent_vias;
    let via_points: Vec<(Id, Point)> = board
        .items
        .vias
        .iter()
        .filter_map(|v| Some((*nets.get(&v.net)?, board_point(board, v.at))))
        .collect();
    let cleaned = clean_tiny(&mut tracks, &via_points);
    report.add_n(Case::TinyTrackCleaned, cleaned);
    let mut vias = Vec::with_capacity(board.items.vias.len());
    for via in &board.items.vias {
        let Some(net) = nets.get(&via.net) else {
            report.add(Case::ViaWithoutNet);
            continue;
        };
        let (Some(a), Some(b)) = (
            copper_layer(board, &via.layers[0]),
            copper_layer(board, &via.layers[1]),
        ) else {
            report.add(Case::ViaRefused);
            continue;
        };
        let span = match (a, b) {
            (CopperLayer::Top, CopperLayer::Bottom) | (CopperLayer::Bottom, CopperLayer::Top) => {
                ViaSpan::Through
            }
            (from, to) => ViaSpan::Between { from, to },
        };
        if via.remove_unused_layers {
            report.add(Case::ViaUnusedLayers);
        }
        vias.push(ViaRequest {
            net: *net,
            position: board_point(board, via.at),
            drill: mm(via.drill),
            diameter: mm(via.size),
            span,
            tented: via.tenting.map_or(tented, |(front, back)| front && back),
        });
    }
    let added = add(session, tracks, Vec::new(), report)?;
    report.counts.tracks = added;
    let added = add(session, Vec::new(), vias, report)?;
    report.counts.vias = added;
    Ok(())
}

/// A track end's place on the nanometre grid, with its net and layer.
type EndKey = (Id, CopperLayer, i64, i64);

fn end_key(t: &RouteRequest, p: Point) -> EndKey {
    (
        t.net,
        t.layer,
        (p[0] * 1e6).round() as i64,
        (p[1] * 1e6).round() as i64,
    )
}

/// Remove track pieces shorter than the engine's least track length
/// ([`MIN_SEGMENT_MM`]) and join their neighbours: the ends a tiny piece
/// joins (on its net and layer, chained through other tiny pieces) become
/// one point, a via's centre when one is there, else their mean, so every
/// track that met the piece still meets the others and nothing dangles.
/// Moving an end by less than the least length can change no
/// connection: the copper graph joins by overlap, and the moved end stays
/// within its old end's reach. Repeats while the joining makes new tiny
/// pieces. Returns how many pieces were removed.
pub(crate) fn clean_tiny(tracks: &mut Vec<RouteRequest>, vias: &[(Id, Point)]) -> usize {
    let tiny = |t: &RouteRequest| distance(t.start, t.end) < MIN_SEGMENT_MM;
    let mut removed = 0;
    // Each pass removes at least one piece, so it ends.
    while tracks.iter().any(tiny) {
        // Union-find over the ends tiny pieces join.
        let mut parent: BTreeMap<EndKey, EndKey> = BTreeMap::new();
        fn root(parent: &mut BTreeMap<EndKey, EndKey>, k: EndKey) -> EndKey {
            let mut r = k;
            while let Some(p) = parent.get(&r).copied() {
                if p == r {
                    break;
                }
                r = p;
            }
            parent.insert(k, r);
            r
        }
        let mut points: BTreeMap<EndKey, Point> = BTreeMap::new();
        for t in tracks.iter().filter(|t| tiny(t)) {
            let (a, b) = (end_key(t, t.start), end_key(t, t.end));
            points.insert(a, t.start);
            points.insert(b, t.end);
            let (ra, rb) = (root(&mut parent, a), root(&mut parent, b));
            parent.entry(ra).or_insert(ra);
            if ra != rb {
                parent.insert(rb, ra);
            }
        }
        // Each group's point: a via of its net there, else the mean.
        let keys: Vec<EndKey> = points.keys().copied().collect();
        let mut groups: BTreeMap<EndKey, Vec<Point>> = BTreeMap::new();
        for k in &keys {
            let r = root(&mut parent, *k);
            if let Some(p) = points.get(k) {
                groups.entry(r).or_default().push(*p);
            }
        }
        let mut to: BTreeMap<EndKey, Point> = BTreeMap::new();
        for (r, members) in &groups {
            let via = vias.iter().find(|(net, v)| {
                *net == r.0 && members.iter().any(|p| distance(*p, *v) < MIN_SEGMENT_MM)
            });
            let n = members.len() as f64;
            let mean = [
                members.iter().map(|p| p[0]).sum::<f64>() / n,
                members.iter().map(|p| p[1]).sum::<f64>() / n,
            ];
            to.insert(*r, via.map_or(mean, |(_, v)| *v));
        }
        let before = tracks.len();
        tracks.retain(|t| !tiny(t));
        removed += before - tracks.len();
        for t in tracks.iter_mut() {
            for end in [false, true] {
                let p = if end { t.end } else { t.start };
                let k = end_key(t, p);
                if parent.contains_key(&k) {
                    let r = root(&mut parent, k);
                    if let Some(q) = to.get(&r) {
                        if end {
                            t.end = *q;
                        } else {
                            t.start = *q;
                        }
                    }
                }
            }
        }
    }
    removed
}

/// Add tracks and vias in batches; a refused batch is halved until each
/// refused item is found and counted. Returns how many were added.
fn add(
    session: &mut Session<'_>,
    tracks: Vec<RouteRequest>,
    vias: Vec<ViaRequest>,
    report: &mut Report,
) -> Result<usize, ImportError> {
    let mut added = 0;
    let mut pending: Vec<(Vec<RouteRequest>, Vec<ViaRequest>)> = Vec::new();
    for chunk in tracks.chunks(ROUTE_BATCH) {
        pending.push((chunk.to_vec(), Vec::new()));
    }
    for chunk in vias.chunks(ROUTE_BATCH) {
        pending.push((Vec::new(), chunk.to_vec()));
    }
    pending.reverse();
    while let Some((tracks, vias)) = pending.pop() {
        let n = tracks.len() + vias.len();
        if n == 0 {
            continue;
        }
        let edits = RouteEditSet {
            add: tracks.clone(),
            vias: vias.clone(),
            ..RouteEditSet::default()
        };
        match session.apply(
            "layout.route_apply",
            Operation::Layout(LayoutCommand::ApplyRoute { edits }),
        ) {
            Ok(_) => added += n,
            Err(ImportError::Refused { reason, .. }) if n == 1 => {
                let case = if tracks.is_empty() {
                    Case::ViaRefused
                } else {
                    Case::TrackRefused
                };
                report.add_detail(case, "", "", reason_kind(&reason));
            }
            Err(ImportError::Refused { .. }) => {
                // Halve and try each half; the first half first.
                let (t1, t2) = tracks.split_at(tracks.len() / 2);
                let (v1, v2) = vias.split_at(vias.len() / 2);
                pending.push((t2.to_vec(), v2.to_vec()));
                pending.push((t1.to_vec(), v1.to_vec()));
            }
            Err(other) => return Err(other),
        }
    }
    Ok(added)
}

/// The kind of a refusal, without the names the core's message may carry.
fn reason_kind(reason: &str) -> &'static str {
    if reason.contains("Too close") {
        "clearance"
    } else if reason.contains("inside the board") {
        "outside the board"
    } else if reason.contains("width") {
        "width"
    } else {
        "other"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(net: u64, start: Point, end: Point) -> RouteRequest {
        RouteRequest {
            net,
            start,
            end,
            width: 0.2,
            layer: CopperLayer::Top,
        }
    }

    #[test]
    fn a_tiny_piece_goes_and_its_neighbours_meet() {
        // A chain A–B, a 4 µm piece B–B', then B'–C; a second net's track
        // ends at B' too and must not be joined.
        let b = [5.0, 0.0];
        let b2 = [5.004, 0.0];
        let mut tracks = vec![
            track(1, [0.0, 0.0], b),
            track(1, b, b2),
            track(1, b2, [10.0, 0.0]),
            track(2, b2, [5.004, 5.0]),
        ];
        assert_eq!(clean_tiny(&mut tracks, &[]), 1);
        assert_eq!(tracks.len(), 3);
        let mean = [5.002, 0.0];
        assert!(distance(tracks[0].end, mean) < 1e-9);
        assert!(distance(tracks[1].start, mean) < 1e-9);
        // The other net's end stays.
        assert!(distance(tracks[2].start, b2) < 1e-12);
    }

    #[test]
    fn a_via_holds_the_joined_point_and_chains_collapse() {
        let via = [1.0, 1.0];
        let mut tracks = vec![
            track(1, [0.0, 1.0], via),
            track(1, via, [1.003, 1.0]),
            track(1, [1.003, 1.0], [1.006, 1.0]),
            track(1, [1.006, 1.0], [3.0, 1.0]),
            // Becomes zero length once its ends join: removed next pass.
            track(1, [1.006, 1.0], via),
        ];
        assert_eq!(clean_tiny(&mut tracks, &[(1, via)]), 3);
        assert_eq!(tracks.len(), 2);
        assert!(distance(tracks[0].end, via) < 1e-12);
        assert!(distance(tracks[1].start, via) < 1e-12);
    }
}
