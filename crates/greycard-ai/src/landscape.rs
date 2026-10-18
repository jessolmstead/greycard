//! The landscape: the picture's water, foliage, mountains and
//! buildings, from the Sky prior's own scene labels (EoMT-S, COCO
//! panoptic, [`crate::sky`]), and nothing where the model is not sure
//! of them. The model runs once for the sky and all four, and one
//! labeling pass makes every map they read ([`Prior::labeled`]).
//!
//! The stages, each a function with a CPU reference and a test (the
//! notes' landscape trial on 135 frames is the spec):
//!
//! 1. **The groups** ([`Group`]): COCO's classes as the trial grouped
//!    them. Water is sea, river and water-other (a lake is "sea"),
//!    Foliage is tree-merged (bushes and hedges too), Mountain is
//!    mountain-merged (hills, distant ridges in haze), Building is
//!    building-other-merged, house and roof.
//! 2. **The measure and the gate** ([`Scene::measure`], [`gate`]): the
//!    mean winning class probability over the group's labeled pixels,
//!    at or above the group's own bar (Water 0.85, Foliage 0.93,
//!    Mountain 0.93, Building 0.90), and a core of a quarter of a
//!    percent of the frame: the group's labeled pixels where its soft
//!    map is over 0.8, after an
//!    erosion of one percent of the long side. The soft map divides by
//!    the kept queries alone: over all 200, as the sky's is, the
//!    leftover queries dilute a group the model is less sure of than
//!    it is of a sky to nothing. The mean, not the best query: a
//!    lavender field is painted foliage by a query 99.4% sure, but the
//!    mean over it is 0.73. Under this gate the four had no wrong frame
//!    of the trial's 135.
//! 3. **The outline**: the prior's own labels of the group. Not SAM
//!    seeded as the sky is: a point on a canopy or a hillside gets one
//!    tree or one slope, and the union of eight is a patchwork.
//! 4. **The edge** ([`refine`]): for Water and Foliage the color-line
//!    matte ([`crate::matte`]), the group standing as the sky; Foliage
//!    then cut by one minus the found sky's matte ([`found_sky`]),
//!    which takes the sky between its branches; for Mountain and Building
//!    the labels feathered at the Subject rule ([`sky::feathered`]),
//!    never the matte, whose color-line guards were tuned for a bright
//!    sky and spread a mountain over the wooded hillside of nearly its
//!    own colors beside it.

use crate::image::Mask;
use crate::matte::{self, FLOWER_CLASS, TREE_CLASS};
use crate::sky::{self, Frame, Labeled, Logits, Prior, SKY_CLASS, erode};

/// The gate's core: the share of the frame the soft map over
/// `CONFIDENT` must cover once eroded, and the erosion as a share of
/// the long side. Sky's own.
const MIN_CORE: f32 = 0.0025;
const CONFIDENT: f32 = 0.8;
const CORE_EROSION: f32 = 0.01;

/// A part of the landscape the labels offer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Group {
    Water,
    Foliage,
    Mountain,
    Building,
}

/// How a group's edge is made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    /// The color-line matte from the labels.
    Matte,
    /// The labels feathered by the guided filter at the Subject rule.
    Feathered,
}

impl Group {
    pub const ALL: [Group; 4] = [
        Group::Water,
        Group::Foliage,
        Group::Mountain,
        Group::Building,
    ];

