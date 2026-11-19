//! The regions, their distances, and their rings (SPEC.md stages 3 to
//! 5), each in O(N) where the oracle iterates max pools.

/// The label map, as rows of a region table (SPEC stage 3 and 9).
#[derive(Debug, Clone, PartialEq)]
pub struct Labels {
    /// Per half-size cell, row-major: its row in `ids`. Row 0 is the
    /// outside when any cell is outside; every other row is a region.
    pub inv: Vec<u32>,
    /// Per row, ascending: the region's id, its largest flat index on
    /// the padded half grid plus one, or 0 for the outside.
    pub ids: Vec<u32>,
}

impl Labels {
    /// Whether row 0 is the outside (some cell is in no region).
    pub fn has_outside(&self) -> bool {
        self.ids.first() == Some(&0)
    }

    /// The oracle's label map: each cell's id, 0 outside.
    pub fn id(&self, cell: usize) -> u32 {
        self.ids[self.inv[cell] as usize]
    }
}

const OUTSIDE: u32 = u32::MAX;

/// The 8-connected components of `mask` (`width` × `height`, row-major)
/// with the oracle's ids: a region's id is its largest flat index plus
/// one (SPEC stage 3).
///
/// Union-find, in place in the one plane it returns. Scanning in
/// raster order, a cell's earlier neighbors (left, and the three above)
/// all have smaller indices, so linking each neighbor's root under the
/// current cell keeps every parent larger than its child, and a root
/// is its component's largest index: the id the max-pool propagation
/// settles on. The partition is the 8-connected one the 3×3 max pool
/// spreads through. A last pass from the end turns roots into rows;
/// since a parent's index is larger, it is already a row when its
/// children are reached.
pub fn labels(mask: &[bool], width: usize, height: usize) -> Labels {
    let n = width * height;
    debug_assert_eq!(mask.len(), n);
    debug_assert!(n < OUTSIDE as usize);
    let mut p: Vec<u32> = vec![OUTSIDE; n];
    fn find(p: &mut [u32], mut i: u32) -> u32 {
        while p[i as usize] != i {
            // Path halving: a grandparent is larger still.
            let g = p[p[i as usize] as usize];
            p[i as usize] = g;
            i = g;
        }
        i
    }
    for y in 0..height {
        for x in 0..width {
            let i = y * width + x;
            if !mask[i] {
                continue;
            }
            p[i] = i as u32;
            let mut join = |j: usize| {
                if p[j] != OUTSIDE {
                    let r = find(&mut p, j as u32);
                    if r != i as u32 {
                        p[r as usize] = i as u32;
                    }
                }
            };
            if x > 0 {
                join(i - 1);
            }
            if y > 0 {
                let up = i - width;
                if x > 0 {
                    join(up - 1);
                }
                join(up);
                if x + 1 < width {
                    join(up + 1);
                }
            }
        }
    }
    let mut ids = Vec::new();
    let outside = mask.iter().any(|&m| !m);
    if outside {
        ids.push(0);
    }
    for (i, &q) in p.iter().enumerate() {
        if q == i as u32 {
            ids.push(i as u32 + 1);
        }
    }
    let mut next = ids.len() as u32;
    for i in (0..n).rev() {
        let q = p[i];
        p[i] = if q == OUTSIDE {
            0
        } else if q == i as u32 {
            next -= 1;
            next
        } else {
            p[q as usize]
        };
    }
    Labels { inv: p, ids }
}

/// The chessboard distance in cells from each cell to the nearest cell
/// where `target` holds, saturating at `cap`; `cap` everywhere when no
/// cell holds. The frame's edge is no target. Two passes of the 3×3
/// chamfer with unit weights, which is exact for the chessboard metric.
pub fn chessboard(target: &[bool], width: usize, height: usize, cap: u16) -> Vec<u16> {
    let mut d: Vec<u16> = target.iter().map(|&t| if t { 0 } else { cap }).collect();
    let step = |v: u16| v.saturating_add(1).min(cap);
    for y in 0..height {
        for x in 0..width {
            let i = y * width + x;
            if d[i] == 0 {
                continue;
            }
            let mut m = d[i];
            if x > 0 {
                m = m.min(step(d[i - 1]));
            }
            if y > 0 {
                let u = i - width;
                m = m.min(step(d[u]));
                if x > 0 {
                    m = m.min(step(d[u - 1]));
                }
                if x + 1 < width {
                    m = m.min(step(d[u + 1]));
                }
            }
            d[i] = m;
        }
    }
    for y in (0..height).rev() {
        for x in (0..width).rev() {
            let i = y * width + x;
            if d[i] == 0 {
                continue;
            }
            let mut m = d[i];
            if x + 1 < width {
                m = m.min(step(d[i + 1]));
            }
            if y + 1 < height {
                let b = i + width;
                m = m.min(step(d[b]));
                if x > 0 {
                    m = m.min(step(d[b - 1]));
                }
                if x + 1 < width {
                    m = m.min(step(d[b + 1]));
                }
            }
            d[i] = m;
        }
    }
    d
}

