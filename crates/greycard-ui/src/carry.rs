//! What a sheet carries: the checks of the sync, paste and preset
//! sheets, a section a heading with its parts under it and the masks
//! listed one by one.
//!
//! A section with parts (`Section::parts`) has one check a part, and
//! its heading's check is on when all of them are, off when none is,
//! and shows "some" between. A section without parts has one check.
//! The masks' parts are the masks of the frame the sheet reads from,
//! by name as the Masks tab shows them; with none, one check, which
//! with "replace" clears the frames' own as ticking the masks always
//! did. Under them, whether the masks replace the frames' or join
//! them.
//!
//! What the checks make is a `Choice` (`choice`): a section all on
//! goes whole, as before parts, so a sheet left at its sections writes
//! and does what it always did; a section partly on sends its parts.
//!
//! The sheet's list is laid out here, not in Slint: two columns of
//! sections, each column a run of whole sections in the panel's order,
//! split where the two come out nearest in height; a section's parts
//! two to a line under its heading. Pure, so the checks and the layout
//! are tested without a window.

use std::rc::Rc;

use greycard_edit::Edit;
use greycard_edit::preset::{Choice, Masks, Section};
use slint::{Model, VecModel};

use crate::CarryRow;

/// What a row of the list is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Target {
    /// A section's heading, by its place in `Section::ALL`.
    Section(usize),
    /// A part of one: the section's place, then the part's (or the
    /// mask's) under it.
    Part(usize, usize),
    /// Whether the masks replace the frames' or join them.
    Join,
}

/// A row's kind, as `CarryRow::kind` has it.
const HEADING: i32 = 0;
const PART: i32 = 1;
const JOIN: i32 = 2;

/// The checks of one sheet.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Checks {
    /// A section's checks, in `Section::ALL`'s order: one a part, one
    /// a mask for the masks, or one alone for a section without.
    on: Vec<Vec<bool>>,
    /// The masks listed under Masks: id and name, in the frame's order.
    masks: Vec<(u64, String)>,
    /// The masks join the frames' rather than replace them.
    pub(crate) join: bool,
}

/// Where Masks sits in `Section::ALL`.
fn masks_at() -> usize {
    Section::ALL
        .iter()
        .position(|s| *s == Section::Adjustments)
        .expect("the masks are a section")
}

/// The masks of `edit` as the sheet lists them: id and the name the
/// Masks tab shows.
pub(crate) fn masks_of(edit: &Edit) -> Vec<(u64, String)> {
    edit.adjustments
        .iter()
        .map(|a| {
            let name = if a.name.trim().is_empty() {
                format!("Mask {}", a.id)
            } else {
                a.name.clone()
            };
            (a.id, name)
        })
        .collect()
}

impl Checks {
    /// Every check as `pick` says: the section, and the part's place
    /// under it (always 0 for a section without parts).
    pub(crate) fn from_fn(
        masks: Vec<(u64, String)>,
        pick: impl Fn(Section, usize) -> bool,
    ) -> Self {
        let on = Section::ALL
            .iter()
            .map(|&s| (0..slots(s, masks.len())).map(|j| pick(s, j)).collect())
            .collect();
        Self {
            on,
            masks,
            join: false,
        }
    }

    /// Each section whole, on or off as `pick` says.
    pub(crate) fn new(masks: Vec<(u64, String)>, pick: impl Fn(Section) -> bool) -> Self {
        Self::from_fn(masks, |s, _| pick(s))
    }

    /// These checks over another frame's masks: what the sheet last
    /// had, opened on a frame whose masks are not the ones it listed.
    /// The masks all on or all off stay so; some on, the ones of the
    /// same name are.
    pub(crate) fn with_masks(&self, masks: Vec<(u64, String)>) -> Self {
        let m = masks_at();
        let was = self.state(m);
        let names: Vec<&str> = self
            .masks
            .iter()
            .zip(&self.on[m])
            .filter(|(_, on)| **on)
            .map(|(mask, _)| mask.1.as_str())
            .collect();
        let mut on = self.on.clone();
        on[m] = (0..slots(Section::Adjustments, masks.len()))
            .map(|j| match was {
                State::On => true,
                State::Off => false,
                State::Partly => masks
                    .get(j)
                    .is_some_and(|(_, n)| names.contains(&n.as_str())),
            })
            .collect();
        Self {
            on,
            masks,
            join: self.join,
        }
    }

    /// How section `i`'s checks stand.
    pub(crate) fn state(&self, i: usize) -> State {
        let on = &self.on[i];
        if on.iter().all(|b| *b) {
            State::On
        } else if on.iter().any(|b| *b) {
            State::Partly
        } else {
            State::Off
        }
    }

