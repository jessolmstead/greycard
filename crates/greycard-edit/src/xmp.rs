//! The meta section in an XMP sidecar, for the tools that read one.
//!
//! The `.gcd` is still the truth (§72, §117): one file beside the
//! frame holding the edit, its history and the meta together. This
//! module is the interop layer over the meta half of it, so a folder
//! shared with Lightroom, Bridge, darktable, digiKam or exiftool
//! shows the same stars, the same label and the same keywords.
//!
//! **The fields.** `xmp:Rating` (0 to 5), `xmp:Label` (the color's
//! English name, capitalized, as Lightroom writes it), `dc:subject`
//! (the keywords, an `rdf:Bag`), `dc:title` and `dc:description`
//! (`rdf:Alt` with an `x-default`). The pick flag has no standard
//! field — Adobe never gave the pick one, and Lightroom does not
//! export a pick to XMP at all — so it goes under greycard's own
//! namespace as `greycard:Flag`, which every other tool will ignore
//! and this one reads back.
//!
//! **The orientation.** `tiff:Orientation` is the EXIF tag by
//! another name, 1 to 8, and it is the one field here that is not
//! part of the meta: the `.gcd` keeps a frame's quarter turns
//! ([`crate::Sidecar::turn`]) as turns *on top of* the camera's own
//! tag, and the XMP has to carry what the frame shows as, which is
//! the two composed. So a write takes the camera's tag and steps it
//! along by the turn ([`greycard_core::raw::Orientation::turned`]),
//! and a read takes the value back apart: the quarter turns that
//! carry the camera's tag to the packet's, or none when no quarter
//! turn does, which is a packet claiming a mirror the frame has not
//! got. Storing the turn rather than the composed value is what
//! keeps an edit readable when a later build reads the file's tag
//! differently, and what makes a turn mean the same thing on the
//! frame's own JPEG, whose tag is the same one.
//!
//! The camera's tag is the caller's to supply, since only the
//! caller knows the frame: with none, the property is neither
//! written nor believed, and whatever the file says is left where
//! it is.
//!
//! **The reject.** Bridge, darktable and exiftool spell a reject
//! `xmp:Rating` of -1, and that convention is honored both ways: a
//! frame with no stars and a reject flag writes -1, and a -1 read
//! back is a reject. A frame that is *both* rejected and rated keeps
//! its stars in `xmp:Rating`, since they are the field's own
//! meaning, and its reject in `greycard:Flag`; nothing is lost
//! either way round.
//!
//! **Everything else in the file is kept.** [`write`] does not
//! rebuild a packet: it finds the properties above, rewrites those
//! and splices the rest of the bytes through untouched, so
//! Lightroom's `crs:` block, darktable's history and an `xmpMM:`
//! chain survive a star pressed here. A property already in the
//! file keeps its form — an attribute stays an attribute, and only
//! its value moves — so changing a rating on a Lightroom sidecar is
//! a one-character diff. The one property that is *removed* rather
//! than kept is `lr:hierarchicalSubject`, and only when the keywords
//! change: it is Lightroom's parallel copy of `dc:subject` with the
//! paths spelled out, and a stale copy beside a fresh flat list
//! would contradict it. Lightroom rebuilds it from `dc:subject` on
//! the next read.
//!
//! **What a packet says, and what it is silent about, are different
//! things.** [`read`] reports which of the properties were there, so
//! taking a packet's word ([`Packet::over`]) moves only the fields
//! it carried and leaves the rest of the `.gcd`'s meta alone. A
//! property that is present and empty — an empty `rdf:Bag` — does
//! clear, since that is a statement rather than a silence.
//!
//! **Not written.** A file this build cannot parse is left alone:
//! the XMP is lost, never the `.gcd`, because a packet we cannot
//! read is one we cannot safely rewrite either. And a frame with
//! nothing to say never gets an XMP made for it.

use std::ops::Range;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use greycard_core::raw::Orientation;

use crate::meta::{Flag, Label, Meta, STARS};
use crate::{Error, Result};

/// The XMP basic namespace: `xmp:Rating` and `xmp:Label`.
const XMP: &str = "http://ns.adobe.com/xap/1.0/";
/// Dublin Core: `dc:subject`, `dc:title`, `dc:description`.
const DC: &str = "http://purl.org/dc/elements/1.1/";
const RDF: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";
/// Lightroom's own, for the keyword paths it keeps beside the flat
/// list. Nothing here is read from it; it is taken out when the flat
/// list changes, so the two cannot disagree.
const LR: &str = "http://ns.adobe.com/lightroom/1.0/";
/// The TIFF namespace, for `tiff:Orientation`: the EXIF tag as XMP
/// spells it, which is what Lightroom, Bridge and exiftool read.
const TIFF: &str = "http://ns.adobe.com/tiff/1.0/";
const HIERARCHICAL: &str = "hierarchicalSubject";
/// Greycard's own, shared with the packet an export carries (§51).
/// The pick flag lives here because standard XMP has nowhere for it.
pub const GREYCARD: &str = greycard_core::exif::NAMESPACE;

/// One property this module owns. Everything else in a packet is
/// somebody else's and is never touched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Prop {
    Rating,
    Label,
    Flag,
    Subject,
    Title,
    Description,
    Orientation,
}

impl Prop {
    const ALL: [Prop; 7] = [
        Prop::Rating,
        Prop::Label,
        Prop::Flag,
        Prop::Subject,
        Prop::Title,
        Prop::Description,
        Prop::Orientation,
    ];

    fn ns(self) -> &'static str {
        match self {
            Prop::Rating | Prop::Label => XMP,
            Prop::Flag => GREYCARD,
            Prop::Orientation => TIFF,
            Prop::Subject | Prop::Title | Prop::Description => DC,
        }
    }

    /// The prefix to use when the file binds none for the namespace.
    fn prefix(self) -> &'static str {
        match self.ns() {
            XMP => "xmp",
            GREYCARD => "greycard",
            TIFF => "tiff",
            _ => "dc",
        }
    }

    fn name(self) -> &'static str {
        match self {
            Prop::Rating => "Rating",
            Prop::Label => "Label",
            Prop::Flag => "Flag",
            Prop::Subject => "subject",
            Prop::Title => "title",
            Prop::Description => "description",
            Prop::Orientation => "Orientation",
        }
    }

    /// Whether the value is one word, so it can sit in an attribute.
    /// The other three are `rdf:Bag`s and `rdf:Alt`s and cannot.
    fn scalar(self) -> bool {
        matches!(
            self,
            Prop::Rating | Prop::Label | Prop::Flag | Prop::Orientation
        )
    }

    fn index(self) -> usize {
        Prop::ALL.iter().position(|p| *p == self).unwrap_or(0)
    }

    fn bit(self) -> u8 {
        1 << self.index()
    }
}

/// How a frame stands: the tag the camera wrote, and the quarter
/// turns clockwise this build has put on top of it (§117's rule, on
/// the sidecar beside the edit).
///
/// `tiff:Orientation` is the two composed, which is the value every
/// other tool expects to read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Turn {
    pub camera: Orientation,
    /// 0 to 3.
    pub quarters: u8,
}

impl Turn {
    pub fn new(camera: Orientation, quarters: u8) -> Self {
        Self {
            camera,
            quarters: quarters % 4,
        }
    }

    /// What the frame shows as: the tag a camera that had held the
    /// body this way up would have written.
    pub fn shown(self) -> Orientation {
        self.camera.turned(i32::from(self.quarters))
    }
}

/// Which property a namespace and a name are, if either is ours.
fn prop_of(ns: Option<&str>, name: &str) -> Option<Prop> {
    Prop::ALL
        .into_iter()
        .find(|p| ns == Some(p.ns()) && p.name() == name)
}

/// Whether a namespace and a name are Lightroom's keyword paths.
fn is_hierarchical(ns: Option<&str>, name: &str) -> bool {
    ns == Some(LR) && name == HIERARCHICAL
}