/// The oracle's `distance(mask, reach, step=2) * 2` in full-size px from
/// a chessboard distance `c` in cells: 2·ceil(c/2) cells, `reach` when
/// beyond it, times two (SPEC stage 4). `reach` is even.
#[inline]
pub fn quantized(c: u16, reach: u16) -> u16 {
    (c.div_ceil(2) * 2).min(reach) * 2
}

/// The distance from each cell to the nearest cell where `inside` does
/// not hold, quantized as the oracle's: [`quantized`] of the chessboard
/// distance, in full-size px. d0 is this of fa with reach 256, dc0 of fc
/// with reach 256; dcore (reach 8) is min(dc0, 16).
pub fn distance(inside: &[bool], width: usize, height: usize, reach: u16) -> Vec<u16> {
    let outside: Vec<bool> = inside.iter().map(|&v| !v).collect();
    // Any c >= reach - 1 quantizes to the cap.
    let mut d = chessboard(&outside, width, height, reach);
    for v in &mut d {
        *v = quantized(*v, reach);
    }
    d
}

/// Each region's ring: `steps` cells grown out of the regions through
/// the cells outside every region, 8-connected, a cell reached by two
/// regions in the same step taking the larger id (SPEC stage 5). Per
/// cell, the owner's row in `labels`, or 0 off every ring (row 0 is
/// then the outside, never an owner, or there is no cell outside).
///
/// Breadth first: a cell first reached at step t takes the largest
/// owner among its neighbors reached at step t - 1 (the regions' own
/// cells at step 0), which is what the oracle's synchronous 3×3 max
/// gives: a neighbor reached earlier would have reached this cell
/// earlier, and one reached at the same step still held 0. Rows ascend
/// with ids, so the largest row is the largest id.
pub fn ring(fa: &[bool], labels: &Labels, width: usize, height: usize, steps: usize) -> Vec<u32> {
    let n = width * height;
    let mut owner = vec![0u32; n];
    // The step a cell outside was reached at; u8::MAX not yet.
    let mut when = vec![u8::MAX; n];
    debug_assert!(steps < u8::MAX as usize);
    let neighbors = |i: usize, f: &mut dyn FnMut(usize)| {
        let (x, y) = (i % width, i / width);
        for ny in y.saturating_sub(1)..(y + 2).min(height) {
            for nx in x.saturating_sub(1)..(x + 2).min(width) {
                let j = ny * width + nx;
                if j != i {
                    f(j);
                }
            }
        }
    };
    let mut front: Vec<usize> = Vec::new();
    for (i, &inside) in fa.iter().enumerate() {
        if inside {
            let mut edge = false;
            neighbors(i, &mut |j| edge |= !fa[j]);
            if edge {
                front.push(i);
            }
        }
    }
    let mut next: Vec<usize> = Vec::new();
    for t in 1..=steps {
        if front.is_empty() {
            break;
        }
        let stamp = t as u8;
        for &i in &front {
            let own = if fa[i] { labels.inv[i] } else { owner[i] };
            neighbors(i, &mut |j| {
                if fa[j] {
                    return;
                }
                if when[j] == u8::MAX {
                    when[j] = stamp;
                    owner[j] = own;
                    next.push(j);
                } else if when[j] == stamp && own > owner[j] {
                    owner[j] = own;
                }
            });
        }
        std::mem::swap(&mut front, &mut next);
        next.clear();
    }
    owner
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
        fn mask(&mut self, n: usize, per_256: u64) -> Vec<bool> {
            (0..n).map(|_| self.next() % 256 < per_256).collect()
        }
    }

    /// The oracle's ids by flood fill: each component's largest flat
    /// index plus one, 0 outside.
    fn flood(mask: &[bool], w: usize, h: usize) -> Vec<u32> {
        let mut comp = vec![usize::MAX; w * h];
        let mut out = vec![0u32; w * h];
        for s in 0..w * h {
            if !mask[s] || comp[s] != usize::MAX {
                continue;
            }
            let mut stack = vec![s];
            let mut members = vec![];
            comp[s] = s;
            while let Some(i) = stack.pop() {
                members.push(i);
                let (x, y) = ((i % w) as isize, (i / w) as isize);
                for dy in -1..=1 {
                    for dx in -1..=1 {
                        let (nx, ny) = (x + dx, y + dy);
                        if nx < 0 || ny < 0 || nx >= w as isize || ny >= h as isize {
                            continue;
                        }
                        let j = ny as usize * w + nx as usize;
                        if mask[j] && comp[j] == usize::MAX {
                            comp[j] = s;
                            stack.push(j);
                        }
                    }
                }
            }
            let id = *members.iter().max().unwrap() as u32 + 1;
            for m in members {
                out[m] = id;
            }
        }
        out
    }

    #[test]
    fn labels_match_a_flood_fill() {
        let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
        for case in 0..200 {
            let (w, h) = (1 + case % 37, 1 + (case * 7) % 29);
            let mask = rng.mask(w * h, [40, 100, 140, 200, 256][case % 5]);
            let l = labels(&mask, w, h);
            let want = flood(&mask, w, h);
            let got: Vec<u32> = (0..w * h).map(|i| l.id(i)).collect();
            assert_eq!(got, want, "case {case} ({w}x{h})");
            let mut sorted = want.clone();
            sorted.sort_unstable();
            sorted.dedup();
            assert_eq!(l.ids, sorted, "case {case}: rows are the sorted unique ids");
        }
    }

    #[test]
    fn distances_match_the_definition() {
        let mut rng = Rng(0x2545_f491_4f6c_dd1d);
        for case in 0..120 {
            let (w, h) = (1 + case % 41, 1 + (case * 5) % 31);
            let inside = rng.mask(w * h, [250, 240, 200, 256][case % 4]);
            for reach in [4u16, 8, 10, 256] {
                let got = distance(&inside, w, h, reach);
                for y in 0..h {
                    for x in 0..w {
                        // The nearest cell outside, chebyshev.
                        let mut c = u32::MAX;
                        for yy in 0..h {
                            for xx in 0..w {
                                if !inside[yy * w + xx] {
                                    c = c.min(x.abs_diff(xx).max(y.abs_diff(yy)) as u32);
                                }
                            }
                        }
                        let q = if c == u32::MAX {
                            reach as u32
                        } else {
                            (2 * c.div_ceil(2)).min(reach as u32)
                        };
                        assert_eq!(
                            got[y * w + x] as u32,
                            2 * q,
                            "case {case} reach {reach} at ({x}, {y})"
                        );
                    }
                }
            }
        }
    }

    /// The oracle's `grow` as written: `steps` synchronous rounds of
    /// out = out > 0 ? out : 3×3 max(out) · inside, then the regions'
    /// own cells zeroed.
    fn grow(ids: &[u32], fa: &[bool], w: usize, h: usize, steps: usize) -> Vec<u32> {
        let mut out = ids.to_vec();
        for _ in 0..steps {
            let mut new = out.clone();
            for y in 0..h {
                for x in 0..w {
                    let i = y * w + x;
                    if out[i] > 0 {
                        continue;
                    }
                    let mut m = 0;
                    for yy in y.saturating_sub(1)..(y + 2).min(h) {
                        for xx in x.saturating_sub(1)..(x + 2).min(w) {
                            m = m.max(out[yy * w + xx]);
                        }
                    }
                    new[i] = if fa[i] { 0 } else { m };
                }
            }
            out = new;
        }
        out.iter()
            .zip(ids)
            .map(|(&o, &id)| if id > 0 { 0 } else { o })
            .collect()
    }

    #[test]
    fn rings_match_the_synchronous_growth() {
        let mut rng = Rng(0x0123_4567_89ab_cdef);
        for case in 0..150 {
            let (w, h) = (2 + case % 45, 2 + (case * 3) % 33);
            let fa = rng.mask(w * h, [6, 12, 30, 80][case % 4]);
            let l = labels(&fa, w, h);
            let ids: Vec<u32> = (0..w * h).map(|i| l.id(i)).collect();
            for steps in [1usize, 3, 12] {
                let want = grow(&ids, &fa, w, h, steps);
                let got = ring(&fa, &l, w, h, steps);
                let got: Vec<u32> = got
                    .iter()
                    .map(|&r| if r == 0 { 0 } else { l.ids[r as usize] })
                    .collect();
                assert_eq!(got, want, "case {case} ({w}x{h}) steps {steps}");
            }
        }
    }
}