    /// Check or uncheck everything: every section, every part, every
    /// mask.
    pub(crate) fn set_all(&mut self, on: bool) {
        for checks in &mut self.on {
            checks.fill(on);
        }
    }

    /// What a click on a row does: a heading all on goes off, and
    /// otherwise all on; a part or a mask turns over; the masks' line
    /// is not a check.
    pub(crate) fn toggle(&mut self, target: Target) {
        match target {
            Target::Section(i) => {
                let on = self.state(i) != State::On;
                self.on[i].fill(on);
            }
            Target::Part(i, j) => {
                if let Some(b) = self.on[i].get_mut(j) {
                    *b = !*b;
                }
            }
            Target::Join => {}
        }
    }

    /// What the checks choose: a section all on whole, one partly on
    /// by its parts, the masks by id when some are on.
    pub(crate) fn choice(&self) -> Choice {
        let mut choice = Choice {
            masks_land: if self.join {
                Masks::Join
            } else {
                Masks::Replace
            },
            ..Choice::default()
        };
        for (i, &s) in Section::ALL.iter().enumerate() {
            match self.state(i) {
                State::Off => {}
                State::On => choice.sections.push(s),
                State::Partly if s == Section::Adjustments => {
                    choice.sections.push(s);
                    choice.masks = Some(
                        self.masks
                            .iter()
                            .zip(&self.on[i])
                            .filter(|(_, on)| **on)
                            .map(|((id, _), _)| *id)
                            .collect(),
                    );
                }
                State::Partly => choice.parts.extend(
                    s.parts()
                        .zip(&self.on[i])
                        .filter(|(_, on)| **on)
                        .map(|(p, _)| p),
                ),
            }
        }
        choice
    }

    /// The list's rows, each with what it is, and how many lines the
    /// taller column takes.
    pub(crate) fn rows(&self) -> (Vec<(Target, CarryRow)>, i32) {
        let heights: Vec<usize> = Section::ALL
            .iter()
            .map(|&s| block_lines(s, self.masks.len()))
            .collect();
        let split = split_at(&heights);
        let mut rows = Vec::new();
        let mut line = [0usize; 2];
        for (i, &s) in Section::ALL.iter().enumerate() {
            let col = usize::from(i >= split);
            let state = self.state(i);
            let at = |line: usize, sub: usize, kind: i32| CarryRow {
                col: col as i32,
                line: line as i32,
                sub: sub as i32,
                kind,
                ..CarryRow::default()
            };
            rows.push((
                Target::Section(i),
                CarryRow {
                    label: s.title().into(),
                    a11y: s.title().into(),
                    state: state.number(),
                    ..at(line[col], 0, HEADING)
                },
            ));
            line[col] += 1;
            let names: Vec<String> = if s == Section::Adjustments {
                self.masks.iter().map(|(_, n)| n.clone()).collect()
            } else {
                s.parts().map(|p| p.title().to_string()).collect()
            };
            for (j, name) in names.iter().enumerate() {
                rows.push((
                    Target::Part(i, j),
                    CarryRow {
                        a11y: format!("{}: {name}", s.title()).into(),
                        label: name.into(),
                        state: if self.on[i][j] { 2 } else { 0 },
                        ..at(line[col] + j / 2, j % 2, PART)
                    },
                ));
            }
            line[col] += names.len().div_ceil(2);
            if s == Section::Adjustments {
                rows.push((
                    Target::Join,
                    CarryRow {
                        label: "".into(),
                        a11y: "".into(),
                        state: i32::from(self.join),
                        ..at(line[col], 0, JOIN)
                    },
                ));
                line[col] += 1;
            }
        }
        (rows, line[0].max(line[1]) as i32)
    }
}

/// A sheet's list as shown: the model its rows are in, kept from one
/// click to the next so a check turning over changes its row and no
/// other, and what each row is.
#[derive(Clone)]
pub(crate) struct Shown {
    pub(crate) model: Rc<VecModel<CarryRow>>,
    pub(crate) targets: Vec<Target>,
}

impl Default for Shown {
    fn default() -> Self {
        Self {
            model: Rc::new(VecModel::default()),
            targets: Vec::new(),
        }
    }
}

