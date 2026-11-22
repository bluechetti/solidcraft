//! A variable or spline fillet may save `curvature: true`: its sections are curvature-continuous
//! (G2), a Curvature fillet's, with the size following the radius along the edge, instead of
//! circular arcs. Older designs never save it and are read as before. The last release opens a
//! design with such a fillet with a warning about the newer format and makes its round with
//! circular sections (the field is not known there).
