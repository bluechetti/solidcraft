//! Selections of a scan's triangles kept in the design (Keep Selection on the Scan tab), to be
//! opened again: each names its scan, says how many triangles the scan had when it was kept (on a
//! scan made again with other triangles it means nothing and is refused), and holds the triangles
//! selected as runs of consecutive ones.

use serde::{Deserialize, Serialize};

use crate::{DocError, Result};

/// The most selections a design keeps.
pub const MAX_SELECTIONS: usize = 1000;
/// The most runs one selection holds.
pub const MAX_RUNS: usize = 10_000_000;
/// The longest name.
pub const MAX_NAME: usize = 200;

/// A kept selection of a scan's triangles.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScanSelection {
    pub name: String,
    /// The scan (a mesh body), and how many triangles it had.
    pub body: String,
    pub triangles: u32,
    /// The triangles selected: runs of consecutive ones, `[first, how many]`, in order.
    pub runs: Vec<[u32; 2]>,
}

impl ScanSelection {
    /// The selection of `flags` (one a triangle of `body`).
    pub fn from_flags(name: &str, body: &str, flags: &[bool]) -> ScanSelection {
        let mut runs: Vec<[u32; 2]> = Vec::new();
        for (t, &on) in flags.iter().enumerate() {
            if !on {
                continue;
            }
            let t = t as u32;
            match runs.last_mut() {
                Some(run) if run[0] + run[1] == t => run[1] += 1,
                _ => runs.push([t, 1]),
            }
        }
        ScanSelection { name: name.to_owned(), body: body.to_owned(), triangles: flags.len() as u32, runs }
    }

    /// One flag a triangle of the scan it was kept of.
    pub fn flags(&self) -> Vec<bool> {
        let mut flags = vec![false; self.triangles as usize];
        for &[first, count] in &self.runs {
            for t in first..first.saturating_add(count).min(self.triangles) {
                flags[t as usize] = true;
            }
        }
        flags
    }

    /// How many triangles are selected.
    pub fn count(&self) -> usize {
        self.runs.iter().map(|r| r[1] as usize).sum()
    }
}

/// Every kept selection well formed: named, not too many, runs in order within the scan.
pub fn check_all(list: &[ScanSelection]) -> Result<()> {
    if list.len() > MAX_SELECTIONS {
        return Err(DocError::Invalid(format!("more than {MAX_SELECTIONS} kept scan selections")));
    }
    for s in list {
        if s.name.trim().is_empty() || s.name.chars().count() > MAX_NAME || s.body.chars().count() > MAX_NAME {
            return Err(DocError::Invalid("a kept scan selection needs a name and its scan".into()));
        }
        if s.runs.len() > MAX_RUNS {
            return Err(DocError::Invalid(format!("the scan selection `{}` is too large", s.name)));
        }
        let mut end = 0u64;
        for &[first, count] in &s.runs {
            let (first, count) = (u64::from(first), u64::from(count));
            if count == 0 || first < end || first + count > u64::from(s.triangles) {
                return Err(DocError::Invalid(format!("the scan selection `{}` names triangles the scan does not have", s.name)));
            }
            end = first + count;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Flags kept as runs come back as they were; runs out of order or past the scan are refused.
    #[test]
    fn flags_are_kept_as_runs_and_checked() {
        let flags = [false, true, true, false, true, false, false, true, true, true];
        let kept = ScanSelection::from_flags("Top", "scan", &flags);
        assert_eq!(kept.runs, vec![[1, 2], [4, 1], [7, 3]]);
        assert_eq!((kept.flags(), kept.count()), (flags.to_vec(), 6));
        assert!(check_all(std::slice::from_ref(&kept)).is_ok());
        let past = ScanSelection { runs: vec![[8, 5]], ..kept.clone() };
        assert!(check_all(&[past]).is_err());
        let backwards = ScanSelection { runs: vec![[4, 1], [1, 2]], ..kept.clone() };
        assert!(check_all(&[backwards]).is_err());
        let nameless = ScanSelection { name: " ".into(), ..kept };
        assert!(check_all(&[nameless]).is_err());
    }
}