impl Shown {
    /// Put `checks` in the model: row by row when the list keeps its
    /// shape, whole when it does not. The lines the taller column
    /// takes.
    pub(crate) fn show(&mut self, checks: &Checks) -> i32 {
        let (rows, lines) = checks.rows();
        let (targets, rows): (Vec<Target>, Vec<CarryRow>) = rows.into_iter().unzip();
        if targets == self.targets && self.model.row_count() == rows.len() {
            for (i, row) in rows.into_iter().enumerate() {
                if self.model.row_data(i).as_ref() != Some(&row) {
                    self.model.set_row_data(i, row);
                }
            }
        } else {
            self.model.set_vec(rows);
            self.targets = targets;
        }
        lines
    }

    /// What row `i` is.
    pub(crate) fn target(&self, i: i32) -> Option<Target> {
        usize::try_from(i)
            .ok()
            .and_then(|i| self.targets.get(i))
            .copied()
    }
}

/// How a section's checks stand.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum State {
    Off,
    Partly,
    On,
}

impl State {
    /// As `CarryRow::state` has it: 0 off, 1 some, 2 on.
    fn number(self) -> i32 {
        match self {
            State::Off => 0,
            State::Partly => 1,
            State::On => 2,
        }
    }
}

/// How many checks section `s` has: one a part, one a mask, and one
/// for a section with neither.
fn slots(s: Section, masks: usize) -> usize {
    if s == Section::Adjustments {
        masks.max(1)
    } else {
        s.parts().count().max(1)
    }
}

/// How many lines a section takes in the list: its heading, its parts
/// two to a line, and for the masks the line that says how they land.
fn block_lines(s: Section, masks: usize) -> usize {
    let parts = if s == Section::Adjustments {
        masks
    } else {
        s.parts().count()
    };
    1 + parts.div_ceil(2) + usize::from(s == Section::Adjustments)
}

/// Where the second column starts: the first section of it, chosen so
/// the taller column is as short as it can be.
fn split_at(heights: &[usize]) -> usize {
    let total: usize = heights.iter().sum();
    let mut best = (usize::MAX, heights.len());
    let mut first = 0;
    for (k, h) in heights.iter().enumerate() {
        let taller = first.max(total - first);
        if taller < best.0 {
            best = (taller, k);
        }
        first += h;
    }
    best.1
}

#[cfg(test)]
mod tests {
    use super::*;
    use greycard_edit::preset::Part;

    /// The part a row's place under section `i` is.
    fn part_at(i: usize, j: usize) -> Option<Part> {
        Section::ALL[i].parts().nth(j)
    }

    fn at(s: Section) -> usize {
        Section::ALL.iter().position(|x| *x == s).unwrap()
    }

    fn masks() -> Vec<(u64, String)> {
        vec![
            (2, "Sky 2".into()),
            (5, "Face 5".into()),
            (6, "Linear 6".into()),
        ]
    }

    #[test]
    fn the_sections_all_on_are_whole_as_before() {
        let checks = Checks::new(masks(), Section::syncs_by_default);
        let choice = checks.choice();
        let want: Vec<Section> = Section::ALL
            .iter()
            .copied()
            .filter(|s| s.syncs_by_default())
            .collect();
        assert_eq!(choice.sections, want);
        assert!(choice.parts.is_empty());
        assert_eq!(choice.masks, None);
        assert_eq!(choice.masks_land, Masks::Replace);
    }

    #[test]
    fn a_part_unchecked_sends_the_rest_of_its_section_as_parts() {
        let mut checks = Checks::new(Vec::new(), |s| s == Section::Detail);
        let d = at(Section::Detail);
        assert_eq!(checks.state(d), State::On);
        // Texture off: Clarity and Dehaze go, as parts.
        checks.toggle(Target::Part(d, 0));
        assert_eq!(part_at(d, 0), Some(Part::DetailTexture));
        assert_eq!(checks.state(d), State::Partly);
        let choice = checks.choice();
        assert!(choice.sections.is_empty());
        assert_eq!(choice.parts, vec![Part::DetailClarity, Part::DetailDehaze]);
        // The heading of a section partly on turns it all on, then off.
        checks.toggle(Target::Section(d));
        assert_eq!(checks.state(d), State::On);
        assert_eq!(checks.choice().sections, vec![Section::Detail]);
        checks.toggle(Target::Section(d));
        assert_eq!(checks.state(d), State::Off);
        assert!(checks.choice().is_empty());
        // A section without parts is one check.
        let wb = at(Section::WhiteBalance);
        checks.toggle(Target::Section(wb));
        assert_eq!(checks.choice().sections, vec![Section::WhiteBalance]);
    }