/// The scalar a property should be written as, or None when the meta
/// says nothing under it.
fn scalar_value(prop: Prop, meta: &Meta, turn: Option<Turn>) -> Option<String> {
    match prop {
        // Always written when the caller knows the frame's tag, turn
        // or no turn: it is the field's own meaning rather than
        // something this build has to say, and leaving a stale one
        // behind after a turn is taken back would be worse than
        // writing the plain truth every time.
        Prop::Orientation => turn.map(|t| t.shown().to_exif().to_string()),
        // The stars when there are stars; -1, the reject every other
        // tool knows, when there are none and the frame is rejected.
        Prop::Rating => match (meta.rating, meta.flag) {
            (0, Flag::Reject) => Some("-1".into()),
            (0, _) => None,
            (stars, _) => Some(stars.to_string()),
        },
        Prop::Label => (meta.label != Label::None).then(|| meta.label.name().to_string()),
        Prop::Flag => match meta.flag {
            Flag::None => None,
            Flag::Pick => Some("pick".into()),
            Flag::Reject => Some("reject".into()),
        },
        _ => None,
    }
}

// -- reading ---------------------------------------------------------

/// What a packet said, and which of the six properties it said
/// anything about at all.
///
/// The difference matters: a tool that adds one property of its own
/// to a sidecar has not thereby cleared the five it never mentions.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Packet {
    /// The fields as the packet gives them; the ones it was silent
    /// about are at their defaults and mean nothing.
    pub meta: Meta,
    /// What `tiff:Orientation` said, when it said anything: the
    /// frame's whole orientation, camera tag and turn together.
    /// [`Packet::turn_over`] takes the turn back out of it.
    orientation: Orientation,
    present: u8,
}

impl Packet {
    fn has(&self, prop: Prop) -> bool {
        self.present & prop.bit() != 0
    }

    /// The quarter turns this packet asks for on a frame whose
    /// camera tag is `camera`, when it said anything about the
    /// orientation at all and a quarter turn can get there.
    ///
    /// None when the packet was silent, and none when the value it
    /// carries is the frame mirrored: a mirror is not a turn, and
    /// this build would rather leave the sidecar's turn where it is
    /// than pretend a flip is a quarter of one.
    pub fn turn_over(&self, camera: Orientation) -> Option<u8> {
        self.has(Prop::Orientation)
            .then(|| self.orientation.turns_from(camera))
            .flatten()
    }

    fn mark(&mut self, prop: Prop) {
        self.present |= prop.bit();
    }

    /// Whether the packet carried any of the six at all.
    pub fn says_anything(&self) -> bool {
        self.present != 0
    }

    /// This packet's word laid over `base`: the fields it carried
    /// replace `base`'s, and the ones it was silent about keep
    /// theirs.
    pub fn over(&self, base: &Meta) -> Meta {
        let mut out = base.clone();
        if self.has(Prop::Rating) {
            out.rating = self.meta.rating;
        }
        if self.has(Prop::Label) {
            out.label = self.meta.label;
        }
        if self.has(Prop::Flag) {
            out.flag = self.meta.flag;
        }
        if self.has(Prop::Subject) {
            out.keywords.clone_from(&self.meta.keywords);
        }
        if self.has(Prop::Title) {
            out.title.clone_from(&self.meta.title);
        }
        if self.has(Prop::Description) {
            out.caption.clone_from(&self.meta.caption);
        }
        out
    }
}

/// The meta an XMP packet holds. Every field falls back to its
/// default, as the `.gcd`'s does (§117): a label spelled a way this
/// build does not know costs that field and nothing else.
pub fn read(xml: &str) -> Result<Packet> {
    let doc = roxmltree::Document::parse(xml).map_err(|e| Error::Xmp(e.to_string()))?;
    let mut packet = Packet::default();
    // `greycard:Flag` is the whole truth about the flag, so a -1
    // rating does not set it back to a reject once it is read.
    let mut flag_is_ours = false;
    for desc in descriptions(&doc) {
        for a in desc.attributes() {
            if let Some(prop) = prop_of(a.namespace(), a.name()) {
                put(&mut packet, prop, a.value(), &mut flag_is_ours);
            }
        }
        for child in desc.children().filter(|n| n.is_element()) {
            let tag = child.tag_name();
            let Some(prop) = prop_of(tag.namespace(), tag.name()) else {
                continue;
            };
            match prop {
                // Present and empty clears: an empty `rdf:Bag` is a
                // tool saying there are no keywords, not saying
                // nothing.
                Prop::Subject => {
                    packet.meta.set_keywords(items(&child));
                    packet.mark(prop);
                }
                Prop::Title | Prop::Description => {
                    put(&mut packet, prop, default_text(&child), &mut flag_is_ours);
                }
                _ => {
                    if let Some(text) = child.text() {
                        put(&mut packet, prop, text, &mut flag_is_ours);
                    }
                }
            }
        }
    }
    Ok(packet)
}

/// One value into the packet, read loosely: what makes no sense here
/// is not counted as having been said.
fn put(packet: &mut Packet, prop: Prop, value: &str, flag_is_ours: &mut bool) {
    let meta = &mut packet.meta;
    let trimmed = value.trim();
    match prop {
        // Read wide, as the `.gcd`'s rating is: a whole number, a
        // "3.0", or a thousand, all land somewhere sensible.
        // A rating that is not a number at all is not a rating that
        // was said, so nothing is taken from it either.
        Prop::Rating => {
            if let Ok(n) = trimmed.parse::<f64>() {
                let n = n.round();
                // -1 is Bridge's, darktable's and exiftool's reject,
                // and it says two things at once.
                if n < 0.0 {
                    meta.rating = 0;
                    if !*flag_is_ours {
                        meta.flag = Flag::Reject;
                    }
                    packet.mark(Prop::Flag);
                } else {
                    meta.set_rating(n.min(STARS as f64) as u8);
                }
                packet.mark(Prop::Rating);
            }
        }
        Prop::Label => {
            meta.label = Label::ALL
                .into_iter()
                .find(|l| l.name().eq_ignore_ascii_case(trimmed))
                .unwrap_or_default();
            packet.mark(prop);
        }
        Prop::Flag => {
            meta.flag = match trimmed.to_ascii_lowercase().as_str() {
                "pick" => Flag::Pick,
                "reject" => Flag::Reject,
                _ => Flag::None,
            };
            *flag_is_ours = true;
            packet.mark(prop);
        }
        // The trimming of these three is Meta's, not this module's,
        // so that a padded caption reads the same from either file
        // and does not churn a save.
        Prop::Subject => {
            meta.set_keywords(vec![value.to_string()]);
            packet.mark(prop);
        }
        Prop::Title => {
            meta.set_title(value);
            packet.mark(prop);
        }
        Prop::Description => {
            meta.set_caption(value);
            packet.mark(prop);
        }
        // 1 to 8, the EXIF tag. Anything else is not an orientation
        // that was said, so nothing is taken from it.
        Prop::Orientation => {
            if let Some(o) = trimmed.parse::<u16>().ok().and_then(Orientation::from_exif) {
                packet.orientation = o;
                packet.mark(prop);
            }
        }
    }
}

fn descriptions<'a, 'input: 'a>(
    doc: &'a roxmltree::Document<'input>,
) -> impl Iterator<Item = roxmltree::Node<'a, 'input>> {
    doc.descendants().filter(|n| {
        n.is_element()
            && n.tag_name().namespace() == Some(RDF)
            && n.tag_name().name() == "Description"
    })
}

/// The `rdf:li` texts under a property, whichever container it uses.
/// A property written as bare text is one item, which is what a
/// hand-edited file tends to hold.
fn items(node: &roxmltree::Node<'_, '_>) -> Vec<String> {
    let Some(container) = node
        .children()
        .find(|n| n.is_element() && n.tag_name().namespace() == Some(RDF))
    else {
        return node
            .text()
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(|t| vec![t.to_string()])
            .unwrap_or_default();
    };
    container
        .children()
        .filter(|n| n.is_element() && n.tag_name().name() == "li")
        .filter_map(|n| n.text())
        .map(str::to_string)
        .collect()
}

/// The `x-default` of an `rdf:Alt`, or the first item it has.
fn default_text<'a, 'input: 'a>(node: &roxmltree::Node<'a, 'input>) -> &'a str {
    let alt = node
        .children()
        .find(|n| n.is_element() && n.tag_name().namespace() == Some(RDF));
    let Some(alt) = alt else {
        return node.text().unwrap_or_default();
    };
    let li = |want: Option<&str>| {
        alt.children()
            .filter(|n| n.is_element() && n.tag_name().name() == "li")
            .find(|n| {
                want.is_none_or(|w| {
                    n.attribute(("http://www.w3.org/XML/1998/namespace", "lang")) == Some(w)
                })
            })
            .map(|n| n.text().unwrap_or_default())
    };
    li(Some("x-default"))
        .or_else(|| li(None))
        .unwrap_or_default()
}

