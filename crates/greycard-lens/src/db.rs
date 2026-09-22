//! The lensfun database, read: cameras with their mounts and crop
//! factors, lenses with their calibrations, and the mounts' compatible
//! mounts. The files are the database's own XML (version 2), one
//! `<lensdatabase>` a file; nothing here is interpreted beyond what
//! the elements say.

use std::path::Path;

use crate::Error;

/// A camera body: how to know it, what it takes, and its sensor's size.
#[derive(Debug, Clone, PartialEq)]
pub struct Camera {
    pub maker: String,
    /// The model as the camera writes it, then the names given in
    /// other languages.
    pub models: Vec<String>,
    pub mount: String,
    pub crop_factor: f32,
}

/// The kind of projection a lens makes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LensType {
    #[default]
    Rectilinear,
    /// Any of the fisheye projections; the distortion models do not
    /// describe these and none is offered.
    Fisheye,
}

/// A distortion calibration at one focal length.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DistortionCalibration {
    pub focal: f32,
    /// The real focal length at this nominal one, when measured.
    pub real_focal: Option<f32>,
    pub model: DistortionModel,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DistortionModel {
    Poly3 { k1: f32 },
    Poly5 { k1: f32, k2: f32 },
    Ptlens { a: f32, b: f32, c: f32 },
    Acm { k: [f32; 5] },
}

/// A lateral CA calibration at one focal length: `[v, c, b]` for red
/// and for blue, the linear model's `k` as `v`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TcaCalibration {
    pub focal: f32,
    pub red: [f32; 3],
    pub blue: [f32; 3],
}

/// A vignetting calibration at one focal length, aperture and
/// distance.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VignettingCalibration {
    pub focal: f32,
    pub aperture: f32,
    /// Meters.
    pub distance: f32,
    pub k: [f32; 3],
}

/// A lens: how to know it, what it fits, the sensor it was calibrated
/// on, and its calibrations.
#[derive(Debug, Clone, PartialEq)]
pub struct Lens {
    pub maker: String,
    pub models: Vec<String>,
    pub mounts: Vec<String>,
    /// The crop factor of the sensor the calibrations were made on.
    pub crop_factor: f32,
    /// Width over height of the pictures the calibrations were made
    /// on; three by two when not said.
    pub aspect_ratio: f32,
    pub kind: LensType,
    pub distortion: Vec<DistortionCalibration>,
    pub tca: Vec<TcaCalibration>,
    pub vignetting: Vec<VignettingCalibration>,
}

/// A mount and the mounts whose lenses it takes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mount {
    pub name: String,
    pub compatible: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Database {
    pub cameras: Vec<Camera>,
    pub lenses: Vec<Lens>,
    pub mounts: Vec<Mount>,
}

impl Database {
    /// Every `.xml` file in a directory, read; a file that will not
    /// parse is skipped (the database ships a DTD beside them).
    pub fn load_dir(dir: &Path) -> Result<Self, Error> {
        let mut db = Self::default();
        let mut entries: Vec<_> = std::fs::read_dir(dir)?.flatten().collect();
        entries.sort_by_key(|e| e.path());
        let mut files = 0;
        for e in entries {
            let path = e.path();
            if path.extension().and_then(|e| e.to_str()) != Some("xml") {
                continue;
            }
            let text = std::fs::read_to_string(&path)?;
            if db.read_xml(&text).is_ok() {
                files += 1;
            }
        }
        if files == 0 {
            return Err(Error::Empty(dir.to_path_buf()));
        }
        Ok(db)
    }

