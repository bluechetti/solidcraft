//! A Remesh step can make a hollow part: `shell` (a length expression, which may name parameters)
//! is the wall's thickness, and the step makes the surface at its `offset` and, inside it, the
//! surface that much farther in, facing into the hollow, joined along the scan's open rims. An
//! older design has no such field and is not rewritten. The last release reads the step without
//! it (an unknown field) and makes the plain offset surface; it warns about the newer format number.