// -- writing ---------------------------------------------------------

/// `meta` put into `xml`, with every byte that is not one of this
/// module's properties left exactly as it was.
///
/// Errors when the packet will not parse or holds no
/// `rdf:Description`: a file that cannot be read cannot be rewritten
/// without losing what is in it, so it is left alone instead.
pub fn write(xml: &str, meta: &Meta, turn: Option<Turn>) -> Result<String> {
    let doc = roxmltree::Document::parse(xml).map_err(|e| Error::Xmp(e.to_string()))?;
    let descs: Vec<_> = descriptions(&doc).collect();
    // The one that already carries a property of ours, so a second
    // description with a `crs:` block in it is not where a rating
    // lands.
    let target = descs
        .iter()
        .find(|d| ours(d))
        .or_else(|| descs.first())
        .ok_or_else(|| Error::Xmp("no rdf:Description in the packet".into()))?;

    let mut splices: Vec<(Range<usize>, String)> = Vec::new();
    let mut done = [false; Prop::ALL.len()];
    let mut kept: Vec<usize> = Vec::new();
    // A caller that does not know the frame's tag has nothing to say
    // about which way up it is, so whatever the file says stands.
    if turn.is_none() {
        done[Prop::Orientation.index()] = true;
    }

    // What the target already has and can keep: a scalar's value
    // moves and its form does not, and a property that already says
    // what the meta says is not touched at all. That is what makes a
    // rating pressed on a Lightroom sidecar a one-character diff.
    for a in target.attributes() {
        let Some(prop) = prop_of(a.namespace(), a.name()) else {
            continue;
        };
        if prop.scalar()
            && !done[prop.index()]
            && let Some(value) = scalar_value(prop, meta, turn)
        {
            splices.push((a.range_value(), escape(&value, true)));
            done[prop.index()] = true;
            kept.push(a.range().start);
        }
    }
    for child in target.children().filter(|n| n.is_element()) {
        let tag = child.tag_name();
        let Some(prop) = prop_of(tag.namespace(), tag.name()) else {
            continue;
        };
        if done[prop.index()] {
            continue;
        }
        let text = child.first_child().filter(|n| n.is_text());
        if !prop.scalar() {
            if unchanged(prop, &child, meta) {
                done[prop.index()] = true;
                kept.push(child.range().start);
            }
        } else if let Some(text) = text
            && child.children().count() == 1
            && let Some(value) = scalar_value(prop, meta, turn)
        {
            splices.push((text.range(), escape(&value, false)));
            done[prop.index()] = true;
            kept.push(child.range().start);
        }
    }
    // Lightroom's keyword paths go when the flat list moves, and
    // stay when it does not: a `Places|Scotland|Skye` left beside a
    // list that no longer holds it is a contradiction, and the one
    // that Lightroom believes.
    let keywords_moved = !done[Prop::Subject.index()];

    // Everything else of ours goes, wherever in the packet it sits.
    let mine = |ns: Option<&str>, name: &str| match prop_of(ns, name) {
        Some(Prop::Orientation) => turn.is_some(),
        Some(_) => true,
        None => false,
    };
    for desc in &descs {
        for a in desc.attributes() {
            let ours = mine(a.namespace(), a.name())
                || (keywords_moved && is_hierarchical(a.namespace(), a.name()));
            if ours && !kept.contains(&a.range().start) {
                splices.push((back_over_space(xml, a.range()), String::new()));
            }
        }
        for child in desc.children().filter(|n| n.is_element()) {
            let tag = child.tag_name();
            let ours = mine(tag.namespace(), tag.name())
                || (keywords_moved && is_hierarchical(tag.namespace(), tag.name()));
            if ours && !kept.contains(&child.range().start) {
                splices.push((back_over_line(xml, child.range()), String::new()));
            }
        }
    }

    // What is left to write goes in as elements after the children
    // the description already has, with whatever namespace
    // declarations the file does not bind already.
    let range = target.range();
    let (tag_end, slash) = start_tag_end(xml, range.start)
        .ok_or_else(|| Error::Xmp("the rdf:Description has no start tag".into()))?;
    let indent = line_indent(xml, range.start);
    let inner = format!("{indent} ");
    let mut decls: Vec<(String, String)> = Vec::new();
    let mut body = String::new();
    for prop in Prop::ALL {
        if done[prop.index()] {
            continue;
        }
        let mut bind = |uri: &str, want: &str| bound(target, &mut decls, uri, want);
        if let Some(text) = element(prop, meta, turn, &mut bind, &inner) {
            body.push('\n');
            body.push_str(&inner);
            body.push_str(&text);
        }
    }
    let decls: String = decls
        .iter()
        .map(|(p, uri)| format!("\n{inner} xmlns:{p}=\"{}\"", escape(uri, true)))
        .collect();
    match slash {
        // `<rdf:Description .../>` has to be opened up first.
        Some(slash) if !decls.is_empty() || !body.is_empty() => splices.push((
            slash..tag_end + 1,
            format!("{decls}>{body}\n{indent}</{}>", qname(xml, range.start)),
        )),
        Some(_) => {}
        None => {
            if !decls.is_empty() {
                splices.push((tag_end..tag_end, decls));
            }
            if !body.is_empty() {
                // After the last child, so what is already in the
                // file keeps its order and a rewrite of a packet
                // this module wrote changes nothing.
                let at = xml[..range.end]
                    .rfind('<')
                    .map(|i| xml[..i].trim_end().len())
                    .unwrap_or(tag_end + 1)
                    .max(tag_end + 1);
                splices.push((at..at, body));
            }
        }
    }

    // Whatever this file ends its lines with, so a packet written on
    // Windows does not end up with both.
    let eol = newline(xml);
    if eol != "\n" {
        for (_, text) in splices.iter_mut() {
            *text = text.replace('\n', eol);
        }
    }
    Ok(apply(xml, splices))
}

/// A whole packet for a meta, with nothing else in it. Written
/// straight out rather than through [`write`]: there is nothing to
/// preserve and nothing to parse, and [`element`] is still the one
/// place a property is rendered.
pub fn fresh(meta: &Meta, turn: Option<Turn>) -> String {
    const INDENT: &str = "   ";
    let mut decls: Vec<(String, String)> = Vec::new();
    let mut body = String::new();
    for prop in Prop::ALL {
        let mut bind = |uri: &str, want: &str| {
            if !decls.iter().any(|(_, u)| u == uri) {
                decls.push((want.to_string(), uri.to_string()));
            }
            want.to_string()
        };
        if let Some(text) = element(prop, meta, turn, &mut bind, INDENT) {
            body.push('\n');
            body.push_str(INDENT);
            body.push_str(&text);
        }
    }
    // rdf is declared on the RDF element, so it is never one of
    // these; the rest go on the description that uses them.
    let decls: String = decls
        .iter()
        .filter(|(_, uri)| uri != RDF)
        .map(|(p, uri)| format!("\n    xmlns:{p}=\"{}\"", escape(uri, true)))
        .collect();
    format!(
        "<?xpacket begin=\"\u{feff}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>\n\
         <x:xmpmeta xmlns:x=\"adobe:ns:meta/\" x:xmptk=\"greycard\">\n\
         \x20<rdf:RDF xmlns:rdf=\"{RDF}\">\n\
         \x20 <rdf:Description rdf:about=\"\"{decls}>{body}\n\
         \x20 </rdf:Description>\n\
         \x20</rdf:RDF>\n\
         </x:xmpmeta>\n\
         <?xpacket end=\"w\"?>\n"
    )
}

/// Whether a description carries any property of this module's.
fn ours(desc: &roxmltree::Node<'_, '_>) -> bool {
    desc.attributes()
        .any(|a| prop_of(a.namespace(), a.name()).is_some())
        || desc.children().any(|n| {
            n.is_element() && prop_of(n.tag_name().namespace(), n.tag_name().name()).is_some()
        })
}