    /// Read one file's worth of entries into the database.
    pub fn read_xml(&mut self, text: &str) -> Result<(), Error> {
        let doc = roxmltree::Document::parse(text).map_err(|e| Error::Xml(e.to_string()))?;
        let root = doc.root_element();
        for node in root.children().filter(|n| n.is_element()) {
            match node.tag_name().name() {
                "camera" => {
                    if let Some(c) = camera(&node) {
                        self.cameras.push(c);
                    }
                }
                "lens" => {
                    if let Some(l) = lens(&node) {
                        self.lenses.push(l);
                    }
                }
                "mount" => {
                    if let Some(name) = child_text(&node, "name") {
                        self.mounts.push(Mount {
                            name: name.to_string(),
                            compatible: node
                                .children()
                                .filter(|n| n.has_tag_name("compat"))
                                .filter_map(|n| n.text())
                                .map(|s| s.trim().to_string())
                                .collect(),
                        });
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// The mounts a body with `mount` takes lenses of: its own and the
    /// ones declared compatible.
    pub fn mounts_taken<'a>(&'a self, mount: &'a str) -> Vec<&'a str> {
        let mut out = vec![mount];
        if let Some(m) = self.mounts.iter().find(|m| m.name == mount) {
            out.extend(m.compatible.iter().map(String::as_str));
        }
        out
    }
}

fn child_text<'a>(node: &roxmltree::Node<'a, 'a>, name: &str) -> Option<&'a str> {
    node.children()
        .find(|n| n.has_tag_name(name))
        .and_then(|n| n.text())
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

/// The `<model>` texts: the untagged one first, then the languages.
fn models(node: &roxmltree::Node) -> Vec<String> {
    let mut out: Vec<(bool, String)> = node
        .children()
        .filter(|n| n.has_tag_name("model"))
        .filter_map(|n| {
            n.text()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| (n.has_attribute("lang"), s.to_string()))
        })
        .collect();
    out.sort_by_key(|(lang, _)| *lang);
    out.into_iter().map(|(_, m)| m).collect()
}

fn number(node: &roxmltree::Node, name: &str) -> Option<f32> {
    node.attribute(name)?.trim().parse().ok()
}

fn camera(node: &roxmltree::Node) -> Option<Camera> {
    let models = models(node);
    if models.is_empty() {
        return None;
    }
    Some(Camera {
        maker: child_text(node, "maker")?.to_string(),
        models,
        mount: child_text(node, "mount")?.to_string(),
        crop_factor: child_text(node, "cropfactor")?.parse().ok()?,
    })
}

fn lens(node: &roxmltree::Node) -> Option<Lens> {
    let models = models(node);
    if models.is_empty() {
        return None;
    }
    let aspect_ratio = child_text(node, "aspect-ratio")
        .and_then(parse_ratio)
        .unwrap_or(1.5);
    let kind = match child_text(node, "type") {
        Some("rectilinear") | None => LensType::Rectilinear,
        _ => LensType::Fisheye,
    };
    let mut lens = Lens {
        maker: child_text(node, "maker")?.to_string(),
        models,
        mounts: node
            .children()
            .filter(|n| n.has_tag_name("mount"))
            .filter_map(|n| n.text())
            .map(|s| s.trim().to_string())
            .collect(),
        crop_factor: child_text(node, "cropfactor")?.parse().ok()?,
        aspect_ratio,
        kind,
        distortion: Vec::new(),
        tca: Vec::new(),
        vignetting: Vec::new(),
    };
    if let Some(cal) = node.children().find(|n| n.has_tag_name("calibration")) {
        for c in cal.children().filter(|n| n.is_element()) {
            let Some(focal) = number(&c, "focal") else {
                continue;
            };
            match c.tag_name().name() {
                "distortion" => {
                    let model = match c.attribute("model") {
                        Some("poly3") => DistortionModel::Poly3 {
                            k1: number(&c, "k1").unwrap_or(0.0),
                        },
                        Some("poly5") => DistortionModel::Poly5 {
                            k1: number(&c, "k1").unwrap_or(0.0),
                            k2: number(&c, "k2").unwrap_or(0.0),
                        },
                        Some("ptlens") => DistortionModel::Ptlens {
                            a: number(&c, "a").unwrap_or(0.0),
                            b: number(&c, "b").unwrap_or(0.0),
                            c: number(&c, "c").unwrap_or(0.0),
                        },
                        Some("acm") => DistortionModel::Acm {
                            k: [
                                number(&c, "k1").unwrap_or(0.0),
                                number(&c, "k2").unwrap_or(0.0),
                                number(&c, "k3").unwrap_or(0.0),
                                number(&c, "k4").unwrap_or(0.0),
                                number(&c, "k5").unwrap_or(0.0),
                            ],
                        },
                        _ => continue,
                    };
                    lens.distortion.push(DistortionCalibration {
                        focal,
                        real_focal: number(&c, "real-focal"),
                        model,
                    });
                }
                "tca" => {
                    let (red, blue) = match c.attribute("model") {
                        Some("linear") => (
                            [number(&c, "kr").unwrap_or(1.0), 0.0, 0.0],
                            [number(&c, "kb").unwrap_or(1.0), 0.0, 0.0],
                        ),
                        Some("poly3") => (
                            [
                                number(&c, "vr").unwrap_or(1.0),
                                number(&c, "cr").unwrap_or(0.0),
                                number(&c, "br").unwrap_or(0.0),
                            ],
                            [
                                number(&c, "vb").unwrap_or(1.0),
                                number(&c, "cb").unwrap_or(0.0),
                                number(&c, "bb").unwrap_or(0.0),
                            ],
                        ),
                        _ => continue,
                    };
                    lens.tca.push(TcaCalibration { focal, red, blue });
                }
                "vignetting" => {
                    let k = match c.attribute("model") {
                        Some("pa") => [
                            number(&c, "k1").unwrap_or(0.0),
                            number(&c, "k2").unwrap_or(0.0),
                            number(&c, "k3").unwrap_or(0.0),
                        ],
                        Some("acm") => [
                            number(&c, "alpha1").unwrap_or(0.0),
                            number(&c, "alpha2").unwrap_or(0.0),
                            number(&c, "alpha3").unwrap_or(0.0),
                        ],
                        _ => continue,
                    };
                    lens.vignetting.push(VignettingCalibration {
                        focal,
                        aperture: number(&c, "aperture").unwrap_or(0.0),
                        distance: number(&c, "distance").unwrap_or(1000.0),
                        k,
                    });
                }
                _ => {}
            }
        }
    }
    let by_focal = |a: f32, b: f32| a.partial_cmp(&b).unwrap_or(std::cmp::Ordering::Equal);
    lens.distortion.sort_by(|a, b| by_focal(a.focal, b.focal));
    lens.tca.sort_by(|a, b| by_focal(a.focal, b.focal));
    lens.vignetting.sort_by(|a, b| {
        by_focal(a.focal, b.focal)
            .then(by_focal(a.aperture, b.aperture))
            .then(by_focal(a.distance, b.distance))
    });
    Some(lens)
}

/// "3:2" or "1.5".
fn parse_ratio(s: &str) -> Option<f32> {
    if let Some((w, h)) = s.split_once(':') {
        let (w, h): (f32, f32) = (w.trim().parse().ok()?, h.trim().parse().ok()?);
        (h > 0.0).then(|| w / h)
    } else {
        s.trim().parse().ok()
    }
}

impl Lens {
    /// The lens's name as the database gives it first.
    pub fn name(&self) -> &str {
        &self.models[0]
    }

    /// The focal lengths the calibrations cover, shortest and longest.
    pub fn focal_range(&self) -> Option<(f32, f32)> {
        let focals = self
            .distortion
            .iter()
            .map(|c| c.focal)
            .chain(self.tca.iter().map(|c| c.focal))
            .chain(self.vignetting.iter().map(|c| c.focal));
        let (mut lo, mut hi) = (f32::INFINITY, 0.0f32);
        for f in focals {
            lo = lo.min(f);
            hi = hi.max(f);
        }
        (hi > 0.0).then_some((lo, hi))
    }
}

#[cfg(test)]
pub(crate) const SAMPLE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<lensdatabase version="2">
    <mount>
        <name>Canon RF</name>
        <compat>Canon EF</compat>
    </mount>
    <mount>
        <name>Canon EF</name>
        <compat>Generic</compat>
    </mount>
    <camera>
        <maker>Canon</maker>
        <model>Canon EOS R6</model>
        <model lang="en">EOS R6</model>
        <mount>Canon RF</mount>
        <cropfactor>1</cropfactor>
    </camera>
    <camera>
        <maker>Canon</maker>
        <model>Canon EOS R7</model>
        <model lang="en">EOS R7</model>
        <mount>Canon RF</mount>
        <cropfactor>1.6</cropfactor>
    </camera>
    <lens>
        <maker>Canon</maker>
        <model>Canon RF 50mm F1.8 STM</model>
        <mount>Canon RF</mount>
        <cropfactor>1.0</cropfactor>
        <calibration>
            <distortion model="ptlens" focal="50.0" a="0.002" b="-0.009" c="0.014"/>
            <tca model="poly3" focal="50.0" vr="0.9998424" vb="1.0000271"/>
            <vignetting model="pa" focal="50" aperture="1.8" distance="10" k1="-1.8308" k2="1.8716" k3="-0.8660"/>
            <vignetting model="pa" focal="50" aperture="1.8" distance="1000" k1="-1.8308" k2="1.8716" k3="-0.8660"/>
            <vignetting model="pa" focal="50" aperture="2.5" distance="10" k1="-0.2789" k2="-0.7487" k3="0.3534"/>
            <vignetting model="pa" focal="50" aperture="2.5" distance="1000" k1="-0.2789" k2="-0.7487" k3="0.3534"/>
            <vignetting model="pa" focal="50" aperture="5" distance="10" k1="-0.5085" k2="0.1646" k3="-0.0261"/>
            <vignetting model="pa" focal="50" aperture="22" distance="10" k1="-0.5032" k2="0.1516" k3="-0.0085"/>
        </calibration>
    </lens>
    <lens>
        <maker>Canon</maker>
        <model>Canon RF 24-105mm F4L IS USM</model>
        <mount>Canon RF</mount>
        <cropfactor>1.0</cropfactor>
        <calibration>
            <distortion model="poly3" focal="24" k1="-0.02"/>
            <distortion model="poly3" focal="50" k1="0.01"/>
            <distortion model="poly3" focal="105" k1="0.02"/>
            <tca model="linear" focal="24" kr="1.0002" kb="0.9998"/>
            <tca model="linear" focal="105" kr="0.9999" kb="1.0001"/>
            <vignetting model="pa" focal="24" aperture="4" distance="10" k1="-1.0" k2="0.0" k3="0.0"/>
            <vignetting model="pa" focal="24" aperture="8" distance="10" k1="-0.4" k2="0.0" k3="0.0"/>
            <vignetting model="pa" focal="105" aperture="4" distance="10" k1="-0.6" k2="0.0" k3="0.0"/>
            <vignetting model="pa" focal="105" aperture="8" distance="10" k1="-0.2" k2="0.0" k3="0.0"/>
        </calibration>
    </lens>
    <lens>
        <maker>Canon</maker>
        <model>Canon RF 24-105mm F4-7.1 IS STM</model>
        <mount>Canon RF</mount>
        <cropfactor>1.0</cropfactor>
        <calibration>
            <distortion model="ptlens" focal="24" a="0.0" b="-0.05" c="0.0"/>
        </calibration>
    </lens>
    <lens>
        <maker>Canon</maker>
        <model>Canon EF-S 10-18mm f/4.5-5.6 IS STM</model>
        <mount>Canon EF-S</mount>
        <cropfactor>1.6</cropfactor>
        <calibration>
            <distortion model="ptlens" focal="10" a="0.01" b="-0.03" c="0.0"/>
        </calibration>
    </lens>
    <lens>
        <maker>Sigma</maker>
        <model>Sigma 28mm F1.4 DG HSM | A</model>
        <model lang="en">28mm F1.4 DG HSM | Art</model>
        <mount>Nikon F AF</mount>
        <mount>Leica L</mount>
        <mount>Sony E</mount>
        <cropfactor>1.0</cropfactor>
        <calibration>
            <distortion model="ptlens" focal="28" a="0.0" b="-0.02" c="0.0"/>
        </calibration>
    </lens>
    <lens>
        <maker>Samyang</maker>
        <model>Samyang 8mm f/3.5 Fish-eye</model>
        <mount>Canon EF</mount>
        <cropfactor>1.6</cropfactor>
        <type>fisheye</type>
        <calibration>
            <distortion model="ptlens" focal="8" a="0.01" b="-0.03" c="0.0" real-focal="7.9"/>
        </calibration>
    </lens>
</lensdatabase>
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sample_reads_into_cameras_lenses_and_mounts() {
        let mut db = Database::default();
        db.read_xml(SAMPLE).unwrap();
        assert_eq!(db.cameras.len(), 2);
        assert_eq!(db.lenses.len(), 6);
        assert_eq!(db.mounts.len(), 2);
        let r6 = &db.cameras[0];
        assert_eq!(r6.models, ["Canon EOS R6", "EOS R6"]);
        assert_eq!(r6.crop_factor, 1.0);
        assert_eq!(db.mounts_taken("Canon RF"), ["Canon RF", "Canon EF"]);
        assert_eq!(db.mounts_taken("Nikon Z"), ["Nikon Z"]);
        let fifty = &db.lenses[0];
        assert_eq!(fifty.name(), "Canon RF 50mm F1.8 STM");
        assert_eq!(fifty.aspect_ratio, 1.5);
        assert_eq!(fifty.kind, LensType::Rectilinear);
        assert!(matches!(
            fifty.distortion[0].model,
            DistortionModel::Ptlens { a, b, c } if a == 0.002 && b == -0.009 && c == 0.014
        ));
        assert_eq!(fifty.tca[0].red, [0.9998424, 0.0, 0.0]);
        assert_eq!(fifty.vignetting.len(), 6);
        assert_eq!(fifty.vignetting[0].aperture, 1.8);
        assert_eq!(fifty.focal_range(), Some((50.0, 50.0)));
        let zoom = &db.lenses[1];
        assert_eq!(zoom.focal_range(), Some((24.0, 105.0)));
        assert_eq!(zoom.tca[0].red, [1.0002, 0.0, 0.0]);
        let fish = &db.lenses[5];
        assert_eq!(fish.kind, LensType::Fisheye);
        assert_eq!(fish.distortion[0].real_focal, Some(7.9));
        assert_eq!(parse_ratio("4:3"), Some(4.0 / 3.0));
        assert_eq!(parse_ratio("1.5"), Some(1.5));
    }
}