    /// Its classes among the model's (detectron2's contiguous COCO
    /// panoptic ids).
    pub fn classes(self) -> &'static [u8] {
        match self {
            // river, sea, water-other
            Group::Water => &[99, 103, 113],
            // tree-merged
            Group::Foliage => &[TREE_CLASS],
            // mountain-merged
            Group::Mountain => &[124],
            // building-other-merged, house, roof
            Group::Building => &[129, 91, 101],
        }
    }

    /// Whether the found sky caps it: its shape is at most one minus the
    /// sky's matte. Foliage's labels take the sky between its branches,
    /// and its matte, whose guards were tuned for a sky against a tree
    /// and not the other way, cannot tell the gaps from the leaves; the
    /// sky's matte, made from the sky's side, can.
    pub fn capped_by_sky(self) -> bool {
        self == Group::Foliage
    }

    /// The gate's bar on the mean class probability over its pixels.
    /// Each sits in the gap the trial found between its wrong frames
    /// and its right ones: Water's wrong frame was a painted mural at
    /// 0.59; Foliage's were all under 0.93; Mountain's a fog bank at
    /// 0.904 and a roof at 0.92; Building's a temple interior at 0.82.
    pub fn bar(self) -> f32 {
        match self {
            Group::Water => 0.85,
            Group::Foliage => 0.93,
            Group::Mountain => 0.93,
            Group::Building => 0.90,
        }
    }

    pub fn edge(self) -> Edge {
        match self {
            Group::Water | Group::Foliage => Edge::Matte,
            Group::Mountain | Group::Building => Edge::Feathered,
        }
    }

    /// Its name in words, lower case.
    pub fn name(self) -> &'static str {
        match self {
            Group::Water => "water",
            Group::Foliage => "foliage",
            Group::Mountain => "mountain",
            Group::Building => "building",
        }
    }

    /// The group by its name.
    pub fn of_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|g| g.name() == name)
    }

    fn index(self) -> usize {
        Self::ALL
            .iter()
            .position(|&g| g == self)
            .expect("every group is in ALL")
    }
}

/// What the labels say of a group on a frame: the share of the frame
/// labeled it, the mean of the winning query's class probability over
/// those pixels, and the share of the frame its eroded core covers.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Measure {
    pub area: f32,
    pub mean: f32,
    pub core: f32,
}

/// Why the gate refused a group.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Why {
    /// Nothing is labeled the group.
    Absent,
    /// The model's mean class probability over the group: under its bar.
    Unsure(f32),
    /// Too little of the soft map over 0.8 survives the erosion: the
    /// share that did.
    NoCore(f32),
}

impl Why {
    /// The reason in words, for the log.
    pub fn says(self, group: Group) -> String {
        let name = group.name();
        match self {
            Why::Absent => format!("the model calls nothing {name}"),
            Why::Unsure(p) => format!("the model is only {:.1}% sure of its {name}", p * 100.0),
            Why::NoCore(core) => format!(
                "too little the model is sure is {name} ({:.2}% of the frame)",
                core * 100.0
            ),
        }
    }
}

/// The gate on a group's measure: the rule that must not fail is the
/// sky's, no shape where there is none.
pub fn gate(group: Group, m: Measure) -> Result<(), Why> {
    if m.area <= 0.0 {
        return Err(Why::Absent);
    }
    if m.mean < group.bar() {
        return Err(Why::Unsure(m.mean));
    }
    if m.core < MIN_CORE {
        return Err(Why::NoCore(m.core));
    }
    Ok(())
}

/// A shape the labels offer on a frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Kind {
    Sky,
    Group(Group),
}

/// One thing the Landscape button offers: what, and the share of the
/// frame labeled it, which orders the offer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Offer {
    pub kind: Kind,
    pub area: f32,
}

/// Everything one labeling pass says of a picture, kept for its
/// develop: the sky's prior, every group's measure, and the groups'
/// soft maps at the size the labels were made at, brought to the
/// picture's size only when a shape is made.
#[derive(Debug, Clone, PartialEq)]
pub struct Scene {
    pub prior: Prior,
    work: (usize, usize),
    soft: Vec<Vec<f32>>,
    measures: [Measure; 4],
}

impl Scene {
    /// The labeling rule ([`Prior::labeled`]) at `width` × `height`,
    /// and every group measured.
    pub fn label(logits: &Logits, content: (usize, usize), width: usize, height: usize) -> Self {
        Self::of(Prior::labeled(logits, content, width, height))
    }

    /// The scene of one labeling pass.
    pub fn of(l: Labeled) -> Self {
        let prior = l.prior;
        let (w, h) = (prior.width, prior.height);
        let n = (w * h).max(1) as f32;
        let win = sky::nearest(&l.win, l.work.0, l.work.1, w, h);
        let r = ((CORE_EROSION * w.max(h) as f32).round() as usize).max(1);
        let mut measures = [Measure::default(); 4];
        for g in Group::ALL {
            let classes = g.classes();
            let (mut count, mut sum) = (0usize, 0.0f64);
            for (&label, &p) in prior.labels.iter().zip(&win) {
                if classes.contains(&label) {
                    count += 1;
                    sum += p as f64;
                }
            }
            if count == 0 {
                continue;
            }
            // The core is of the group's own pixels: over the kept
            // queries alone, a pixel no kept query claims divides one
            // faint mask by others, and the group's share of that can
            // be anything.
            let soft = bring(&l.groups[g.index()], l.work, (w, h));
            let confident: Vec<bool> = soft
                .iter()
                .zip(&prior.labels)
                .map(|(&p, l)| p > CONFIDENT && classes.contains(l))
                .collect();
            let core = erode(&confident, w, h, r).iter().filter(|&&v| v).count() as f32 / n;
            measures[g.index()] = Measure {
                area: count as f32 / n,
                mean: (sum / count as f64) as f32,
                core,
            };
        }
        Self {
            prior,
            work: l.work,
            soft: l.groups,
            measures,
        }
    }