/// One property as an element. `bind` hands back the prefix to use
/// for a namespace and records a declaration when the caller has to
/// add one.
fn element(
    prop: Prop,
    meta: &Meta,
    turn: Option<Turn>,
    bind: &mut impl FnMut(&str, &str) -> String,
    indent: &str,
) -> Option<String> {
    if prop.scalar() {
        let value = scalar_value(prop, meta, turn)?;
        let name = format!("{}:{}", bind(prop.ns(), prop.prefix()), prop.name());
        return Some(format!("<{name}>{}</{name}>", escape(&value, false)));
    }
    let (container, words): (&str, Vec<String>) = match prop {
        Prop::Subject if !meta.keywords.is_empty() => ("Bag", meta.keywords.clone()),
        Prop::Title if !meta.title.is_empty() => ("Alt", vec![meta.title.clone()]),
        Prop::Description if !meta.caption.is_empty() => ("Alt", vec![meta.caption.clone()]),
        _ => return None,
    };
    let name = format!("{}:{}", bind(prop.ns(), prop.prefix()), prop.name());
    let rdf = bind(RDF, "rdf");
    let lang = if container == "Alt" {
        " xml:lang=\"x-default\""
    } else {
        ""
    };
    let items: String = words
        .iter()
        .map(|w| {
            format!(
                "\n{indent}  <{rdf}:li{lang}>{}</{rdf}:li>",
                escape(w, false)
            )
        })
        .collect();
    Some(format!(
        "<{name}>\n{indent} <{rdf}:{container}>{items}\n{indent} </{rdf}:{container}>\n{indent}</{name}>"
    ))
}

/// The prefix the file binds `uri` to, or one this write will bind:
/// `want` when it is free, and `want` with a number when it is not.
fn bound(
    target: &roxmltree::Node<'_, '_>,
    decls: &mut Vec<(String, String)>,
    uri: &str,
    want: &str,
) -> String {
    if let Some(p) = target.lookup_prefix(uri).filter(|p| !p.is_empty()) {
        return p.to_string();
    }
    if let Some((p, _)) = decls.iter().find(|(_, u)| u == uri) {
        return p.clone();
    }
    let taken = |p: &str| {
        target.lookup_namespace_uri(Some(p)).is_some() || decls.iter().any(|(q, _)| q == p)
    };
    let mut prefix = want.to_string();
    for n in 1.. {
        if !taken(&prefix) {
            break;
        }
        prefix = format!("{want}{n}");
    }
    decls.push((prefix.clone(), uri.to_string()));
    prefix
}

/// Whether a property already in the file says what the meta says,
/// and so can be left exactly as it is. Only the three that are not
/// scalars need asking: a scalar's value is rewritten in place
/// whatever it was.
fn unchanged(prop: Prop, node: &roxmltree::Node<'_, '_>, meta: &Meta) -> bool {
    let mut theirs = Meta::default();
    match prop {
        Prop::Subject => {
            theirs.set_keywords(items(node));
            !meta.keywords.is_empty() && theirs.keywords == meta.keywords
        }
        Prop::Title => {
            theirs.set_title(default_text(node));
            !meta.title.is_empty() && theirs.title == meta.title
        }
        Prop::Description => {
            theirs.set_caption(default_text(node));
            !meta.caption.is_empty() && theirs.caption == meta.caption
        }
        _ => false,
    }
}

/// An element's name as the file spells it, prefix and all, read
/// from the source: what a closing tag has to repeat.
fn qname(xml: &str, start: usize) -> &str {
    let from = start + 1;
    let len = xml[from..]
        .find(|c: char| c.is_whitespace() || c == '>' || c == '/')
        .unwrap_or(xml.len() - from);
    &xml[from..from + len]
}

/// What this file ends its lines with, by weight of numbers.
fn newline(xml: &str) -> &'static str {
    let crlf = xml.matches("\r\n").count();
    if crlf * 2 > xml.matches('\n').count() {
        "\r\n"
    } else {
        "\n"
    }
}

/// The `>` that ends an element's start tag, and where its `/` is
/// when it is self-closing. A `>` inside an attribute's value is
/// legal, so the quotes are followed.
fn start_tag_end(xml: &str, start: usize) -> Option<(usize, Option<usize>)> {
    let mut quote = None;
    for (i, c) in xml[start..].char_indices().map(|(i, c)| (start + i, c)) {
        match (quote, c) {
            (Some(q), _) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '"' | '\'') => quote = Some(c),
            (None, '>') => {
                let slash = xml[start..i]
                    .rfind(|c: char| !c.is_whitespace())
                    .map(|j| start + j)
                    .filter(|j| xml.as_bytes()[*j] == b'/');
                return Some((i, slash));
            }
            _ => {}
        }
    }
    None
}

/// A range grown backwards over the whitespace before it: how an
/// attribute is taken out without leaving a double space.
fn back_over_space(xml: &str, range: Range<usize>) -> Range<usize> {
    let start = xml[..range.start]
        .trim_end_matches([' ', '\t', '\n', '\r'])
        .len();
    start..range.end
}

/// A range grown backwards over its indentation and the newline
/// before that: how a child element is taken out with its line.
fn back_over_line(xml: &str, range: Range<usize>) -> Range<usize> {
    let mut start = xml[..range.start].trim_end_matches([' ', '\t']).len();
    if xml[..start].ends_with('\n') {
        start -= 1;
        if xml[..start].ends_with('\r') {
            start -= 1;
        }
    }
    start..range.end
}

/// The whitespace at the start of the line `at` falls on.
fn line_indent(xml: &str, at: usize) -> String {
    let from = xml[..at].rfind('\n').map_or(0, |i| i + 1);
    xml[from..at]
        .chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .collect()
}

/// The splices, applied from the back so the earlier offsets hold.
fn apply(xml: &str, mut splices: Vec<(Range<usize>, String)>) -> String {
    splices.sort_by_key(|(r, _)| std::cmp::Reverse(r.start));
    let mut out = xml.to_string();
    let mut last = xml.len() + 1;
    for (range, text) in splices {
        // By construction nothing overlaps; a packet strange enough
        // to make it happen loses the later splice rather than
        // panicking on a byte index.
        if range.end > last {
            continue;
        }
        last = range.start;
        out.replace_range(range, &text);
    }
    out
}

fn escape(text: &str, attribute: bool) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' if attribute => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
    out
}

// -- the files beside a frame ----------------------------------------

/// Lightroom's and Bridge's name: `IMG.CR3` keeps its XMP in
/// `IMG.xmp`.
pub fn short_path(frame: &Path) -> PathBuf {
    frame.with_extension("xmp")
}

/// darktable's other name, and the one that cannot collide:
/// `IMG.CR3.xmp`.
pub fn long_path(frame: &Path) -> PathBuf {
    let mut name = frame
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    name.push(".xmp");
    frame.with_file_name(name)
}

/// The two names that could be this frame's, and whether the short
/// one is its own. `IMG.CR3.xmp` is always this frame's; `IMG.xmp`
/// is only when nothing else in the folder answers to `IMG`, since a
/// raw and a JPEG of one shot would otherwise be handed one sidecar
/// between them and each would write over the other.
///
/// The folder is only read when the answer can matter: when the
/// short name is already there, or when a file is about to be made.
fn owned(frame: &Path, making: bool) -> (PathBuf, Option<PathBuf>) {
    let short = short_path(frame);
    let ask = making || short.exists();
    (
        long_path(frame),
        (ask && !shares_stem(frame)).then_some(short),
    )
}

/// Every XMP beside `frame` that is this frame's and is there: one
/// name usually, both when a folder has been through both tools.
/// What a frame takes with it when it moves.
pub fn paths_of(frame: &Path) -> Vec<PathBuf> {
    let (long, short) = owned(frame, false);
    let mut all = Vec::new();
    if long.exists() {
        all.push(long);
    }
    if let Some(short) = short.filter(|p| p.exists()) {
        all.push(short);
    }
    all
}

/// The XMP beside `frame` that is this frame's, for reading and for
/// writing both, so the two cannot drift apart. When both names are
/// ours and both are there, the newer: whichever wrote last is the
/// one with something to say.
pub fn path_of(frame: &Path) -> Option<PathBuf> {
    let mut all = paths_of(frame);
    all.sort_by_key(|p| modified(p));
    all.pop()
}

