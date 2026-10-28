//! The open frame, its develop and the tools on it: [`Develop`].

use crate::*;

/// The develop side: the open frame and its edit, the develop out and
/// the picture on the GPU, the viewport and its zoom, the tools in hand
/// on the picture (crop, masks, parts, retouch), the panel's lists
/// (looks, profiles, presets, models) and the display tables.
pub(crate) struct Develop {
    /// The edit the worker is developing, or last developed.
    pub(crate) edit: Edit,
    /// The developed image waiting to go to the GPU, if any.
    pub(crate) pending: Option<Landed>,
    /// The white balance of the image on the GPU.
    pub(crate) base_white: Option<WhiteBase>,
    /// The open frame and its profile, for the white balance preview.
    pub(crate) frame: Option<(Arc<RawFrame>, Box<CameraProfile>)>,
    /// The file the panel is for and what is known of its mosaic, set
    /// by `browser::panel_frame` wherever the panel's file changes and
    /// brought up to date by `browser::learn_mosaic` as the worker
    /// finds out; read through [`State::open_mosaic`].
    pub(crate) panel_mosaic: Option<(PathBuf, crate::panel::browser::OpenMosaic)>,
    /// What the session has learned of files' mosaics from the worker:
    /// its read ahead of a decode, and the decoded frames.
    pub(crate) mosaics: HashMap<PathBuf, crate::panel::browser::OpenMosaic>,
    /// The kind of the picture on the GPU, set where its develop is
    /// uploaded: what the viewport finishes it as. Until the open
    /// frame's develop lands the picture there is the last frame's,
    /// and drawn as the open frame's kind a picture already rendered
    /// would take a raw's baseline and shoulder, or a raw's scene go
    /// to the screen without them.
    pub(crate) drawn_kind: finish::Source,
    /// The open raw's own white as a temperature and tint, as its
    /// decode read it: what the white balance sliders show under As
    /// shot. None until the decode lands, and for a picture.
    pub(crate) as_shot_white: Option<(f32, f32)>,
    /// The last preview matrix, keyed by what it was computed for.
    pub(crate) white_cache: Option<(WhiteKey, Matrix3)>,
    /// What the picture on screen's masks made their white balances
    /// from, kept from its own develop: a picture held up while the
    /// next frame's develop is on the way keeps its masks' whites,
    /// though the frame and its profile are the next one's by then
    /// (`panel::color::local_white_shift`).
    pub(crate) shown_white: Option<greycard_edit::WhiteShift>,
    /// Holds a develop back until the sliders rest.
    pub(crate) debounce: slint::Timer,
    /// Holds a sidecar write back until the sliders rest.
    pub(crate) save_timer: slint::Timer,
    /// The image the viewport shows, in pixels, for fit and zoom.
    pub(crate) image_size: (u32, u32),
    /// Display pixels per image pixel; 0 means fit to the view.
    pub(crate) zoom: f32,
    /// The mat: a band of the canvas around the fitted picture, by
    /// its key, kept in the settings.
    pub(crate) mat: bool,
    /// The image pixel at the view's center.
    pub(crate) center: (f32, f32),
    pub(crate) renderer: Option<Renderer>,
    /// The viewport's adapter as the log names it, for a report; none
    /// until the rendering setup has run.
    pub(crate) gpu: Option<String>,
    /// Where the window draws on a software adapter and the engine's
    /// ops run on the CPU, the words every develop's line carries after
    /// its time to say so (`adapter::Lines`).
    pub(crate) software_tail: Option<&'static str>,
    /// The last scope bins: the histogram, the backdrop of the curve
    /// editor, and after it whatever `scope` asked for.
    pub(crate) bins: Option<Vec<u32>>,
    /// Which scope the panel shows.
    pub(crate) scope: scope::Scope,
    /// The pointer on the view, logical pixels, while it is there:
    /// the color under it is marked on the scope.
    pub(crate) hover: Option<(f32, f32)>,
    /// What the scope shows of the color under the pointer, and on
    /// which scope: a move that reads the same leaves the marks be,
    /// rather than restart their glide to where they already are.
    pub(crate) hover_shown: Option<(scope::Scope, scope::Reading)>,
    /// The curve point under the pointer while it is held.
    pub(crate) curve_drag: Option<usize>,
    /// The developed picture's size, the source of the geometry.
    /// Cleared on every select: until this frame's develop lands
    /// nothing may take the last frame's shape for this one's.
    pub(crate) source_size: (u32, u32),
    /// The size of the developed picture on the GPU, which is what
    /// the viewport draws while a develop is pending and no camera
    /// picture stands in. Not cleared by a select: the picture stays
    /// on screen until another replaces it.
    pub(crate) shown_size: (u32, u32),
    /// The frame that picture is a develop of and the quarter turns
    /// it was developed at. A turn pressed since is drawn at once by
    /// turning that picture in the viewport (`lagging_turn`) rather
    /// than waiting a second for the develop at the new turn.
    pub(crate) shown_turn: Option<(usize, u8)>,
    /// When the open frame was last turned, for the log's line on the
    /// first frame drawn after it.
    pub(crate) turn_pressed: Option<std::time::Instant>,
    /// The crop as it was when a handle was pressed.
    pub(crate) crop_drag: Option<Crop>,
    /// The anchor of a crop being drawn fresh from a press outside
    /// today's, fractions of the leveled plane, [`Crop`]'s own units.
    pub(crate) crop_draw: Option<(f32, f32)>,
    /// The aspect the crop opens under on a frame with no crop of its
    /// own (the Settings sheet's choice, `settings.crop_start`), and
    /// the aspect last picked from the list, which "Last used" opens
    /// under (`settings.crop_last`, as `panel::crop::aspect_key`
    /// spells it).
    pub(crate) crop_start: crate::panel::crop::Start,
    pub(crate) crop_last: String,
    /// A frame opened with crop mode on: the starting aspect is laid
    /// on it once, when its develop lands and its shape is known.
    pub(crate) crop_start_due: bool,
    /// What a Fill drawn next asks to be made by, as last chosen
    /// (`settings.fill_choice`); None until one is, for the machine's
    /// default (`ai::FillHaves::default_choice`).
    pub(crate) fill_choice: Option<greycard_edit::retouch::FillChoice>,
    /// Which look the panel edits: an adjustment by index, or the
    /// global one. The panel is the truth for that look; `edit` is
    /// for everything else, the other adjustments included.
    pub(crate) target: Option<usize>,
    /// The id of a mask "New mask" made that has not yet taken a
    /// shape: still nameless work in progress, so a shape drawn into
    /// it and abandoned takes the mask with it. Cleared the moment a
    /// shape lands in any mask, so a mask once emptied by deleting
    /// its shapes, or the mask "New mask" made once it has moved on
    /// to something else, is never taken by mistake.
    pub(crate) fresh_mask: Option<u64>,
    /// A mask shape being drawn in the viewport.
    pub(crate) placing: Option<Placing>,
    /// A part's person being picked on the picture.
    pub(crate) choosing: Option<panel::parts::Choosing>,
    /// A part chosen from the menu, waiting on the people (or its
    /// model's download).
    pub(crate) part_pending: Option<panel::parts::Pending>,
    /// The Landscape button pressed, waiting on what the model finds
    /// (or on its model's download), for the mask it was pressed on.
    pub(crate) landscape_pending: Option<panel::landscape::Pending>,
    /// What it found is on its menu: the mask a pick from it goes to.
    pub(crate) landscape_offer: Option<panel::landscape::Pending>,
    /// The open picture's placed parts that ask which person they are
    /// of, by component.
    pub(crate) part_asks: HashMap<Key, ai::Ask>,
    /// The asks put down with Escape on the open picture: not brought
    /// back by returning to the Masks tab, only by choosing their mask.
    pub(crate) part_dismissed: std::collections::HashSet<Key>,
    /// The frames a paste or a sync took People shapes to, a person
    /// picked on one of them offered for the rest, and the run that
    /// looks for them there ("use this person on all").
    pub(crate) people_all: panel::parts_all::All,
    /// A dropper's press, while the pointer is down.
    pub(crate) picking: Option<Picking>,
    /// A shape's handle being dragged.
    pub(crate) mask_drag: Option<MaskDrag>,
    /// The brushes' rasters, by adjustment id and component index,
    /// kept up with their strokes a stamp at a time.
    pub(crate) rasters: HashMap<(u64, usize), Arc<Raster>>,
    /// The repair tool in hand, the stroke under way (a patch index),
    /// and a patch's handle being dragged: the patch as grabbed, the
    /// handle, and where it was on the view.
    pub(crate) retouching: Option<RetouchMethod>,
    pub(crate) patch_stroke: Option<usize>,
    /// The engine each Fill patch of the open file was made by, by
    /// patch id, as the last develop to apply them said: a patch the
    /// generative remove made has "Another" on its row.
    pub(crate) fill_engines: HashMap<u64, crate::ai::Engine>,
    pub(crate) patch_drag: Option<(Patch, usize, (f32, f32))>,
    /// The patch handles' positions on the view, one model for the
    /// life of the window, as the masks' are.
    pub(crate) patch_handles: Rc<VecModel<Pt>>,
    /// The chosen patch's rings in the picture's units, for the
    /// patch they were made for, so a frame maps them and no more.
    pub(crate) patch_shape: Option<PatchShape>,
    /// The perspective guide's first stroke, while it waits for the
    /// second.
    pub(crate) guiding: Guiding,
    /// That stroke's two ends where they are in the view now, for the
    /// overlay to draw.
    pub(crate) guide_kept: Rc<VecModel<Pt>>,
    /// How each frame stands in its own file — its orientation tag
    /// and the size a develop comes out at — read once and kept. No
    /// pixel is decoded for it, and nothing asks until a turn is
    /// pressed or an XMP is written, so a folder of frames nobody
    /// turns and nobody shares never pays for it.
    pub(crate) stances: HashMap<PathBuf, greycard_core::decode::Stance>,
    /// The learned masks' rasters as the worker made them, with the
    /// shape each was made for.
    pub(crate) learned: HashMap<Key, (Shape, Arc<Raster>)>,
    /// The shapes asked of the worker and not yet answered.
    pub(crate) asked: HashMap<Key, Shape>,
    /// The shapes the worker answered by failing (no runtime here, a
    /// provider that would not run it), with the file they were asked
    /// of. They stay in `asked`, so the same shape is not asked again
    /// and again, but count as answered for a capture's wait, which
    /// would otherwise wait on them for ever.
    pub(crate) masks_failed: HashMap<Key, (Shape, Option<PathBuf>)>,
    /// The file `learned` and `asked` are of: the one last sent to the
    /// worker to open (`panel::mask::learned_for`).
    pub(crate) learned_of: Option<PathBuf>,
    /// The model store, to know what is there.
    pub(crate) store: Option<greycard_ai::Store>,
    /// What the lens database made of the open file.
    pub(crate) lens: LensReport,
    /// The camera profiles the profile directory holds, and the open
    /// file's camera, for the panel's list and its warning.
    pub(crate) profiles: Vec<greycard_edit::camera::Entry>,
    pub(crate) camera: (String, String),
    /// The other names the open file's camera is sold under.
    pub(crate) other_names: Vec<String>,
    /// The look tables the look directory holds, for the panel's list.
    pub(crate) looks: Vec<greycard_edit::look::Entry>,
    /// The look the viewport draws through, and the section it was
    /// resolved for. Kept here rather than read on every frame: the
    /// table is on the GPU already, and reading the section is a stat
    /// of the file. `None` for the section asks for it again, which
    /// is what a re-listing sets it to.
    /// It is resolved through the gate for the picture's display
    /// curve, so the curve is part of what it was resolved for: a
    /// fitted table is no look under another curve.
    pub(crate) look: Option<greycard_core::lut::Look>,
    pub(crate) look_for: Option<(greycard_edit::LookLut, greycard_edit::DisplayCurve)>,
    /// The look list's groups the user has opened (true) or folded,
    /// by key (`greycard_edit::look::Listing::open`).
    pub(crate) look_groups_open: std::collections::BTreeMap<String, bool>,
    /// The look list's row under the pointer and the look previewed
    /// from it, drawn in place of `look` while it stands.
    pub(crate) look_hover: RefCell<panel::look_preview::Hover>,
    /// The profile the white balance is solved through, and the name
    /// it was resolved for: the edit's DCP when it names one that
    /// reads, else the file's own. The develop converts temperature
    /// and tint through the chosen profile's matrices, so the panel's
    /// preview and the neutral dropper have to use the same ones or
    /// they answer for a camera the picture was not developed as.
    pub(crate) white_profile: Option<(String, CameraProfile)>,
    /// The handles' positions on the view, one model for the life of
    /// the window: replaced, the handle under the pointer would be
    /// made anew mid-drag and lose the press.
    pub(crate) mask_handles: Rc<VecModel<Pt>>,
    /// An object's boxes and picks on the view.
    pub(crate) mask_boxes: Rc<VecModel<MaskBox>>,
    pub(crate) mask_picks: Rc<VecModel<MaskPick>>,
    /// The people a part may be of, outlined while one is picked.
    pub(crate) person_boxes: Rc<VecModel<PersonBox>>,
    /// The displays the system's color manager knows, primary first, for the panel's
    /// "System" choice.
    pub(crate) monitors: Vec<display::Monitor>,
    /// What the display table on the renderer was built for: the
    /// output space, the proof if one is on, and the monitor.
    pub(crate) lut_for: Option<(
        export::Space,
        Option<display::Proof>,
        display::MonitorProfile,
    )>,
    /// What the encoded path's table was built for: the monitor
    /// alone, since the camera's JPEG is sRGB and takes no proof.
    pub(crate) encoded_lut_for: Option<display::MonitorProfile>,
    /// The presets, as the store lists them, and the store.
    pub(crate) presets: Vec<Entry>,
    pub(crate) preset_store: Option<preset::Store>,
    /// A preset click's own words ("Muted Slide onto 3 frames"),
    /// waiting for the current frame's develop (its generation) to
    /// land: the develop's own line would otherwise stand alone,
    /// since it is set again once that develop is on screen.
    pub(crate) status_after_develop: Option<(u64, String)>,
    /// The hint the tool in hand put on the status line, which
    /// putting the tool down takes off while the line still says it.
    pub(crate) hint: Option<String>,
    /// A state shown in the viewport in place of the panel's while a
    /// history or snapshot row is under the pointer, and the status
    /// line it covers meanwhile.
    pub(crate) peek: Option<Edit>,
    /// The edit of the picture still on the GPU after another file was
    /// chosen: the viewport shows that picture under this look until
    /// the new file's develop lands, rather than the new look over the
    /// old picture.
    pub(crate) held: Option<Edit>,
    pub(crate) status_kept: Option<slint::SharedString>,
    /// The view (the frame's size and the center) before a peek, put
    /// back after it: a hover over a row is not to lose a zoomed place.
    pub(crate) view_kept: Option<((u32, u32), (f32, f32))>,
    /// The mask toggle as it was before a tool went in hand; the mask
    /// shows while a shape is placed, and the toggle comes back after,
    /// unless it was thrown meanwhile.
    pub(crate) show_mask_kept: Option<bool>,
    /// The look as the panel last showed it, read back from the panel
    /// where it is written (`show_edit`) and where it is changed
    /// (`mask::look_edited`), so the next change can tell a look slider
    /// from a shape control.
    pub(crate) look_seen: RefCell<Option<greycard_edit::Look>>,
    /// The detail planes the shown develop came with: a mask's Detail
    /// moved to ask for one it lacks develops again
    /// (`panel::mask::detail_asks`).
    pub(crate) planes_wants: greycard_edit::DetailWants,
    /// The planes a develop has been asked for by a mask's Detail and
    /// has not yet brought: none asked for twice while one is on its
    /// way. Cleared when a develop lands.
    pub(crate) planes_asked: greycard_edit::DetailWants,
    pub(crate) generation: u64,
    /// What the worker was last told of the open frame's neighbors, for
    /// them to be sent again once when what they are moves
    /// (`browser::neighbors_moved`).
    pub(crate) neighbors_sent: Option<crate::panel::browser::NeighborsSent>,
}
