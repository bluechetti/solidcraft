# RulerCraft known-answer parts

Fourteen small original parts with answers fixed in advance, for checking scan-to-CAD results against
the truth. Each `<name>.stl` has a companion `<name>.solidcraft.json` (the editable design it was made
from) or `<name>.json` (fixed bounds, selections and dimensions). `generate_known.py` rebuilds the four
procedural STLs (hole-block, stepped-cylinder, sphere-seat, parallel-bracket) and never touches the
answers.

Source: <https://github.com/flyboy-byte/rulercraft>, `fixtures/known/` and `tools/generate_known.py` at
`518d3ca60d`, by Logan Night and RulerCraft contributors, MIT OR Apache-2.0, imported with the creator's
permission. The `*.solidcraft.json` designs are in RulerCraft's design format `solidcraft/57`; SolidCraft
upgrades them on open. The curved parts are sampled with straight facets, so a mesh volume is slightly
under the design's.

Copyright (c) 2026 Logan Night and RulerCraft contributors. Licensed under the MIT licence or the Apache
Licence 2.0, at your option (the texts are LICENSE-MIT and LICENSE-APACHE at the repository root, the same
terms as SolidCraft itself).