    /// The size the soft maps are kept at.
    pub fn work(&self) -> (usize, usize) {
        self.work
    }

    pub fn measure(&self, group: Group) -> Measure {
        self.measures[group.index()]
    }

    pub fn gate(&self, group: Group) -> Result<(), Why> {
        gate(group, self.measure(group))
    }

    /// What the Landscape button offers: the sky when the sky's own
    /// gate passes, and each group whose gate passes, the largest
    /// first.
    pub fn offers(&self) -> Vec<Offer> {
        let mut offers: Vec<Offer> = Group::ALL
            .into_iter()
            .filter(|&g| self.gate(g).is_ok())
            .map(|g| Offer {
                kind: Kind::Group(g),
                area: self.measure(g).area,
            })
            .collect();
        if sky::gate(&self.prior).is_ok() {
            offers.push(Offer {
                kind: Kind::Sky,
                area: self.prior.sky_area(),
            });
        }
        offers.sort_by(|a, b| b.area.total_cmp(&a.area));
        offers
    }

    /// A group's soft map at the picture's size.
    pub fn soft(&self, group: Group) -> Vec<f32> {
        bring(
            &self.soft[group.index()],
            self.work,
            (self.prior.width, self.prior.height),
        )
    }

    /// The group's labels, one or nothing, at the picture's size: its
    /// outline.
    pub fn labeled(&self, group: Group) -> Mask {
        let classes = group.classes();
        Mask::new(
            self.prior.width,
            self.prior.height,
            self.prior
                .labels
                .iter()
                .map(|&l| if classes.contains(&l) { 1.0 } else { 0.0 })
                .collect(),
        )
    }

    /// The prior as the matte reads one for `group`: the group's soft
    /// map as the sky's, its classes labeled sky, the real sky, tree and
    /// flower moved to stuff ids nothing treats specially, and the
    /// things left as they are (known not the group). No class is one
    /// the group shows through: the sky beside foliage is known not
    /// foliage past the matte's growth band and held down by the
    /// foliage's own map within it, and the gaps between its branches
    /// are the found sky's to cut ([`find`]).
    pub fn as_sky(&self, group: Group) -> Prior {
        let classes = group.classes();
        let soft = self.soft(group);
        let labels = self
            .prior
            .labels
            .iter()
            .map(|&c| {
                if classes.contains(&c) {
                    SKY_CLASS
                } else if c == SKY_CLASS {
                    202
                } else if c == TREE_CLASS {
                    200
                } else if c == FLOWER_CLASS {
                    201
                } else {
                    c
                }
            })
            .collect();
        Prior {
            width: self.prior.width,
            height: self.prior.height,
            sky: soft.clone(),
            sky_kept: soft,
            labels,
            sure: self.measure(group).mean,
        }
    }
}

/// `map` of size `from` brought to `to` bilinearly, as the sky's soft
/// map is.
fn bring(map: &[f32], from: (usize, usize), to: (usize, usize)) -> Vec<f32> {
    if from == to {
        return map.to_vec();
    }
    Mask::new(from.0, from.1, map.to_vec())
        .resampled(to.0, to.1)
        .data
}

/// The group's edge: its outline (one or nothing, at the preview's
/// size) as the matte the raster is made from, `size` (width, height),
/// and whether the color-line matte made it. Water and Foliage take the
/// matte where it has something to go on (some known group once the
/// outline is shrunk, a linear frame of the preview's shape), else the
/// outline feathered; Mountain and Building take the outline feathered.
pub fn refine(
    group: Group,
    outline: &Mask,
    scene: &Scene,
    frame: &Frame,
    size: (usize, usize),
) -> (Mask, bool) {
    if group.edge() == Edge::Matte
        && let Some(m) = matte::color_line(
            outline,
            &scene.as_sky(group),
            frame.linear,
            &[],
            matte::Hold::None,
            size.0,
            size.1,
        )
    {
        return (m, true);
    }
    (
        sky::feathered(outline, frame).resampled(size.0, size.1),
        false,
    )
}

