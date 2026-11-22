//! A design saves its inspection plan (`checks`): each check's kind (diameter, distance, flatness,
//! position, point), its body and a point on each face it reads, its tolerance and an optional
//! name. What a scan measured is not saved. An older design has none and opens as before; the
//! last release warns about the newer format number and drops the plan on its next save.
