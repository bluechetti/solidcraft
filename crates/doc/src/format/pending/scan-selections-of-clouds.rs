//! A design saves `cloud_selections`: selections of a point cloud's points kept to be opened
//! again (Keep Selection with a cloud selected), each its name, the cloud's name, how many points
//! the cloud had and the points as runs, as `scan_selections` keeps a scan's triangles. An older
//! design has none, and reads as before. The last release refuses a design with this change (the
//! newer format number).