/// A group's shape found, or why not.
pub enum Found {
    Shape {
        matte: Mask,
        /// The color-line matte made the edge (not the feathering).
        matted: bool,
    },
    None(Why),
}

/// The sky a group capped by it is cut by, at `size`: the Sky shape
/// with no picks, as `sky::find` makes it (SAM's outline through
/// `decode`, or the prior's labels without it), so a Sky beside the
/// Foliage made the same way sums with it to one at most. None when
/// the sky's gate finds no sky, or the sky cannot be made.
pub fn found_sky(
    scene: &Scene,
    frame: &Frame,
    size: (usize, usize),
    decode: Option<&mut sky::Decode>,
) -> Option<Mask> {
    let mut times = sky::Times::default();
    match sky::find(&scene.prior, frame, &[], decode, size, &mut times) {
        Ok(sky::Found::Sky { matte, .. }) => Some(matte),
        _ => None,
    }
}

/// Where the sky's share reaches `SKY_CUT.0` the cut starts, and by
/// `SKY_CUT.1` nothing of the group is left: past one minus the sky,
/// since a sky the Sky shape holds at only 0.6 or 0.7 in open blue
/// (where its outline left a corner out) is still open sky, and one
/// minus it would leave the group a third of it.
const SKY_CUT: (f32, f32) = (0.2, 0.55);

/// `matte` cut by `sky`, pixel by pixel, the two the same size: at most
/// one minus the sky's share brought up by the ramp `SKY_CUT`, so never
/// more than one minus the sky, and nothing where the sky holds the
/// pixel at `SKY_CUT.1` or more.
pub fn cap(matte: &mut Mask, sky: &Mask) {
    assert_eq!((matte.width, matte.height), (sky.width, sky.height));
    let (a, b) = SKY_CUT;
    for (m, &s) in matte.data.iter_mut().zip(&sky.data) {
        let cut = ((s - a) / (b - a)).clamp(0.0, 1.0).max(s);
        *m = m.min(1.0 - cut);
    }
}

