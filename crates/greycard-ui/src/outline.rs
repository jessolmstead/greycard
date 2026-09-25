//! A repair patch's shape for the overlay: the edge of a spot's disc
//! or of a stroke's swept path, as closed rings the viewport draws
//! with a Path.

use std::fmt::Write as _;

use greycard_edit::retouch::distance_to_polyline;

/// The edge of everything within `radius` of the polyline `points`,
/// as closed rings in the points' own units: the contour of the
/// distance to the polyline at the radius, marched over a grid of
/// `cell` (never finer than a hundred and twenty-eighth of the
/// shape's extent). A contour is exact for any self-overlap, where
/// an offset of the polyline would draw chords across the inside; a
/// stroke that loops gives an outer ring and a hole. Empty for no
/// points or no radius.
pub fn contours(points: &[(f32, f32)], radius: f32, cell: f32) -> Vec<Vec<(f32, f32)>> {
    if points.is_empty() || radius <= 0.0 {
        return Vec::new();
    }
    let (mut lo, mut hi) = ((f32::MAX, f32::MAX), (f32::MIN, f32::MIN));
    for p in points {
        lo = (lo.0.min(p.0), lo.1.min(p.1));
        hi = (hi.0.max(p.0), hi.1.max(p.1));
    }
    let extent = (hi.0 - lo.0 + 2.0 * radius).max(hi.1 - lo.1 + 2.0 * radius);
    let cell = cell.max(extent / 128.0).max(1e-6);
    // A margin of a cell beyond the radius, so every node on the
    // grid's rim is outside and the contours close within it.
    let origin = (lo.0 - radius - cell, lo.1 - radius - cell);
    let nx = ((hi.0 - lo.0 + 2.0 * (radius + cell)) / cell).ceil() as usize + 1;
    let ny = ((hi.1 - lo.1 + 2.0 * (radius + cell)) / cell).ceil() as usize + 1;
    let node = |i: usize, j: usize| (origin.0 + i as f32 * cell, origin.1 + j as f32 * cell);
    // The signed field at the nodes: negative inside.
    let field: Vec<f32> = (0..=ny)
        .flat_map(|j| (0..=nx).map(move |i| (i, j)))
        .map(|(i, j)| distance_to_polyline(node(i, j), points) - radius)
        .collect();
    let at = |i: usize, j: usize| field[j * (nx + 1) + i];
    // The edges of the grid, each crossed by the contour at most
    // once: the crossing, and the two crossings it is joined to.
    let edges = 2 * (nx + 1) * (ny + 1);
    let mut crossing: Vec<Option<(f32, f32)>> = vec![None; edges];
    let mut joined: Vec<[usize; 2]> = vec![[usize::MAX; 2]; edges];
    let across = |i: usize, j: usize| 2 * (j * (nx + 1) + i);
    let down = |i: usize, j: usize| 2 * (j * (nx + 1) + i) + 1;
    let mut join = |a: usize, b: usize| {
        for (from, to) in [(a, b), (b, a)] {
            let slot = &mut joined[from];
            if slot[0] == usize::MAX {
                slot[0] = to;
            } else {
                slot[1] = to;
            }
        }
    };
    for j in 0..ny {
        for i in 0..nx {
            let (fa, fb, fc, fd) = (at(i, j), at(i + 1, j), at(i + 1, j + 1), at(i, j + 1));
            let case = usize::from(fa < 0.0)
                | usize::from(fb < 0.0) << 1
                | usize::from(fc < 0.0) << 2
                | usize::from(fd < 0.0) << 3;
            if case == 0 || case == 15 {
                continue;
            }
            let (top, right, bottom, left) =
                (across(i, j), down(i + 1, j), across(i, j + 1), down(i, j));
            let mut cross = |edge: usize, f0: f32, f1: f32, p0: (f32, f32), p1: (f32, f32)| {
                let t = (f0 / (f0 - f1)).clamp(0.0, 1.0);
                crossing[edge] = Some((p0.0 + (p1.0 - p0.0) * t, p0.1 + (p1.1 - p0.1) * t));
            };
            let (a, b, c, d) = (
                node(i, j),
                node(i + 1, j),
                node(i + 1, j + 1),
                node(i, j + 1),
            );
            cross(top, fa, fb, a, b);
            cross(right, fb, fc, b, c);
            cross(bottom, fd, fc, d, c);
            cross(left, fa, fd, a, d);
            // The saddles go by the cell's middle.
            let middle = (fa + fb + fc + fd) / 4.0 < 0.0;
            let pairs: &[(usize, usize)] = match case {
                1 | 14 => &[(left, top)],
                2 | 13 => &[(top, right)],
                3 | 12 => &[(left, right)],
                4 | 11 => &[(right, bottom)],
                6 | 9 => &[(top, bottom)],
                7 | 8 => &[(left, bottom)],
                5 if middle => &[(top, right), (left, bottom)],
                5 => &[(left, top), (right, bottom)],
                10 if middle => &[(left, top), (right, bottom)],
                _ => &[(top, right), (left, bottom)],
            };
            for &(p, q) in pairs {
                join(p, q);
            }
        }
    }
    // Each ring walked from its first crossing round to it again.
    let mut seen = vec![false; edges];
    let mut rings = Vec::new();
    for start in 0..edges {
        if seen[start] || crossing[start].is_none() {
            continue;
        }
        let mut ring = Vec::new();
        let (mut prev, mut cur) = (usize::MAX, start);
        loop {
            seen[cur] = true;
            if let Some(p) = crossing[cur] {
                ring.push(p);
            }
            let [x, y] = joined[cur];
            let next = if x != prev && x != usize::MAX { x } else { y };
            if next == usize::MAX || next == start || seen[next] {
                break;
            }
            prev = cur;
            cur = next;
        }
        if ring.len() >= 3 {
            rings.push(ring);
        }
    }
    rings
}

