//! A feature can be a Deform (`type: deform`): a body's name and how it is carried through space,
//! `map` a bend (`origin`, `along`, `up`, `length`, `angle`), a twist (`origin`, `axis`, `length`,
//! `angle`) or a cage (`lo`, `hi`, `counts`, `moves` of lattice points). When the design opens the
//! body is made again through it. An older design has no such feature and is not rewritten. The
//! last release opens a design with one with a warning about the newer format, keeps the feature it
//! cannot read and shows the body undeformed.
