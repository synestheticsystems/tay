//! The order units could start in, and the circles that leave none.

use std::collections::{BTreeSet, HashMap, HashSet};

use crate::diagnostic::source::Loc;
use crate::diagnostic::{Diagnostic, Diagnostics};

/// `from` starts only after `to`.
pub struct Edge {
    pub from: usize,
    pub to: usize,
    pub kind: EdgeKind,
    pub loc: Loc,
}

pub enum EdgeKind {
    Needs,
    /// `from` uses this value, which `to` produces by running.
    Uses(String),
    After,
}

/// The units in an order they could start in: each after everything it
/// needs or waits for, ties broken by name so every run gives the same order.
/// Every circle is reported, whole, when there is none.
pub fn order(
    units: &[usize],
    name: &dyn Fn(usize) -> String,
    edges: &[Edge],
    d: &mut Diagnostics,
) -> Option<Vec<usize>> {
    let in_plan: HashSet<usize> = units.iter().copied().collect();
    let edges: Vec<&Edge> = edges
        .iter()
        .filter(|e| in_plan.contains(&e.from) && in_plan.contains(&e.to))
        .collect();
    let mut waiting: HashMap<usize, usize> = units.iter().map(|u| (*u, 0)).collect();
    for edge in &edges {
        *waiting
            .get_mut(&edge.from)
            .expect("edges are between units in the plan") += 1;
    }
    let mut ready: BTreeSet<(String, usize)> = waiting
        .iter()
        .filter(|(_, count)| **count == 0)
        .map(|(unit, _)| (name(*unit), *unit))
        .collect();
    let mut ordered = Vec::new();
    while let Some(first) = ready.pop_first() {
        let unit = first.1;
        ordered.push(unit);
        for edge in edges.iter().filter(|e| e.to == unit) {
            let count = waiting
                .get_mut(&edge.from)
                .expect("edges are between units in the plan");
            *count -= 1;
            if *count == 0 {
                ready.insert((name(edge.from), edge.from));
            }
        }
    }
    if ordered.len() == units.len() {
        return Some(ordered);
    }
    let started: HashSet<usize> = ordered.into_iter().collect();
    let left: Vec<&Edge> = edges
        .into_iter()
        .filter(|e| !started.contains(&e.from) && !started.contains(&e.to))
        .collect();
    report_circles(&left, name, d);
    None
}

/// Reports one circle through each group of units that wait on each other.
fn report_circles(edges: &[&Edge], name: &dyn Fn(usize) -> String, d: &mut Diagnostics) {
    let mut reported: HashSet<usize> = HashSet::new();
    let mut nodes: Vec<(String, usize)> = edges.iter().map(|e| (name(e.from), e.from)).collect();
    nodes.sort();
    nodes.dedup();
    for (_, start) in nodes {
        if reported.contains(&start) {
            continue;
        }
        let Some(circle) = circle_through(start, edges) else {
            continue;
        };
        reported.extend(circle.iter().map(|e| e.from));
        let steps: Vec<String> = circle
            .iter()
            .map(|edge| {
                let (from, to) = (name(edge.from), name(edge.to));
                match &edge.kind {
                    EdgeKind::Needs => format!("`{from}` needs `{to}`"),
                    EdgeKind::Uses(value) => {
                        format!("`{from}` uses `{value}`, which `{to}` produces by running")
                    }
                    EdgeKind::After => format!("`{from}` waits for `{to}` (`after`)"),
                }
            })
            .collect();
        let path = match steps.as_slice() {
            [only] => only.clone(),
            [rest @ .., last] => format!("{}, and {last}", rest.join(", ")),
            [] => unreachable!("a circle has at least one step"),
        };
        let mut error = Diagnostic::new(circle[0].loc, format!("a circle among units: {path}"));
        if circle.len() > 1 {
            error = error.with(circle[circle.len() - 1].loc, "the circle closes here");
        }
        d.push(error);
    }
}

/// A path of edges from `start` back to `start`, if there is one.
fn circle_through<'e>(start: usize, edges: &[&'e Edge]) -> Option<Vec<&'e Edge>> {
    let mut path: Vec<&Edge> = Vec::new();
    let mut seen = HashSet::new();
    fn walk<'e>(
        at: usize,
        start: usize,
        edges: &[&'e Edge],
        path: &mut Vec<&'e Edge>,
        seen: &mut HashSet<usize>,
    ) -> bool {
        for edge in edges.iter().filter(|e| e.from == at) {
            path.push(edge);
            if edge.to == start {
                return true;
            }
            if seen.insert(edge.to) && walk(edge.to, start, edges, path, seen) {
                return true;
            }
            path.pop();
        }
        false
    }
    walk(start, start, edges, &mut path, &mut seen).then_some(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostic::source::Sources;

    fn edges(pairs: &[(usize, usize)]) -> Vec<Edge> {
        let mut sources = Sources::default();
        let source = sources.add_argument("test");
        pairs
            .iter()
            .enumerate()
            .map(|(line, (from, to))| Edge {
                from: *from,
                to: *to,
                kind: EdgeKind::Needs,
                loc: Loc::new(source, line as u32 + 1, 1),
            })
            .collect()
    }

    fn name(unit: usize) -> String {
        ["a", "b", "c", "d"][unit].to_string()
    }

    #[test]
    fn each_unit_comes_after_what_it_needs_and_ties_go_by_name() {
        let mut d = Diagnostics::default();
        let order = order(
            &[0, 1, 2, 3],
            &name,
            &edges(&[(0, 2), (1, 2), (2, 3)]),
            &mut d,
        );
        assert_eq!(order, Some(vec![3, 2, 0, 1]));
    }

    #[test]
    fn a_circle_is_reported_whole_with_where_it_closes() {
        let mut d = Diagnostics::default();
        let order = order(
            &[0, 1, 2, 3],
            &name,
            &edges(&[(0, 1), (1, 2), (2, 0), (3, 0)]),
            &mut d,
        );
        assert_eq!(order, None);
        let errors = d.into_sorted();
        assert_eq!(errors.len(), 1);
        assert_eq!(
            errors[0].message,
            "a circle among units: `a` needs `b`, `b` needs `c`, and `c` needs `a`"
        );
        assert_eq!((errors[0].loc.line, errors[0].related[0].loc.line), (1, 3));
    }

    #[test]
    fn a_unit_that_needs_itself_is_a_circle() {
        let mut d = Diagnostics::default();
        assert_eq!(order(&[0], &name, &edges(&[(0, 0)]), &mut d), None);
        assert_eq!(
            d.into_sorted()[0].message,
            "a circle among units: `a` needs `a`"
        );
    }
}