/// The rings as a Path's commands, each moved to, drawn round and
/// closed. Empty for none.
pub fn commands(rings: &[Vec<(f32, f32)>]) -> String {
    let mut s = String::with_capacity(rings.iter().map(|r| r.len() * 14 + 4).sum());
    for ring in rings {
        if ring.len() < 2 {
            continue;
        }
        for (i, (x, y)) in ring.iter().enumerate() {
            let _ = write!(s, "{}{x:.1} {y:.1}", if i == 0 { "M" } else { " L" });
        }
        s.push_str(" Z ");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    fn area(poly: &[(f32, f32)]) -> f32 {
        let n = poly.len();
        (0..n)
            .map(|i| {
                let (a, b) = (poly[i], poly[(i + 1) % n]);
                a.0 * b.1 - b.0 * a.1
            })
            .sum::<f32>()
            .abs()
            / 2.0
    }

    type Segment = ((f32, f32), (f32, f32));

    /// Whether the segments `a` and `b` properly cross.
    fn crosses(a: Segment, b: Segment) -> bool {
        let side = |p: (f32, f32), q: (f32, f32), r: (f32, f32)| {
            (q.0 - p.0) * (r.1 - p.1) - (q.1 - p.1) * (r.0 - p.0)
        };
        let (s1, s2) = (side(a.0, a.1, b.0), side(a.0, a.1, b.1));
        let (s3, s4) = (side(b.0, b.1, a.0), side(b.0, b.1, a.1));
        s1 * s2 < 0.0 && s3 * s4 < 0.0
    }

    /// Every pair of segments, of one ring or of two, that cross.
    fn crossings(rings: &[Vec<(f32, f32)>]) -> usize {
        let segments: Vec<(usize, Segment)> = rings
            .iter()
            .enumerate()
            .flat_map(|(k, r)| (0..r.len()).map(move |i| (k, (r[i], r[(i + 1) % r.len()]))))
            .collect();
        let mut n = 0;
        for (i, (ka, a)) in segments.iter().enumerate() {
            for (kb, b) in &segments[i + 1..] {
                // Neighbors on a ring share an end and do not count.
                if ka == kb && (a.1 == b.0 || a.0 == b.1) {
                    continue;
                }
                if crosses(*a, *b) {
                    n += 1;
                }
            }
        }
        n
    }

    fn all_at_radius(rings: &[Vec<(f32, f32)>], pts: &[(f32, f32)], r: f32, tolerance: f32) {
        for ring in rings {
            for p in ring {
                let d = distance_to_polyline(*p, pts);
                assert!((d - r).abs() < tolerance, "{p:?} at {d}, wanted {r}");
            }
        }
    }

    #[test]
    fn a_spot_is_a_circle() {
        let rings = contours(&[(0.5, 0.5)], 0.1, 0.025);
        assert_eq!(rings.len(), 1);
        assert!(rings[0].len() >= 24, "{}", rings[0].len());
        // Between nodes the field is smooth, so the crossings sit
        // close to the true circle.
        all_at_radius(&rings, &[(0.5, 0.5)], 0.1, 0.025 / 16.0);
        let want = PI * 0.01;
        assert!((area(&rings[0]) - want).abs() < 0.03 * want);
        assert!(contours(&[], 0.1, 0.025).is_empty());
        assert!(contours(&[(0.5, 0.5)], 0.0, 0.025).is_empty());
        assert!(commands(&[]).is_empty());
    }

    #[test]
    fn a_straight_stroke_is_a_stadium() {
        let pts = [(0.2, 0.5), (0.8, 0.5)];
        let rings = contours(&pts, 0.05, 0.0125);
        assert_eq!(rings.len(), 1);
        all_at_radius(&rings, &pts, 0.05, 0.0125 / 2.0);
        let want = PI * 0.05 * 0.05 + 2.0 * 0.05 * 0.6;
        assert!(
            (area(&rings[0]) - want).abs() < 0.03 * want,
            "{}",
            area(&rings[0])
        );
        assert_eq!(crossings(&rings), 0);
        let s = commands(&rings);
        assert!(s.starts_with('M') && s.ends_with(" Z "), "{s}");
        assert_eq!(s.matches('M').count(), 1);
    }

    #[test]
    fn a_zigzag_and_a_hairpin_never_cross_themselves() {
        // A stroke scribbled back and forth over a blemish, rows a
        // radius apart, as the Fill gesture goes; and one that doubles
        // straight back. An offset walk drew chords across both.
        let zigzag = [(0.3, 0.65), (0.5, 0.66), (0.31, 0.68), (0.5, 0.7)];
        let rings = contours(&zigzag, 0.02, 0.005);
        assert!(!rings.is_empty());
        assert_eq!(crossings(&rings), 0, "the zigzag's outline crosses itself");
        all_at_radius(&rings, &zigzag, 0.02, 0.005 / 2.0);
        let hairpin = [(0.2, 0.5), (0.5, 0.5), (0.21, 0.52)];
        let rings = contours(&hairpin, 0.05, 0.0125);
        assert_eq!(rings.len(), 1);
        assert_eq!(crossings(&rings), 0, "the hairpin's outline crosses itself");
        all_at_radius(&rings, &hairpin, 0.05, 0.0125 / 2.0);
        // Its inner feather line, a tenth of the radius, too.
        let rings = contours(&hairpin, 0.005, 0.00125);
        assert!(!rings.is_empty());
        assert_eq!(crossings(&rings), 0);
    }

    #[test]
    fn a_loop_has_a_hole() {
        let loop_: Vec<(f32, f32)> = (0..=48)
            .map(|k| {
                let a = 2.0 * PI * k as f32 / 48.0;
                (0.5 + 0.2 * a.cos(), 0.5 + 0.2 * a.sin())
            })
            .collect();
        let rings = contours(&loop_, 0.03, 0.0075);
        assert_eq!(rings.len(), 2, "an outer ring and the hole");
        let mut radii: Vec<f32> = rings
            .iter()
            .map(|r| (r[0].0 - 0.5).hypot(r[0].1 - 0.5))
            .collect();
        radii.sort_by(f32::total_cmp);
        assert!(
            (radii[0] - 0.17).abs() < 0.004 && (radii[1] - 0.23).abs() < 0.004,
            "{radii:?}"
        );
        assert_eq!(crossings(&rings), 0);
        assert_eq!(commands(&rings).matches('M').count(), 2);
    }
}