    #[test]
    fn check_all_and_uncheck_all_reach_every_part_and_mask() {
        let mut checks = Checks::new(masks(), |s| s == Section::Light);
        checks.toggle(Target::Part(at(Section::Light), 2));
        checks.set_all(true);
        for i in 0..Section::ALL.len() {
            assert_eq!(checks.state(i), State::On, "{:?}", Section::ALL[i]);
        }
        let choice = checks.choice();
        assert_eq!(choice.sections, Section::ALL.to_vec());
        assert!(choice.parts.is_empty());
        assert_eq!(choice.masks, None, "every mask");
        checks.set_all(false);
        assert!(checks.choice().is_empty());
    }

    #[test]
    fn masks_are_checked_one_by_one_and_land_as_chosen() {
        let mut checks = Checks::new(masks(), |_| false);
        let m = at(Section::Adjustments);
        checks.toggle(Target::Part(m, 1));
        checks.join = true;
        let choice = checks.choice();
        assert_eq!(choice.sections, vec![Section::Adjustments]);
        assert_eq!(choice.masks, Some(vec![5]));
        assert_eq!(choice.masks_land, Masks::Join);
        // Over another frame's masks: the one of the same name stays
        // on; all on or all off stays so.
        let other = vec![(1, "Face 5".into()), (2, "Sky 9".into())];
        let moved = checks.with_masks(other.clone());
        assert_eq!(moved.choice().masks, Some(vec![1]));
        assert!(moved.join);
        checks.set_all(true);
        assert_eq!(checks.with_masks(other.clone()).state(m), State::On);
        checks.set_all(false);
        assert_eq!(checks.with_masks(other).state(m), State::Off);
        // No masks on the frame: one check, which sends the masks
        // (none, replacing) as ticking them always did.
        let mut none = Checks::new(Vec::new(), |_| false);
        none.toggle(Target::Section(m));
        assert_eq!(none.choice().sections, vec![Section::Adjustments]);
        assert_eq!(none.choice().masks, None);
    }

    #[test]
    fn the_list_is_two_columns_of_headings_with_their_parts_under() {
        let checks = Checks::new(masks(), |_| true);
        let (rows, lines) = checks.rows();
        // Every section's heading once, every part, every mask and
        // the masks' line.
        let headings = rows
            .iter()
            .filter(|(t, _)| matches!(t, Target::Section(_)))
            .count();
        assert_eq!(headings, Section::ALL.len());
        let parts = rows
            .iter()
            .filter(|(t, _)| matches!(t, Target::Part(..)))
            .count();
        assert_eq!(parts, Part::ALL.len() + 3);
        assert_eq!(rows.iter().filter(|(t, _)| *t == Target::Join).count(), 1);
        // No two rows on one spot, and nothing past the last line.
        let mut spots: Vec<(i32, i32, i32)> =
            rows.iter().map(|(_, r)| (r.col, r.line, r.sub)).collect();
        spots.sort();
        spots.dedup();
        assert_eq!(spots.len(), rows.len());
        assert!(rows.iter().all(|(_, r)| r.line < lines));
        // Both columns used, each a run of whole sections.
        assert!(rows.iter().any(|(_, r)| r.col == 1));
        let col_of = |i: usize| {
            rows.iter()
                .find(|(t, _)| *t == Target::Section(i))
                .unwrap()
                .1
                .col
        };
        let cols: Vec<i32> = (0..Section::ALL.len()).map(col_of).collect();
        assert!(cols.windows(2).all(|w| w[0] <= w[1]), "{cols:?}");
        // A part's label is its own; its name for a screen reader
        // says the section too.
        let d = at(Section::Detail);
        let dehaze = &rows
            .iter()
            .find(|(t, _)| *t == Target::Part(d, 2))
            .unwrap()
            .1;
        assert_eq!(dehaze.label, "Dehaze");
        assert_eq!(dehaze.a11y, "Detail: Dehaze");
        let masks_heading = &rows
            .iter()
            .find(|(t, _)| *t == Target::Section(at(Section::Adjustments)))
            .unwrap()
            .1;
        assert_eq!(masks_heading.label, "Masks");
    }

    #[test]
    fn a_mask_without_a_name_is_named_as_the_masks_tab_names_it() {
        let mut edit = Edit::default();
        edit.adjustments.push(greycard_edit::Adjustment {
            id: 3,
            name: String::new(),
            ..Default::default()
        });
        edit.adjustments.push(greycard_edit::Adjustment {
            id: 4,
            name: "Sky 4".into(),
            ..Default::default()
        });
        assert_eq!(
            masks_of(&edit),
            vec![(3, "Mask 3".to_string()), (4, "Sky 4".to_string())]
        );
    }
}