/// The XMP beside `frame` to write: the one that is there, so a
/// darktable folder keeps darktable's naming, and otherwise the name
/// [`owned`] says is this frame's.
///
/// None when `create` is false and there is nothing there: a frame
/// with no meta to say never has an XMP made for it.
pub fn write_path(frame: &Path, create: bool) -> Option<PathBuf> {
    if let Some(there) = path_of(frame) {
        return Some(there);
    }
    if !create {
        return None;
    }
    let (long, short) = owned(frame, true);
    Some(short.unwrap_or(long))
}

/// Whether another picture in the folder has this frame's stem: the
/// raw-plus-JPEG pair, and nothing else, since a sidecar of ours is
/// not a picture. Names only, so the folder is read and nothing in
/// it is stat'ed.
fn shares_stem(frame: &Path) -> bool {
    let (Some(dir), Some(stem), Some(name)) =
        (frame.parent(), frame.file_stem(), frame.file_name())
    else {
        return false;
    };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    entries.flatten().any(|e| {
        let entry = e.file_name();
        if entry == name {
            return false;
        }
        let entry = Path::new(&entry);
        let sidecar = entry
            .extension()
            .and_then(|x| x.to_str())
            .is_some_and(|x| matches!(x.to_ascii_lowercase().as_str(), "xmp" | "gcd" | "tmp"));
        !sidecar && entry.file_stem() == Some(stem)
    })
}

fn modified(path: &Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(path).ok()?.modified().ok()
}

// -- what was taken from an XMP, and when ----------------------------

/// What was last taken from the XMP beside a frame: which file it
/// was, how long it was, and a hash of its bytes.
///
/// The `.gcd` keeps this beside the meta so that a load can ask the
/// only question worth asking — *has the XMP changed since it was
/// last read?* — rather than comparing two files' modification times
/// and guessing. Times are the wrong instrument here: a `.gcd`
/// written for an exposure would then hide a later rating from
/// Lightroom, a rating cleared here would be undone by an untouched
/// XMP that happens to be newer, and a folder copied without `-p`
/// would have neither. A hash compares the XMP to itself, and
/// survives the copy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Adopted {
    /// The name, not the path: a folder that moves keeps its mark.
    pub file: String,
    pub len: u64,
    /// FNV-1a over the packet's bytes, in hex. Not a cryptographic
    /// question: nobody is trying to forge a sidecar.
    pub hash: String,
}

impl Adopted {
    fn of(path: &Path, bytes: &[u8]) -> Adopted {
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        for b in bytes {
            hash ^= u64::from(*b);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        Adopted {
            file: path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            len: bytes.len() as u64,
            hash: format!("{hash:016x}"),
        }
    }
}

/// What an adoption took from a packet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Adoption {
    /// The meta moved.
    pub meta: bool,
    /// The quarter turns clockwise `tiff:Orientation` asked for on
    /// top of what the sidecar already had, when it asked for any.
    pub turned: Option<i32>,
    /// There was work placed on the picture itself — a mask, a
    /// repair patch — that the turn could not carry round, because
    /// nothing here knows what shape the picture is. The caller is
    /// the one that can say so.
    pub left_placed: bool,
}

impl Adoption {
    /// Whether anything at all moved.
    pub fn moved(self) -> bool {
        self.meta || self.turned.is_some()
    }
}

/// Take what the XMP beside `frame` says, if it has anything new to
/// say, into `sidecar`'s meta and its turn; what it took.
///
/// The XMP is read when it differs from what was last adopted, and
/// only the fields it carries are taken: a tool that adds a lens
/// name to the sidecar has not thereby cleared the keywords.
///
/// Nothing is written here. The meta and the mark ride on the
/// sidecar in memory and reach the disk with the frame's next real
/// save, so opening somebody's five-hundred-frame Lightroom folder
/// deposits nothing in it.
///
/// A packet that will not parse is passed over with a warning, and
/// is not marked, so the complaint comes back if it is never fixed.
/// The XMP is what a malformed file costs, never the `.gcd`.
pub fn adopt(
    frame: &Path,
    sidecar: &mut crate::Sidecar,
    camera: impl FnOnce() -> Option<Orientation>,
) -> Adoption {
    let mut took = Adoption::default();
    let Some(path) = path_of(frame) else {
        // Nothing there to have a mark about.
        sidecar.xmp = None;
        return took;
    };
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) => {
            log::warn!("{}: {e}", path.display());
            return took;
        }
    };
    let mark = Adopted::of(&path, &bytes);
    if sidecar.xmp.as_ref() == Some(&mark) {
        return took;
    }
    let packet = match read(&String::from_utf8_lossy(&bytes)) {
        Ok(p) => p,
        Err(e) => {
            log::warn!("{}: {e}; the sidecar's meta stands", path.display());
            return took;
        }
    };
    sidecar.xmp = Some(mark);
    // The orientation is the one field here that is not the meta's:
    // the camera's tag is wanted to take the turn back out of it, and
    // it is only asked for when the packet said something about the
    // orientation at all, so a folder of XMPs without one costs
    // nothing to open.
    if packet.has(Prop::Orientation)
        && let Some(turn) = camera().and_then(|c| packet.turn_over(c))
        && turn != sidecar.turn
    {
        let by = i32::from(turn) - i32::from(sidecar.turn);
        // No aspect to give: a folder is opened before a frame of it
        // is decoded, and reading a raw to find its shape would
        // charge every open for a field almost no packet carries a
        // surprise in. The geometry turns, anything placed on the
        // picture does not, and the caller is told both.
        took.left_placed = sidecar.placed();
        sidecar.turn_by(by, None);
        took.turned = Some(by);
    }
    let merged = packet.over(&sidecar.meta);
    if merged != sidecar.meta {
        sidecar.meta = merged;
        took.meta = true;
    }
    took
}

