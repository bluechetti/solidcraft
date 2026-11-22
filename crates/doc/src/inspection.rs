//! An inspection plan: checks picked on the model, each with a tolerance, kept in the design so a
//! scan of the part (or the next part) can be measured against them again. A check names its
//! faces by a body and a point on each face, as a pick does, so it finds them again after the
//! model is edited. What is measured is not kept: it is worked out from the scan when asked.

use serde::{Deserialize, Serialize};
use solidcore_record::Vec3;

/// The most checks a plan holds.
pub const MAX_CHECKS: usize = 500;

/// What a check measures.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckKind {
    /// A bore's or a boss's diameter (a cylindrical face).
    Diameter,
    /// The distance between two parallel flat faces.
    Distance,
    /// How flat a flat face is: the band its scan points lie in.
    Flatness,
    /// Where a hole's axis is against where the model has it: twice the axis's offset.
    Position,
    /// How far the scan is off the model at a point, along the face's normal there.
    Point,
}

impl CheckKind {
    pub fn name(self) -> &'static str {
        match self {
            CheckKind::Diameter => "diameter",
            CheckKind::Distance => "distance",
            CheckKind::Flatness => "flatness",
            CheckKind::Position => "position",
            CheckKind::Point => "point",
        }
    }

    /// Whether its tolerance is one-sided (a zone from 0 up: flatness and position) rather than
    /// plus and minus about the nominal.
    pub fn zone(self) -> bool {
        matches!(self, CheckKind::Flatness | CheckKind::Position)
    }
}

/// One check of the plan.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Check {
    pub id: u64,
    pub kind: CheckKind,
    /// The body its faces are on, and a point on its face (design coordinates, mm).
    pub body: String,
    pub at: Vec3,
    /// A point on the second face, for a distance.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at2: Option<Vec3>,
    /// How far above and below the nominal the measured value may be, mm (a zone check uses
    /// `plus` alone: its value may be 0 to `plus`).
    pub plus: f64,
    pub minus: f64,
    /// The user's name for it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl Check {
    /// Settings a check can be measured with.
    pub fn is_valid(&self) -> bool {
        let finite = |v: Vec3| v.x.is_finite() && v.y.is_finite() && v.z.is_finite();
        finite(self.at)
            && self.at2.is_none_or(finite)
            && (self.kind == CheckKind::Distance) == self.at2.is_some()
            && self.plus.is_finite()
            && self.minus.is_finite()
            && self.plus >= 0.0
            && self.minus >= 0.0
            && self.plus + self.minus > 0.0
            && !self.body.trim().is_empty()
            && self.name.as_ref().is_none_or(|n| n.chars().count() <= 200)
    }
}

/// A class of general tolerances for sizes with no tolerance of their own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    Fine,
    Medium,
    Coarse,
}

impl Class {
    pub fn parse(s: &str) -> Option<Class> {
        match s {
            "fine" => Some(Class::Fine),
            "medium" => Some(Class::Medium),
            "coarse" => Some(Class::Coarse),
            _ => None,
        }
    }

    /// The ± tolerance of a linear size of `nominal` mm in this class (the common general
    /// tolerance table for linear sizes, by size range).
    pub fn linear(self, nominal: f64) -> f64 {
        const UPTO: [f64; 7] = [6.0, 30.0, 120.0, 400.0, 1000.0, 2000.0, f64::INFINITY];
        let row: [f64; 7] = match self {
            Class::Fine => [0.05, 0.1, 0.15, 0.2, 0.3, 0.5, 0.5],
            Class::Medium => [0.1, 0.2, 0.3, 0.5, 0.8, 1.2, 2.0],
            Class::Coarse => [0.3, 0.5, 0.8, 1.2, 2.0, 3.0, 4.0],
        };
        let n = nominal.abs();
        UPTO.iter().zip(row).find(|(upto, _)| n <= **upto).map_or(row[6], |(_, t)| t)
    }

    /// The flatness tolerance of a face `length` mm long (its largest extent) in this class.
    pub fn flatness(self, length: f64) -> f64 {
        const UPTO: [f64; 6] = [10.0, 30.0, 100.0, 300.0, 1000.0, f64::INFINITY];
        let row: [f64; 6] = match self {
            Class::Fine => [0.02, 0.05, 0.1, 0.2, 0.3, 0.4],
            Class::Medium => [0.05, 0.1, 0.2, 0.4, 0.6, 0.8],
            Class::Coarse => [0.1, 0.2, 0.4, 0.8, 1.2, 1.6],
        };
        UPTO.iter().zip(row).find(|(upto, _)| length.abs() <= **upto).map_or(row[5], |(_, t)| t)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(kind: CheckKind) -> Check {
        Check {
            id: 7,
            kind,
            body: "Block".into(),
            at: Vec3::new(1.0, 2.0, 3.0),
            at2: (kind == CheckKind::Distance).then(|| Vec3::new(1.0, 2.0, 0.0)),
            plus: 0.1,
            minus: 0.1,
            name: None,
        }
    }

    #[test]
    fn a_design_with_a_plan_survives_a_round_trip_and_an_older_one_has_none() {
        let mut doc = crate::Document::new("Checked");
        doc.checks.push(check(CheckKind::Diameter));
        doc.checks.push(check(CheckKind::Distance));
        let json = serde_json::to_string(&doc).unwrap();
        let back: crate::Document = serde_json::from_str(&json).unwrap();
        assert_eq!(back, doc);
        let old: crate::Document = serde_json::from_str(&serde_json::to_string(&crate::Document::new("Old")).unwrap()).unwrap();
        assert!(old.checks.is_empty());
    }

    #[test]
    fn a_check_needs_its_faces_and_a_tolerance() {
        assert!(check(CheckKind::Diameter).is_valid() && check(CheckKind::Distance).is_valid());
        assert!(!Check { at2: None, ..check(CheckKind::Distance) }.is_valid(), "a distance needs two faces");
        assert!(!Check { at2: Some(Vec3::ZERO), ..check(CheckKind::Flatness) }.is_valid(), "a flatness has one");
        assert!(!Check { plus: 0.0, minus: 0.0, ..check(CheckKind::Diameter) }.is_valid());
        assert!(!Check { plus: f64::NAN, ..check(CheckKind::Diameter) }.is_valid());
        assert!(!Check { at: Vec3::new(f64::INFINITY, 0.0, 0.0), ..check(CheckKind::Point) }.is_valid());
    }

    #[test]
    fn general_tolerances_grow_with_the_size() {
        assert_eq!(Class::Medium.linear(5.0), 0.1);
        assert_eq!(Class::Medium.linear(25.0), 0.2);
        assert_eq!(Class::Medium.linear(-25.0), 0.2);
        assert_eq!(Class::Fine.linear(100.0), 0.15);
        assert_eq!(Class::Coarse.linear(5000.0), 4.0);
        assert_eq!(Class::Medium.flatness(50.0), 0.2);
        assert_eq!(Class::parse("coarse"), Some(Class::Coarse));
        assert_eq!(Class::parse("loose"), None);
    }
}