/// The gate, then the edge from the labels, as a matte of `size`; a
/// group the sky caps cut by `sky`, the found sky at the same size
/// ([`found_sky`]), when there is one.
pub fn find(
    scene: &Scene,
    group: Group,
    frame: &Frame,
    size: (usize, usize),
    sky: Option<&Mask>,
) -> Found {
    if let Err(why) = scene.gate(group) {
        return Found::None(why);
    }
    let (mut matte, matted) = refine(group, &scene.labeled(group), scene, frame, size);
    if group.capped_by_sky()
        && let Some(sky) = sky
    {
        cap(&mut matte, sky);
    }
    Found::Shape { matte, matted }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image::Rgb8;
    use crate::sky::{CLASSES, MASK_SIDE, QUERIES};
    use greycard_core::image::WorkingImage;

    type Region<'a> = &'a dyn Fn(usize, usize) -> bool;

    /// Logits for a list of queries: (class, class probability, mask
    /// region on the 160 grid), every other query "no object", and
    /// `diluting` queries of no class in particular whose masks cover
    /// the whole square faintly, as the model's leftover queries do.
    fn logits_of(queries: &[(usize, f32, Region)], diluting: usize) -> Logits {
        let mut class = vec![0.0f32; QUERIES * CLASSES];
        let mut masks = vec![-10.0f32; QUERIES * MASK_SIDE * MASK_SIDE];
        for q in 0..QUERIES {
            class[q * CLASSES + CLASSES - 1] = 20.0;
        }
        for (q, (c, p, region)) in queries.iter().enumerate() {
            let row = &mut class[q * CLASSES..(q + 1) * CLASSES];
            row.fill(0.0);
            // One class at L, 133 at 0: p = e^L / (e^L + 133).
            row[*c] = (p * 133.0 / (1.0 - p)).ln();
            for y in 0..MASK_SIDE {
                for x in 0..MASK_SIDE {
                    if region(x, y) {
                        masks[q * MASK_SIDE * MASK_SIDE + y * MASK_SIDE + x] = 10.0;
                    }
                }
            }
        }
        for q in queries.len()..queries.len() + diluting {
            class[q * CLASSES..(q + 1) * CLASSES].fill(0.0);
            masks[q * MASK_SIDE * MASK_SIDE..(q + 1) * MASK_SIDE * MASK_SIDE].fill(-4.4);
        }
        Logits { class, masks }
    }

    const SEA: usize = 103;
    const MOUNTAIN: usize = 124;
    const HOUSE: usize = 91;

    /// A coast: sky at the top, a mountain, then the sea, the sea's
    /// query only 90 percent sure.
    fn coast(sea: f32, diluting: usize) -> Scene {
        let sky = |_: usize, y: usize| y < 40;
        let mountain = |_: usize, y: usize| (40..80).contains(&y);
        let water = |_: usize, y: usize| y >= 80;
        let logits = logits_of(
            &[
                (SKY_CLASS as usize, 0.999, &sky),
                (MOUNTAIN, 0.97, &mountain),
                (SEA, sea, &water),
            ],
            diluting,
        );
        Scene::label(&logits, (640, 640), 320, 320)
    }

    #[test]
    fn every_group_is_measured_from_the_one_labeling() {
        let scene = coast(0.9, 0);
        let water = scene.measure(Group::Water);
        assert!((water.area - 0.5).abs() < 0.02, "{water:?}");
        assert!((water.mean - 0.9).abs() < 1e-3, "{water:?}");
        assert!(water.core > 0.4, "{water:?}");
        let mountain = scene.measure(Group::Mountain);
        assert!((mountain.area - 0.25).abs() < 0.02, "{mountain:?}");
        assert!((mountain.mean - 0.97).abs() < 1e-3);
        assert_eq!(scene.measure(Group::Building), Measure::default());
        assert_eq!(scene.gate(Group::Building), Err(Why::Absent));
        // The sky's own prior is the same as the sky's labeling alone.
        let logits = logits_of(
            &[
                (SKY_CLASS as usize, 0.999, &|_, y| y < 40),
                (MOUNTAIN, 0.97, &|_, y| (40..80).contains(&y)),
                (SEA, 0.9, &|_, y| y >= 80),
            ],
            0,
        );
        assert_eq!(scene.prior, Prior::label(&logits, (640, 640), 320, 320));
    }

    #[test]
    fn the_offer_is_what_passes_its_gate_largest_first() {
        let scene = coast(0.9, 0);
        let offers = scene.offers();
        let kinds: Vec<Kind> = offers.iter().map(|o| o.kind).collect();
        assert_eq!(
            kinds,
            [
                Kind::Group(Group::Water),
                Kind::Group(Group::Mountain),
                Kind::Sky
            ]
        );
        assert!(offers.windows(2).all(|w| w[0].area >= w[1].area));
    }

    /// Each group's bar is its own: a sea the model is 84 percent sure
    /// of is held back, at 86 it is offered; a mountain at 92 is held
    /// back where a sea at 92 would pass.
    #[test]
    fn a_group_under_its_bar_is_not_offered() {
        let unsure = coast(0.84, 0);
        assert!(
            matches!(unsure.gate(Group::Water), Err(Why::Unsure(p)) if (p - 0.84).abs() < 1e-3)
        );
        assert!(coast(0.86, 0).gate(Group::Water).is_ok());
        let fog = |_: usize, y: usize| (40..80).contains(&y);
        let logits = logits_of(&[(MOUNTAIN, 0.92, &fog), (SEA, 0.92, &|_, y| y >= 80)], 0);
        let scene = Scene::label(&logits, (640, 640), 320, 320);
        assert!(matches!(scene.gate(Group::Mountain), Err(Why::Unsure(_))));
        assert!(scene.gate(Group::Water).is_ok());
        assert!(
            !scene
                .offers()
                .iter()
                .any(|o| o.kind == Kind::Group(Group::Mountain))
        );
    }

    /// The core divides by the kept queries: a sure sea the leftover
    /// queries would dilute under 0.8 over all 200 keeps its core.
    #[test]
    fn the_soft_map_divides_by_the_kept_queries_alone() {
        let scene = coast(0.95, 60);
        let soft = scene.soft(Group::Water);
        let middle = soft[250 * 320 + 160];
        assert!(middle > 0.9, "{middle}");
        assert!(scene.gate(Group::Water).is_ok());
        // Over every query, as the sky's is, the same sea is under 0.8.
        let over_all = scene.prior.sky[20 * 320 + 160];
        assert!(over_all < 0.8, "the sky over all queries {over_all}");
        let kept = scene.prior.sky_kept[20 * 320 + 160];
        assert!(kept > 0.95, "the sky over the kept queries {kept}");
        // So the sky's gate, whose core is over the kept queries too,
        // passes the same sure sky.
        assert_eq!(sky::gate(&scene.prior), Ok(()));
    }

    /// A sliver: labeled and sure, but thinner than the erosion.
    #[test]
    fn a_group_with_no_core_is_not_offered() {
        let roof = |_: usize, y: usize| y == 100;
        let logits = logits_of(&[(HOUSE, 0.99, &roof)], 0);
        let scene = Scene::label(&logits, (640, 640), 640, 640);
        let m = scene.measure(Group::Building);
        assert!(m.area > 0.0 && m.mean > 0.98, "{m:?}");
        assert_eq!(scene.gate(Group::Building), Err(Why::NoCore(0.0)));
        assert!(scene.offers().is_empty());
    }

    /// The trial's lavender field: a query sure of its foliage over a
    /// small part, an unsure one over the rest. The mean holds it back
    /// where the best query would not.
    #[test]
    fn the_gate_is_the_mean_not_the_best_query() {
        let sure = |x: usize, y: usize| y >= 100 && x < 30;
        let unsure = |x: usize, y: usize| y >= 100 && x >= 30;
        let logits = logits_of(
            &[
                (TREE_CLASS as usize, 0.994, &sure),
                (TREE_CLASS as usize, 0.7, &unsure),
            ],
            0,
        );
        let scene = Scene::label(&logits, (640, 640), 320, 320);
        let m = scene.measure(Group::Foliage);
        assert!(m.mean < 0.8 && m.core > 0.05, "{m:?}");
        assert!(matches!(scene.gate(Group::Foliage), Err(Why::Unsure(_))));
    }

    #[test]
    fn the_matte_reads_the_group_as_its_sky() {
        let scene = coast(0.95, 0);
        let p = scene.as_sky(Group::Water);
        assert_eq!(p.labels[300 * 320 + 160], SKY_CLASS);
        // The real sky and the mountain are known not-water.
        assert_eq!(p.labels[10 * 320 + 160], 202);
        assert_eq!(p.labels[150 * 320 + 160], MOUNTAIN as u8);
        assert!(p.sky[300 * 320 + 160] > 0.9 && p.sky[10 * 320 + 160] < 0.05);
        // Nor is the sky something foliage shows through: known not
        // foliage, as it is known not water.
        let p = scene.as_sky(Group::Foliage);
        assert_eq!(p.labels[10 * 320 + 160], 202);
    }

    fn frame_of(
        w: usize,
        h: usize,
        f: impl Fn(usize, usize) -> [f32; 3],
    ) -> (Rgb8, Vec<f32>, WorkingImage) {
        let mut linear = WorkingImage::new(2 * w, 2 * h);
        let mut data = Vec::with_capacity(w * h * 3);
        for y in 0..h {
            for x in 0..w {
                let p = f(x, y);
                data.extend(p.map(|v| (v.powf(1.0 / 2.2) * 255.0).round() as u8));
            }
        }
        for y in 0..2 * h {
            for x in 0..2 * w {
                linear.data[(y * 2 * w + x) * 3..][..3].copy_from_slice(&f(x / 2, y / 2));
            }
        }
        let display = Rgb8::new(w, h, data);
        let luma = display.luma();
        (display, luma, linear)
    }

    /// The coast made: the water takes the matte and the mountain the
    /// feathered labels, each one on its own and nothing elsewhere; a
    /// group the gate refuses makes nothing.
    #[test]
    fn find_makes_each_group_with_its_own_edge() {
        let scene = coast(0.95, 0);
        let (w, h) = (320usize, 320usize);
        let (display, luma, linear) = frame_of(w, h, |_, y| {
            if y < 80 {
                [0.3, 0.45, 0.8]
            } else if y < 160 {
                [0.08, 0.07, 0.05]
            } else {
                [0.02, 0.06, 0.12]
            }
        });
        let frame = Frame {
            display: &display,
            luma: &luma,
            linear: &linear,
        };
        let Found::Shape { matte, matted } = find(&scene, Group::Water, &frame, (w, h), None)
        else {
            panic!("the sea is there");
        };
        assert!(matted);
        assert!(matte.at(160, 280) > 0.95 && matte.at(160, 100) < 0.05 && matte.at(160, 20) < 0.05);
        let Found::Shape { matte, matted } = find(&scene, Group::Mountain, &frame, (w, h), None)
        else {
            panic!("the mountain is there");
        };
        assert!(!matted);
        assert!(matte.at(160, 120) > 0.95 && matte.at(160, 20) < 0.05 && matte.at(160, 280) < 0.05);
        let refused = coast(0.8, 0);
        assert!(matches!(
            find(&refused, Group::Water, &frame, (w, h), None),
            Found::None(Why::Unsure(_))
        ));
    }

    /// A canopy against a sky, the labels calling the whole canopy tree,
    /// the sky between its branches with it: Foliage's own matte takes
    /// the gaps whole, and cut by the found sky it leaves them, keeps
    /// the branches, and paints none of the open sky. With no sky found
    /// there is nothing to cut by, and nothing is.
    #[test]
    fn foliage_is_cut_by_the_found_sky() {
        let sky_rows = |_: usize, y: usize| y < 40;
        let tree_rows = |_: usize, y: usize| y >= 40;
        let logits = logits_of(
            &[
                (SKY_CLASS as usize, 0.999, &sky_rows),
                (TREE_CLASS as usize, 0.99, &tree_rows),
            ],
            0,
        );
        let (w, h) = (320usize, 320usize);
        let scene = Scene::label(&logits, (640, 640), w, h);
        let blue = [0.3, 0.45, 0.8];
        let gap = |x: usize| (x / 6).is_multiple_of(2);
        let (display, luma, linear) = frame_of(w, h, |x, y| {
            if y < 80 || (y < 200 && gap(x)) {
                blue
            } else {
                [0.03, 0.04, 0.02]
            }
        });
        let frame = Frame {
            display: &display,
            luma: &luma,
            linear: &linear,
        };
        let sky = found_sky(&scene, &frame, (w, h), None).expect("the sky is found");
        let mean = |m: &Mask, rows: std::ops::Range<usize>, gaps: bool| {
            let (mut s, mut n) = (0.0, 0);
            for y in rows {
                for x in 20..300 {
                    // Away from a column's edges, in a gap or on a branch.
                    if x % 6 >= 2 && x % 6 <= 3 && gap(x) == gaps {
                        s += m.at(x, y);
                        n += 1;
                    }
                }
            }
            s / n as f32
        };
        let Found::Shape { matte: whole, .. } = find(&scene, Group::Foliage, &frame, (w, h), None)
        else {
            panic!("the canopy is there");
        };
        assert!(
            mean(&whole, 90..110, true) > 0.9,
            "uncut, the gaps are foliage"
        );
        let Found::Shape { matte: cut, .. } =
            find(&scene, Group::Foliage, &frame, (w, h), Some(&sky))
        else {
            panic!("the canopy is there");
        };
        let (gaps, branches) = (mean(&cut, 90..110, true), mean(&cut, 90..110, false));
        assert!(gaps < 0.3, "the gaps cut: {gaps}");
        assert!(branches > 0.7, "the branches kept: {branches}");
        assert!(mean(&cut, 10..60, true) < 0.05 && mean(&cut, 10..60, false) < 0.05);
        for (c, s) in cut.data.iter().zip(&sky.data) {
            assert!(c + s <= 1.0 + 1e-6);
        }
        // Water is never cut.
        assert!(!Group::Water.capped_by_sky() && Group::Foliage.capped_by_sky());
    }

    #[test]
    fn a_reason_reads_in_words() {
        assert_eq!(
            Why::Absent.says(Group::Water),
            "the model calls nothing water"
        );
        assert_eq!(
            Why::Unsure(0.904).says(Group::Mountain),
            "the model is only 90.4% sure of its mountain"
        );
        assert_eq!(Group::of_name("foliage"), Some(Group::Foliage));
    }
}