/// Put `meta` into the XMP beside `frame`, making one if there is
/// something to say and none is there; the mark for what is now on
/// disk, or None when there was nothing to write and no file.
///
/// A file whose bytes would not change is left alone, mark and all,
/// so taking a snapshot or renaming a preset does not bump every
/// XMP's modification time.
pub fn save(frame: &Path, meta: &Meta, turn: Option<Turn>) -> Result<Option<Adopted>> {
    let says_something = !meta.is_empty() || turn.is_some_and(|t| t.quarters != 0);
    let Some(path) = write_path(frame, says_something) else {
        return Ok(None);
    };
    let existing = match std::fs::read(&path) {
        Ok(b) => Some(b),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e.into()),
    };
    let text = match &existing {
        Some(bytes) => write(&String::from_utf8_lossy(bytes), meta, turn)?,
        None => fresh(meta, turn),
    };
    let mark = Adopted::of(&path, text.as_bytes());
    if existing.as_deref() == Some(text.as_bytes()) {
        return Ok(Some(mark));
    }
    let tmp = path.with_extension("xmp.tmp");
    std::fs::write(&tmp, &text)?;
    std::fs::rename(&tmp, &path)?;
    Ok(Some(mark))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn full() -> Meta {
        let mut meta = Meta {
            rating: 4,
            flag: Flag::Pick,
            label: Label::Purple,
            title: "Low tide".into(),
            caption: "Third of the morning's set, & the best.".into(),
            ..Meta::default()
        };
        meta.set_keywords(vec!["Sea".into(), "Places/Scotland/Skye".into()]);
        meta
    }

    fn meta_of(xml: &str) -> Meta {
        read(xml).unwrap().meta
    }

    #[test]
    fn every_field_round_trips_through_a_packet() {
        let meta = full();
        let xml = fresh(&meta, None);
        assert_eq!(meta_of(&xml), meta);
        // The properties are the ones the other tools look for.
        assert!(xml.contains("<xmp:Rating>4</xmp:Rating>"), "{xml}");
        assert!(xml.contains("<xmp:Label>Purple</xmp:Label>"), "{xml}");
        assert!(xml.contains("<greycard:Flag>pick</greycard:Flag>"), "{xml}");
        assert!(xml.contains("<rdf:li>Sea</rdf:li>"), "{xml}");
        assert!(xml.contains("<dc:title>"), "{xml}");
        assert!(
            xml.contains("<rdf:li xml:lang=\"x-default\">Low tide</rdf:li>"),
            "{xml}"
        );
        assert!(xml.contains("&amp; the best."), "{xml}");
        // And a second write over the first changes nothing.
        assert_eq!(write(&xml, &meta, None).unwrap(), xml);
    }

    #[test]
    fn a_packet_is_only_the_fields_the_meta_has() {
        let xml = fresh(
            &Meta {
                rating: 2,
                ..Meta::default()
            },
            None,
        );
        assert!(xml.contains("<xmp:Rating>2</xmp:Rating>"), "{xml}");
        assert!(!xml.contains("Label"), "{xml}");
        assert!(!xml.contains("subject"), "{xml}");
        assert!(!xml.contains("Flag"), "{xml}");
        assert_eq!(meta_of(&xml).rating, 2);
    }

    /// The whole composition table: every camera tag against every
    /// turn, out to `tiff:Orientation` and back again.
    #[test]
    fn the_orientation_composes_the_camera_tag_with_the_turn_both_ways() {
        for code in 1..=8u16 {
            let camera = Orientation::from_exif(code).unwrap();
            for quarters in 0..4u8 {
                let turn = Turn::new(camera, quarters);
                let xml = fresh(&Meta::default(), Some(turn));
                let shown = camera.turned(i32::from(quarters));
                assert!(
                    xml.contains(&format!(
                        "<tiff:Orientation>{}</tiff:Orientation>",
                        shown.to_exif()
                    )),
                    "{code} + {quarters}: {xml}"
                );
                // And back: the packet read against the same camera
                // tag gives the turn that was put in.
                let packet = read(&xml).unwrap();
                assert_eq!(
                    packet.turn_over(camera),
                    Some(quarters),
                    "{code} + {quarters}"
                );
                // A turn is the four quarters and nothing else: read
                // against a tag on the other side of the mirror, the
                // packet says nothing rather than something wrong.
                let mirrored = Orientation::from_exif(if code <= 4 { 5 } else { 1 }).unwrap();
                let crossed = mirrored.turns_from(camera).is_none();
                assert_eq!(
                    packet.turn_over(mirrored).is_none(),
                    crossed,
                    "{code} + {quarters}"
                );
            }
        }
    }

    #[test]
    fn a_caller_with_no_camera_tag_neither_writes_nor_believes_the_orientation() {
        // Nothing to say: the property is not written.
        let xml = fresh(&full(), None);
        assert!(!xml.contains("Orientation"), "{xml}");
        // And one already in the file is left exactly where it is,
        // rather than being taken for ours and removed.
        let theirs = fresh(&Meta::default(), Some(Turn::new(Orientation::Rotate90, 0)));
        let out = write(&theirs, &full(), None).unwrap();
        assert!(
            out.contains("<tiff:Orientation>6</tiff:Orientation>"),
            "{out}"
        );
        // The property itself is still read, whatever the writing
        // end could say about it; it is `turn_over` that wants a tag
        // to measure it against, and `adopt` that asks for one.
        assert_eq!(
            read(&theirs).unwrap().turn_over(Orientation::Normal),
            Some(1)
        );
    }

    #[test]
    fn an_orientation_that_is_not_a_tag_is_not_an_orientation_that_was_said() {
        for value in ["0", "9", "left", "", "6.5"] {
            let xml = fresh(&Meta::default(), None).replace(
                "<rdf:Description rdf:about=\"\"",
                &format!(
                    "<rdf:Description rdf:about=\"\" xmlns:tiff=\"{TIFF}\" tiff:Orientation=\"{value}\""
                ),
            );
            assert_eq!(
                read(&xml).unwrap().turn_over(Orientation::Normal),
                None,
                "{value}"
            );
        }
    }

    #[test]
    fn an_xmp_turn_is_adopted_against_the_frames_own_tag() {
        let dir = std::env::temp_dir().join(format!("greycard-xmp-turn-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let frame = dir.join("IMG_7000.CR3");
        std::fs::write(&frame, b"raw").unwrap();
        // Somebody else's packet: the frame shows at 8 where the
        // camera said 6, which is a half turn.
        std::fs::write(
            long_path(&frame),
            fresh(&Meta::default(), Some(Turn::new(Orientation::Rotate270, 0))),
        )
        .unwrap();
        let mut sidecar = crate::Sidecar::default();
        assert!(adopt(&frame, &mut sidecar, || Some(Orientation::Rotate90)).moved());
        assert_eq!(sidecar.turn, 2);
        // Taken once; the same file says nothing new.
        assert!(!adopt(&frame, &mut sidecar, || Some(Orientation::Rotate90)).moved());
        // A packet claiming a mirror is not a turn, and leaves the
        // sidecar where it is.
        let mut mirrored = crate::Sidecar::default();
        mirrored.turn_by(1, Some(1.5));
        std::fs::write(
            long_path(&frame),
            fresh(
                &Meta::default(),
                Some(Turn::new(Orientation::FlipHorizontal, 0)),
            ),
        )
        .unwrap();
        adopt(&frame, &mut mirrored, || Some(Orientation::Rotate90));
        assert_eq!(mirrored.turn, 1);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_reject_is_a_rating_of_minus_one_both_ways() {
        let reject = Meta {
            flag: Flag::Reject,
            ..Meta::default()
        };
        let xml = fresh(&reject, None);
        assert!(xml.contains("<xmp:Rating>-1</xmp:Rating>"), "{xml}");
        assert_eq!(meta_of(&xml), reject);
        // Bridge's and darktable's -1, with nothing of ours beside
        // it, is a reject here too, and says so about the flag.
        let theirs = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF
            xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description
            rdf:about="" xmlns:xmp="http://ns.adobe.com/xap/1.0/"
            xmp:Rating="-1"/></rdf:RDF></x:xmpmeta>"#;
        let packet = read(theirs).unwrap();
        assert_eq!(packet.meta, reject);
        assert!(packet.has(Prop::Flag) && packet.has(Prop::Rating));
        // A frame that is rejected and rated keeps both: the stars
        // are the field's own meaning, the reject is ours.
        let both = Meta {
            rating: 3,
            flag: Flag::Reject,
            ..Meta::default()
        };
        let xml = fresh(&both, None);
        assert!(xml.contains("<xmp:Rating>3</xmp:Rating>"), "{xml}");
        assert_eq!(meta_of(&xml), both);
        // And a pick, which no standard field holds, is ours alone.
        let pick = Meta {
            flag: Flag::Pick,
            ..Meta::default()
        };
        assert!(!fresh(&pick, None).contains("Rating"));
        assert_eq!(meta_of(&fresh(&pick, None)), pick);
    }

    #[test]
    fn a_packet_that_says_nothing_about_a_field_leaves_it_alone() {
        let mine = full();
        // A sidecar somebody else wrote that mentions one property
        // and is silent about the rest.
        let theirs = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF
            xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description
            rdf:about="" xmlns:xmp="http://ns.adobe.com/xap/1.0/"
            xmlns:exif="http://ns.adobe.com/exif/1.0/"
            exif:LensModel="RF 50mm F1.2 L USM" xmp:Rating="1"/></rdf:RDF></x:xmpmeta>"#;
        let packet = read(theirs).unwrap();
        let over = packet.over(&mine);
        assert_eq!(over.rating, 1, "the one it spoke about moved");
        assert_eq!(over.keywords, mine.keywords, "the five it did not, did not");
        assert_eq!((over.title, over.caption), (mine.title, mine.caption));
        assert_eq!((over.label, over.flag), (mine.label, mine.flag));
        // A packet with nothing of ours in it at all changes nothing.
        let none =
            read(r#"<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"/>"#).unwrap();
        assert!(!none.says_anything());
        assert_eq!(none.over(&full()), full());
    }

    #[test]
    fn a_property_that_is_there_and_empty_clears() {
        // An empty Bag is a tool saying there are no keywords; a
        // second description's empty Bag has the last word, as the
        // last value of any field does.
        let xml = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF
            xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
            <rdf:Description rdf:about="" xmlns:dc="http://purl.org/dc/elements/1.1/">
             <dc:subject><rdf:Bag><rdf:li>Skye</rdf:li></rdf:Bag></dc:subject>
            </rdf:Description>
            <rdf:Description rdf:about="" xmlns:dc="http://purl.org/dc/elements/1.1/">
             <dc:subject><rdf:Bag/></dc:subject>
             <dc:title><rdf:Alt><rdf:li xml:lang="x-default"></rdf:li></rdf:Alt></dc:title>
            </rdf:Description></rdf:RDF></x:xmpmeta>"#;
        let packet = read(xml).unwrap();
        assert!(packet.meta.keywords.is_empty() && packet.meta.title.is_empty());
        let over = packet.over(&full());
        assert!(over.keywords.is_empty(), "the empty Bag cleared them");
        assert!(over.title.is_empty());
        assert_eq!(over.rating, 4, "and said nothing about the rating");
    }

    /// What Lightroom Classic writes beside a raw: `crs:` as
    /// attributes, an `xmpMM:` chain, the rating as an attribute.
    fn lightroom() -> String {
        r#"<?xpacket begin="﻿" id="W5M0MpCehiHzreSzNTczkc9d"?>
<x:xmpmeta xmlns:x="adobe:ns:meta/" x:xmptk="Adobe XMP Core 7.0-c000 1.000000, 0000/00/00-00:00:00        ">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about=""
    xmlns:xmp="http://ns.adobe.com/xap/1.0/"
    xmlns:xmpMM="http://ns.adobe.com/xap/1.0/mm/"
    xmlns:dc="http://purl.org/dc/elements/1.1/"
    xmlns:lr="http://ns.adobe.com/lightroom/1.0/"
    xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"
   xmp:ModifyDate="2026-09-19T11:04:22+01:00"
   xmp:Rating="2"
   xmp:Label="Blue"
   xmpMM:DocumentID="xmp.did:4d2ab2c0-8a11-4e0a-9d33-0c5f0a2b1111"
   xmpMM:InstanceID="xmp.iid:4d2ab2c0-8a11-4e0a-9d33-0c5f0a2b2222"
   crs:Version="15.4"
   crs:ProcessVersion="15.4"
   crs:WhiteBalance="As Shot"
   crs:Exposure2012="+0.35"
   crs:Contrast2012="+12"
   crs:HasSettings="True">
   <dc:subject>
    <rdf:Bag>
     <rdf:li>Skye</rdf:li>
    </rdf:Bag>
   </dc:subject>
   <lr:hierarchicalSubject>
    <rdf:Bag>
     <rdf:li>Places|Scotland|Skye</rdf:li>
    </rdf:Bag>
   </lr:hierarchicalSubject>
   <xmpMM:History>
    <rdf:Seq>
     <rdf:li
      stEvt:action="derived"
      stEvt:parameters="converted from image/x-canon-cr3 to image/dng"
      xmlns:stEvt="http://ns.adobe.com/xap/1.0/sType/ResourceEvent#"/>
    </rdf:Seq>
   </xmpMM:History>
   <crs:ToneCurvePV2012>
    <rdf:Seq>
     <rdf:li>0, 0</rdf:li>
     <rdf:li>255, 255</rdf:li>
    </rdf:Seq>
   </crs:ToneCurvePV2012>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>
<?xpacket end="w"?>
"#
        .to_string()
    }

    #[test]
    fn a_lightroom_packet_keeps_its_own_blocks_byte_for_byte() {
        let xml = lightroom();
        let mut meta = meta_of(&xml);
        assert_eq!(meta.rating, 2);
        assert_eq!(meta.label, Label::Blue);
        assert_eq!(meta.keywords, ["Skye"]);
        meta.set_rating(5);
        let out = write(&xml, &meta, None).unwrap();

        // Everything that is not ours is where it was.
        for block in [
            r#"crs:Version="15.4""#,
            r#"crs:Exposure2012="+0.35""#,
            r#"crs:HasSettings="True""#,
            r#"xmpMM:DocumentID="xmp.did:4d2ab2c0-8a11-4e0a-9d33-0c5f0a2b1111""#,
            r#"xmp:ModifyDate="2026-09-19T11:04:22+01:00""#,
            "   <xmpMM:History>\n    <rdf:Seq>\n     <rdf:li\n      stEvt:action=\"derived\"",
            "   <crs:ToneCurvePV2012>\n    <rdf:Seq>\n     <rdf:li>0, 0</rdf:li>",
            "<?xpacket end=\"w\"?>",
        ] {
            assert!(out.contains(block), "{block} went missing\n{out}");
        }
        // The rating moved and nothing else did: one character. The
        // keywords did not move, so Lightroom's paths stayed.
        assert_eq!(out, xml.replace(r#"xmp:Rating="2""#, r#"xmp:Rating="5""#));
        assert!(out.contains("Places|Scotland|Skye"), "{out}");
        assert_eq!(meta_of(&out), meta);
    }

    #[test]
    fn a_keyword_moving_takes_lightrooms_paths_with_it() {
        let xml = lightroom();
        let mut meta = meta_of(&xml);
        meta.add_keyword("Tide");
        let out = write(&xml, &meta, None).unwrap();
        // The flat list is rewritten and the paths beside it go, for
        // Lightroom to build again rather than contradict.
        assert!(!out.contains("hierarchicalSubject"), "{out}");
        assert!(!out.contains("Places|Scotland|Skye"), "{out}");
        assert!(out.contains("<rdf:li>Tide</rdf:li>"), "{out}");
        assert!(out.contains(r#"crs:Exposure2012="+0.35""#), "{out}");
        assert_eq!(meta_of(&out).keywords, ["Skye", "Tide"]);
    }

    #[test]
    fn a_field_taken_off_a_lightroom_packet_takes_its_line_with_it() {
        let xml = lightroom();
        let mut meta = meta_of(&xml);
        meta.label = Label::None;
        meta.set_keywords(Vec::new());
        meta.flag = Flag::Pick;
        let out = write(&xml, &meta, None).unwrap();
        assert!(!out.contains("xmp:Label"), "{out}");
        assert!(!out.contains("dc:subject"), "{out}");
        assert!(!out.contains("<rdf:Bag>"), "{out}");
        // No blank line where the keywords were.
        assert!(!out.contains("\n\n"), "{out}");
        // The flag arrived with a namespace the file did not bind.
        assert!(
            out.contains(&format!(r#"xmlns:greycard="{GREYCARD}""#)),
            "{out}"
        );
        assert!(out.contains("<greycard:Flag>pick</greycard:Flag>"), "{out}");
        assert!(out.contains(r#"crs:Contrast2012="+12""#), "{out}");
        assert_eq!(meta_of(&out), meta);
    }

    #[test]
    fn a_packet_written_with_other_prefixes_is_read_and_kept() {
        // darktable's element form, an unusual prefix, and a
        // description with nothing of ours beside one that has.
        let xml = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about="" xmlns:dt="http://ns.adobe.com/xap/1.0/">
   <dt:Rating>1</dt:Rating>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>"#;
        let mut meta = meta_of(xml);
        assert_eq!(meta.rating, 1);
        meta.set_rating(4);
        meta.set_title("Skye");
        let out = write(xml, &meta, None).unwrap();
        // The element form stayed, under the prefix the file chose.
        assert!(out.contains("<dt:Rating>4</dt:Rating>"), "{out}");
        // dc was not bound, so the title brought its own binding.
        assert!(
            out.contains(r#"xmlns:dc="http://purl.org/dc/elements/1.1/""#),
            "{out}"
        );
        assert_eq!(meta_of(&out), meta);
    }

    #[test]
    fn the_lines_a_write_adds_end_the_way_the_file_ends_its_own() {
        let xml = lightroom().replace('\n', "\r\n");
        let mut meta = meta_of(&xml);
        meta.set_title("Low tide");
        let out = write(&xml, &meta, None).unwrap();
        assert!(out.contains("<dc:title>"), "{out}");
        // Every newline in the result is half of a CRLF.
        assert_eq!(out.matches('\n').count(), out.matches("\r\n").count());
        assert_eq!(meta_of(&out), meta);
    }

    #[test]
    fn a_value_a_packet_spells_oddly_costs_that_field_alone() {
        let xml = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF
          xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description
          rdf:about="" xmlns:xmp="http://ns.adobe.com/xap/1.0/"
          xmp:Rating="lots" xmp:Label="Chartreuse"/></rdf:RDF></x:xmpmeta>"#;
        let packet = read(xml).unwrap();
        assert!(packet.meta.is_empty());
        // A rating that cannot be read is not a rating that was
        // said; the label, which was said, clears.
        assert!(!packet.has(Prop::Rating) && packet.has(Prop::Label));
        // A rating written as a real, and one past the top of the
        // scale, land where the `.gcd`'s loose read puts them.
        let stars = |v: &str| meta_of(&xml.replace(r#""lots""#, v)).rating;
        assert_eq!(stars(r#""3.0""#), 3);
        assert_eq!(stars(r#""4.6""#), 5);
        assert_eq!(stars(r#""9""#), STARS);
        assert_eq!(stars(r#""1e9""#), STARS);
    }

    #[test]
    fn a_packet_that_will_not_parse_is_an_error_and_not_a_rewrite() {
        assert!(read("<x:xmpmeta><unclosed>").is_err());
        assert!(write("<x:xmpmeta><unclosed>", &full(), None).is_err());
        // Well formed, but no description to put anything in.
        assert!(write("<hello/>", &full(), None).is_err());
    }

    #[test]
    fn keywords_come_back_in_order_and_from_either_container() {
        let meta = meta_of(
            r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF
            xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description
            rdf:about="" xmlns:dc="http://purl.org/dc/elements/1.1/">
            <dc:subject><rdf:Seq><rdf:li>sea</rdf:li><rdf:li> rocks </rdf:li>
            <rdf:li>Sea</rdf:li></rdf:Seq></dc:subject>
            <dc:description>  a caption  </dc:description>
            </rdf:Description></rdf:RDF></x:xmpmeta>"#,
        );
        assert_eq!(meta.keywords, ["sea", "rocks"]);
        assert_eq!(meta.caption, "a caption", "trimmed on the way into Meta");
    }

    #[test]
    fn the_long_name_is_the_only_one_a_paired_frame_owns() {
        let dir = tempdir("names");
        let frame = dir.join("IMG_0001.CR3");
        std::fs::write(&frame, b"raw").unwrap();
        assert!(short_path(&frame).ends_with("IMG_0001.xmp"));
        assert!(long_path(&frame).ends_with("IMG_0001.CR3.xmp"));
        assert_eq!(path_of(&frame), None);

        // Nothing to say, nothing made.
        save(&frame, &Meta::default(), None).unwrap();
        assert_eq!(path_of(&frame), None);

        // The short name, Lightroom's, when the folder is bare.
        let meta = full();
        save(&frame, &meta, None).unwrap();
        assert_eq!(path_of(&frame), Some(short_path(&frame)));
        assert_eq!(
            meta_of(&std::fs::read_to_string(short_path(&frame)).unwrap()),
            meta
        );

        // darktable's name, when darktable got there first.
        let other = dir.join("IMG_0002.CR3");
        std::fs::write(&other, b"raw").unwrap();
        std::fs::write(long_path(&other), fresh(&Meta::default(), None)).unwrap();
        assert_eq!(path_of(&other), Some(long_path(&other)));
        save(&other, &meta, None).unwrap();
        assert!(!short_path(&other).exists());
        assert_eq!(
            meta_of(&std::fs::read_to_string(long_path(&other)).unwrap()),
            meta
        );

        // A raw and a JPEG of one shot: IMG_0003.xmp is neither
        // frame's, for reading or for writing, whoever wrote it.
        let raw = dir.join("IMG_0003.CR3");
        std::fs::write(&raw, b"raw").unwrap();
        std::fs::write(dir.join("IMG_0003.JPG"), b"jpeg").unwrap();
        std::fs::write(short_path(&raw), fresh(&meta, None)).unwrap();
        assert_eq!(path_of(&raw), None, "the short name is not ours");
        save(&raw, &meta, None).unwrap();
        assert!(long_path(&raw).exists());
        assert_eq!(
            std::fs::read_to_string(short_path(&raw)).unwrap(),
            fresh(&meta, None),
            "and was not written over"
        );

        // Both there and ours: the newer is the one to read and the
        // one to write, which are the same file.
        std::fs::write(long_path(&frame), fresh(&Meta::default(), None)).unwrap();
        touch(&short_path(&frame), 1_000_000_000);
        touch(&long_path(&frame), 2_000_000_000);
        assert_eq!(path_of(&frame), Some(long_path(&frame)));
        assert_eq!(write_path(&frame, true), Some(long_path(&frame)));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn an_xmp_is_taken_once_and_not_again_until_it_changes() {
        let dir = tempdir("mark");
        let frame = dir.join("IMG_0100.CR3");
        std::fs::write(&frame, b"raw").unwrap();
        let three = Meta {
            rating: 3,
            ..Meta::default()
        };
        std::fs::write(short_path(&frame), fresh(&three, None)).unwrap();

        // First look: the XMP is all there is, so it is taken.
        let mut sidecar = crate::Sidecar::default();
        assert!(adopt(&frame, &mut sidecar, || None).moved());
        assert_eq!(sidecar.meta, three);
        assert!(sidecar.xmp.is_some());
        // Looked at again, it has nothing new to say.
        assert!(!adopt(&frame, &mut sidecar, || None).moved());

        // The rating cleared here, and saved. The XMP is untouched
        // and still says three, but it is the same XMP that was
        // taken, so the three do not come back.
        sidecar.meta.set_rating(0);
        sidecar.save(&frame).unwrap();
        let mut reopened = crate::Sidecar::load(&frame).unwrap().unwrap();
        assert_eq!(reopened.meta.rating, 0);
        assert!(!adopt(&frame, &mut reopened, || None).moved());
        assert_eq!(reopened.meta.rating, 0, "the mark held");

        // Lightroom writes again: a different file, so it is taken.
        let five = Meta {
            rating: 5,
            ..Meta::default()
        };
        std::fs::write(short_path(&frame), fresh(&five, None)).unwrap();
        // Older than the `.gcd`, which is now beside the point.
        touch(&short_path(&frame), 1_000_000_000);
        touch(&crate::Sidecar::path_for(&frame), 2_000_000_000);
        assert!(adopt(&frame, &mut reopened, || None).moved());
        assert_eq!(reopened.meta.rating, 5);

        // The XMP taken away leaves the meta and clears the mark.
        std::fs::remove_file(short_path(&frame)).unwrap();
        assert!(!adopt(&frame, &mut reopened, || None).moved());
        assert_eq!(reopened.meta.rating, 5);
        assert!(reopened.xmp.is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_malformed_xmp_costs_the_xmp_and_never_the_sidecar() {
        let dir = tempdir("malformed");
        let frame = dir.join("IMG_0200.CR3");
        std::fs::write(&frame, b"raw").unwrap();
        std::fs::write(short_path(&frame), "<x:xmpmeta><half of a").unwrap();
        let mut sidecar = crate::Sidecar::default();
        sidecar.meta.set_rating(4);
        // Nothing is taken, and nothing is marked, so the complaint
        // comes back next time.
        assert!(!adopt(&frame, &mut sidecar, || None).moved());
        assert_eq!(sidecar.meta.rating, 4);
        assert!(sidecar.xmp.is_none());
        // And the save refuses rather than writing over what it
        // could not read.
        assert!(save(&frame, &sidecar.meta, None).is_err());
        assert_eq!(
            std::fs::read_to_string(short_path(&frame)).unwrap(),
            "<x:xmpmeta><half of a"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_save_that_would_change_nothing_does_not_touch_the_file() {
        let dir = tempdir("idle");
        let frame = dir.join("IMG_0300.CR3");
        std::fs::write(&frame, b"raw").unwrap();
        let meta = full();
        let first = save(&frame, &meta, None).unwrap().unwrap();
        let path = path_of(&frame).unwrap();
        touch(&path, 1_000_000_000);
        // The same meta again: the bytes would be the same, so the
        // file is left alone, mark and all.
        let again = save(&frame, &meta, None).unwrap().unwrap();
        assert_eq!(again, first);
        assert_eq!(modified(&path), Some(stamp(1_000_000_000)));
        // A real change does write.
        let mut moved = meta.clone();
        moved.set_rating(1);
        assert_ne!(save(&frame, &moved, None).unwrap().unwrap(), first);
        assert_ne!(modified(&path), Some(stamp(1_000_000_000)));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    fn tempdir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("greycard-xmp-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn stamp(secs: u64) -> std::time::SystemTime {
        std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs)
    }

    /// A file's modification time, set, so a test does not have to
    /// sleep through a filesystem's resolution.
    fn touch(path: &Path, secs: u64) {
        std::fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(stamp(secs))
            .unwrap();
    }
}
