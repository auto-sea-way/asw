use crate::coast::{CoastlineIndex, CoastlineSections, GRID_CELLS};
use std::path::Path;

/// Quantization unit for `shore_dist`: 0.02 nm (~37 m) per step.
pub const SHORE_DIST_UNIT_NM: f64 = 0.02;
/// Saturation ceiling: 255 units = 5.1 nm. Distances beyond this store 255.
pub const SHORE_DIST_MAX_NM: f64 = SHORE_DIST_UNIT_NM * 255.0;

/// Quantize a shore distance (nm) to `shore_dist` units, rounding DOWN so
/// the stored clearance never overstates the real one.
///
/// A tiny epsilon is added before flooring to counter f64 division error
/// at exact unit boundaries (e.g. `5.1 / 0.02` evaluates to
/// `254.99999999999997` rather than `255.0`), which would otherwise round
/// an exact boundary down to the wrong unit.
pub fn quantize_shore_dist(nm: f64) -> u8 {
    (nm / SHORE_DIST_UNIT_NM + 1e-9).floor().clamp(0.0, 255.0) as u8
}

const _: () = assert!(
    cfg!(target_endian = "little"),
    "v4 graph files are little-endian"
);

const MAGIC: [u8; 4] = *b"ASW\x04";
const VERSION_OFF: usize = 8; // u8 length + 63 bytes
const NUM_NODES_OFF: usize = 72;
const NUM_EDGES_OFF: usize = 76;
const NUM_RUNS_OFF: usize = 80;
const TABLE_OFF: usize = 88;
const SECTION_COUNT: usize = 9;
const HEADER_LEN: usize = TABLE_OFF + SECTION_COUNT * 16; // 232

const SEC_NODE_H3: usize = 0;
const SEC_OFFSETS: usize = 1;
const SEC_EDGE_TARGETS: usize = 2;
const SEC_SHORE_DIST: usize = 3;
const SEC_COAST_RUNS: usize = 4;
const SEC_COAST_BBOX: usize = 5;
const SEC_COAST_POINTS: usize = 6;
const SEC_GRID_OFFSETS: usize = 7;
const SEC_GRID_IDS: usize = 8;

/// Integer types that may be viewed directly in the mapped file.
pub trait Plain: Copy + private::Sealed {}
mod private {
    pub trait Sealed {}
}
macro_rules! plain {
    ($($t:ty),*) => { $(impl private::Sealed for $t {} impl Plain for $t {})* };
}
plain!(u8, u32, u64, i32);

fn cast_slice<T: Plain>(bytes: &[u8]) -> &[T] {
    let size = std::mem::size_of::<T>();
    assert_eq!(
        bytes.len() % size,
        0,
        "section length is not a multiple of the element size"
    );
    assert_eq!(
        bytes.as_ptr() as usize % std::mem::align_of::<T>(),
        0,
        "section is not aligned"
    );
    // SAFETY: T is a plain integer (sealed), length and alignment checked
    // above, and the returned slice borrows `bytes`.
    unsafe { std::slice::from_raw_parts(bytes.as_ptr() as *const T, bytes.len() / size) }
}

fn bytes_of<T: Plain>(data: &[T]) -> &[u8] {
    // SAFETY: T is a plain integer with no padding.
    unsafe { std::slice::from_raw_parts(data.as_ptr() as *const u8, std::mem::size_of_val(data)) }
}

enum Bytes {
    Mmap(memmap2::Mmap),
    /// u64-backed so every 8-aligned section offset stays aligned in memory.
    Owned {
        buf: Vec<u64>,
        len: usize,
    },
}

impl Bytes {
    fn owned(src: &[u8]) -> Self {
        let mut buf = vec![0u64; src.len().div_ceil(8)];
        // SAFETY: the u64 buffer is at least src.len() bytes long.
        let dst = unsafe { std::slice::from_raw_parts_mut(buf.as_mut_ptr() as *mut u8, src.len()) };
        dst.copy_from_slice(src);
        Bytes::Owned {
            buf,
            len: src.len(),
        }
    }

    fn as_slice(&self) -> &[u8] {
        match self {
            Bytes::Mmap(m) => &m[..],
            Bytes::Owned { buf, len } => &bytes_of(buf)[..*len],
        }
    }
}

#[derive(Clone, Copy)]
struct Section {
    off: usize,
    len: usize,
}

/// File layout: 232-byte header, then nine 8-byte-aligned little-endian
/// sections (see the v4 design spec). The struct is a view over the bytes;
/// nothing is decoded at open time.
///
/// Nodes are H3 cell indices in strictly ascending order (array index =
/// node id). Edges are CSR: `offsets` into a varint stream of ascending
/// target deltas. Weights are recomputed at query time as centre-to-centre
/// haversine, so nothing per edge is stored but the id.
pub struct RoutingGraph {
    bytes: Bytes,
    version: String,
    num_nodes: u32,
    num_edges: u32,
    num_runs: u32,
    sections: [Section; SECTION_COUNT],
}

impl std::fmt::Debug for RoutingGraph {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RoutingGraph")
            .field("version", &self.version)
            .field("num_nodes", &self.num_nodes)
            .field("num_edges", &self.num_edges)
            .field("num_runs", &self.num_runs)
            .finish()
    }
}

impl RoutingGraph {
    /// Memory-map a v4 file. `populate` asks the kernel to read the whole
    /// file in at open (MAP_POPULATE on Linux, MADV_WILLNEED elsewhere).
    pub fn open(path: &Path, populate: bool) -> anyhow::Result<Self> {
        let file = std::fs::File::open(path)?;
        let mut opts = memmap2::MmapOptions::new();
        if populate {
            opts.populate();
        }
        // SAFETY: the file is only ever replaced whole by rename; a mapping
        // of the old inode stays valid until dropped.
        let mmap = unsafe { opts.map(&file)? };
        #[cfg(unix)]
        if populate {
            let _ = mmap.advise(memmap2::Advice::WillNeed);
        }
        Self::parse(Bytes::Mmap(mmap))
    }

    pub fn from_bytes(bytes: Vec<u8>) -> anyhow::Result<Self> {
        Self::parse(Bytes::owned(&bytes))
    }

    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        std::fs::write(path, self.bytes.as_slice())?;
        Ok(())
    }

    fn parse(bytes: Bytes) -> anyhow::Result<Self> {
        let b = bytes.as_slice();
        anyhow::ensure!(
            b.len() >= 4 && &b[..3] == b"ASW",
            "Not an ASW graph file (expected ASW magic header). Rebuild required."
        );
        anyhow::ensure!(
            b[3] == MAGIC[3],
            "Unsupported ASW graph version {} (expected 4). Rebuild required.",
            b[3]
        );
        anyhow::ensure!(b.len() >= HEADER_LEN, "graph header truncated");
        let u32_at = |o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap());
        let u64_at = |o: usize| u64::from_le_bytes(b[o..o + 8].try_into().unwrap());
        let vlen = b[VERSION_OFF] as usize;
        anyhow::ensure!(vlen <= 63, "graph version string too long");
        let version = std::str::from_utf8(&b[VERSION_OFF + 1..VERSION_OFF + 1 + vlen])?.to_string();
        let num_nodes = u32_at(NUM_NODES_OFF);
        let num_edges = u32_at(NUM_EDGES_OFF);
        let num_runs = u32_at(NUM_RUNS_OFF);
        let mut sections = [Section { off: 0, len: 0 }; SECTION_COUNT];
        for (i, s) in sections.iter_mut().enumerate() {
            let off = u64_at(TABLE_OFF + i * 16) as usize;
            let len = u64_at(TABLE_OFF + i * 16 + 8) as usize;
            anyhow::ensure!(off.is_multiple_of(8), "section {i} is not 8-byte aligned");
            anyhow::ensure!(
                off >= HEADER_LEN && off.saturating_add(len) <= b.len(),
                "section {i} extends beyond end of file"
            );
            *s = Section { off, len };
        }
        let n = num_nodes as usize;
        let r = num_runs as usize;
        let expect = |i: usize, len: usize, what: &str| -> anyhow::Result<()> {
            anyhow::ensure!(
                sections[i].len == len,
                "{what} section length {} != expected {len}",
                sections[i].len
            );
            Ok(())
        };
        expect(SEC_NODE_H3, n * 8, "node_h3")?;
        expect(SEC_OFFSETS, (n + 1) * 4, "offsets")?;
        expect(SEC_SHORE_DIST, n, "shore_dist")?;
        expect(SEC_COAST_RUNS, (r + 1) * 4, "coast_runs")?;
        expect(SEC_COAST_BBOX, r * 16, "coast_bbox")?;
        expect(SEC_GRID_OFFSETS, (GRID_CELLS + 1) * 4, "grid_offsets")?;
        anyhow::ensure!(
            sections[SEC_COAST_POINTS].len % 8 == 0,
            "coast_points length not a multiple of 8"
        );
        anyhow::ensure!(
            sections[SEC_GRID_IDS].len % 4 == 0,
            "grid_ids length not a multiple of 4"
        );
        let g = Self {
            bytes,
            version,
            num_nodes,
            num_edges,
            num_runs,
            sections,
        };
        // CSR tables: first entry 0, last entry = target section length. The
        // two small tables are also checked for monotonicity (a few ms); the
        // node-sized `offsets` table is not scanned, keeping open O(1) in
        // the node count as the spec asks.
        let offsets: &[u32] = g.section(SEC_OFFSETS);
        anyhow::ensure!(offsets[0] == 0, "offsets[0] != 0");
        anyhow::ensure!(
            offsets[n] as usize == g.sections[SEC_EDGE_TARGETS].len,
            "offsets sentinel != edge_targets length"
        );
        let runs: &[u32] = g.section(SEC_COAST_RUNS);
        anyhow::ensure!(runs[0] == 0, "coast_runs[0] != 0");
        anyhow::ensure!(
            runs.windows(2).all(|w| w[0] <= w[1]),
            "coast_runs not monotonic"
        );
        anyhow::ensure!(
            runs[r] as usize * 8 == g.sections[SEC_COAST_POINTS].len,
            "coast_runs sentinel != coast_points length"
        );
        let grid: &[u32] = g.section(SEC_GRID_OFFSETS);
        anyhow::ensure!(grid[0] == 0, "grid_offsets[0] != 0");
        anyhow::ensure!(
            grid.windows(2).all(|w| w[0] <= w[1]),
            "grid_offsets not monotonic"
        );
        anyhow::ensure!(
            grid[GRID_CELLS] as usize * 4 == g.sections[SEC_GRID_IDS].len,
            "grid_offsets sentinel != grid_ids length"
        );
        Ok(g)
    }

    fn section<T: Plain>(&self, i: usize) -> &[T] {
        let s = self.sections[i];
        cast_slice(&self.bytes.as_slice()[s.off..s.off + s.len])
    }

    pub fn num_nodes(&self) -> u32 {
        self.num_nodes
    }

    pub fn num_edges(&self) -> u32 {
        self.num_edges
    }

    /// Version string stored in the file header (set by the build).
    pub fn version(&self) -> &str {
        &self.version
    }

    pub fn num_coast_runs(&self) -> u32 {
        self.num_runs
    }

    /// All node H3 ids, strictly ascending. Index = node id.
    pub fn node_h3s(&self) -> &[u64] {
        self.section(SEC_NODE_H3)
    }

    pub fn node_h3(&self, node: u32) -> u64 {
        self.node_h3s()[node as usize]
    }

    /// Quantized distance to shore (SHORE_DIST_UNIT_NM units, 255 = >= 5.1 nm).
    pub fn shore_dist(&self, node: u32) -> u8 {
        self.section::<u8>(SEC_SHORE_DIST)[node as usize]
    }

    /// Binary search for an exact H3 cell index.
    pub fn h3_lookup(&self, h3: u64) -> Option<u32> {
        self.node_h3s().binary_search(&h3).ok().map(|i| i as u32)
    }

    pub fn coastline(&self) -> CoastlineIndex<'_> {
        CoastlineIndex::from_slices(
            self.section(SEC_COAST_RUNS),
            self.section(SEC_COAST_BBOX),
            self.section(SEC_COAST_POINTS),
            self.section(SEC_GRID_OFFSETS),
            self.section(SEC_GRID_IDS),
        )
    }

    /// Iterate neighbors of `node` as (target_id, weight_nm) pairs.
    pub fn neighbors(&self, node: u32) -> NeighborIter<'_> {
        let offsets: &[u32] = self.section(SEC_OFFSETS);
        let (start, end) = (
            offsets[node as usize] as usize,
            offsets[node as usize + 1] as usize,
        );
        let (src_lat, src_lon) = self.node_pos(node);
        NeighborIter {
            graph: self,
            data: &self.section::<u8>(SEC_EDGE_TARGETS)[start..end],
            pos: 0,
            prev_target: 0,
            src_lat,
            src_lon,
        }
    }

    /// Neighbour ids only, no centre decode. A* uses this with its own
    /// position cache; `neighbors()` computes the weight for everyone else.
    pub fn neighbor_ids(&self, node: u32) -> impl Iterator<Item = u32> + '_ {
        let offsets: &[u32] = self.section(SEC_OFFSETS);
        let (start, end) = (
            offsets[node as usize] as usize,
            offsets[node as usize + 1] as usize,
        );
        let data = &self.section::<u8>(SEC_EDGE_TARGETS)[start..end];
        let mut pos = 0usize;
        let mut prev = 0u32;
        std::iter::from_fn(move || {
            if pos >= data.len() {
                return None;
            }
            let (delta, new_pos) = crate::varint::decode(data, pos);
            pos = new_pos;
            prev += delta;
            Some(prev)
        })
    }

    /// Decode H3 cell center coordinates to f64 (lat, lng) in degrees.
    pub fn node_pos(&self, node: u32) -> (f64, f64) {
        let cell = h3o::CellIndex::try_from(self.node_h3(node)).expect("invalid H3 index");
        crate::h3::cell_center(cell)
    }
}

/// Iterator over a node's neighbors, decoding varint target deltas and
/// computing each edge's length from the two cell centres.
pub struct NeighborIter<'a> {
    graph: &'a RoutingGraph,
    data: &'a [u8],
    pos: usize,
    prev_target: u32,
    src_lat: f64,
    src_lon: f64,
}

impl Iterator for NeighborIter<'_> {
    type Item = (u32, f32);

    fn next(&mut self) -> Option<Self::Item> {
        if self.pos >= self.data.len() {
            return None;
        }
        let (delta, new_pos) = crate::varint::decode(self.data, self.pos);
        self.pos = new_pos;
        let target = self.prev_target + delta;
        self.prev_target = target;
        // ponytail: one cell-centre decode per relaxation; cache positions in
        // the A* buffers if the bench says this dominates.
        let (lat, lon) = self.graph.node_pos(target);
        let weight_nm = crate::h3::haversine_nm(self.src_lat, self.src_lon, lat, lon) as f32;
        Some((target, weight_nm))
    }
}

#[derive(Default)]
pub struct GraphBuilder {
    /// (h3_index, shore_dist_q) per node, in the order added (must be
    /// strictly ascending by H3).
    nodes: Vec<(u64, u8)>,
    /// (src, dst)
    edges: Vec<(u32, u32)>,
    /// Coastline runs as (lon, lat) degrees.
    pub coastline_runs: Vec<Vec<(f64, f64)>>,
    /// Stored in the header; at most 63 bytes.
    pub version: String,
}

impl GraphBuilder {
    /// Empty builder whose file will carry `version` in its header.
    pub fn with_version(version: impl Into<String>) -> Self {
        Self {
            version: version.into(),
            ..Self::default()
        }
    }

    /// Add a node with its quantized shore distance. Returns node ID.
    pub fn add_node(&mut self, h3_index: u64, shore_dist_q: u8) -> u32 {
        let id = self.nodes.len() as u32;
        self.nodes.push((h3_index, shore_dist_q));
        id
    }

    /// Add a bidirectional edge. Weights are not stored: the router computes
    /// centre-to-centre haversine at query time.
    pub fn add_edge(&mut self, src: u32, dst: u32) {
        self.edges.push((src, dst));
        self.edges.push((dst, src));
    }

    /// Add a one-way edge.
    pub fn add_directed_edge(&mut self, src: u32, dst: u32) {
        self.edges.push((src, dst));
    }

    /// Keep only the largest connected component, renumbering node ids and
    /// preserving H3 order. Returns self unchanged when already connected.
    pub fn prune_to_main_component(mut self) -> Self {
        let n = self.nodes.len();
        let labels = component_labels(n, &self.edges);
        let mut comp_sizes: std::collections::HashMap<u32, usize> =
            std::collections::HashMap::new();
        for &root in &labels {
            *comp_sizes.entry(root).or_insert(0) += 1;
        }
        let Some((&main_root, &main_count)) = comp_sizes.iter().max_by_key(|(_, c)| **c) else {
            return self;
        };
        if main_count == n {
            return self;
        }
        tracing::info!(
            "Pruning {} nodes in {} small components (keeping {} in main component)",
            n - main_count,
            comp_sizes.len() - 1,
            main_count,
        );
        let mut old_to_new: Vec<Option<u32>> = vec![None; n];
        let mut kept = Vec::with_capacity(main_count);
        for (old, node) in self.nodes.iter().enumerate() {
            if labels[old] == main_root {
                old_to_new[old] = Some(kept.len() as u32);
                kept.push(*node);
            }
        }
        self.edges = self
            .edges
            .iter()
            .filter_map(|&(s, d)| Some((old_to_new[s as usize]?, old_to_new[d as usize]?)))
            .collect();
        self.nodes = kept;
        self
    }

    /// Encode the v4 file image. Panics on builder misuse (unsorted or
    /// invalid H3 ids): the builder is the only writer, so this is the one
    /// place the invariants are checked.
    pub fn build_bytes(self) -> Vec<u8> {
        for w in self.nodes.windows(2) {
            assert!(
                w[0].0 < w[1].0,
                "nodes must be added in strictly ascending H3 order"
            );
        }
        for &(h3, _) in &self.nodes {
            assert!(
                h3o::CellIndex::try_from(h3).is_ok(),
                "invalid H3 index {h3:#x}"
            );
        }
        assert!(
            self.version.len() <= 63,
            "version string longer than 63 bytes"
        );
        let num_nodes = self.nodes.len() as u32;
        let node_h3: Vec<u64> = self.nodes.iter().map(|(h3, _)| *h3).collect();
        let shore_dist: Vec<u8> = self.nodes.iter().map(|(_, q)| *q).collect();

        // Group edges by source, sort and dedup targets per source
        let mut adj: Vec<Vec<u32>> = vec![Vec::new(); num_nodes as usize];
        for &(src, dst) in &self.edges {
            adj[src as usize].push(dst);
        }
        let mut edge_targets = Vec::new();
        let mut offsets = Vec::with_capacity(num_nodes as usize + 1);
        let mut num_edges = 0u32;
        for list in &mut adj {
            list.sort_unstable();
            list.dedup();
            offsets.push(edge_targets.len() as u32);
            let mut prev = 0u32;
            for &target in list.iter() {
                crate::varint::encode(target - prev, &mut edge_targets);
                prev = target;
                num_edges += 1;
            }
        }
        offsets.push(edge_targets.len() as u32);

        let coast = CoastlineSections::from_runs(&self.coastline_runs);

        let mut out = vec![0u8; HEADER_LEN];
        let mut table: Vec<(u64, u64)> = Vec::with_capacity(SECTION_COUNT);
        fn push<T: Plain>(out: &mut Vec<u8>, table: &mut Vec<(u64, u64)>, data: &[T]) {
            while !out.len().is_multiple_of(8) {
                out.push(0);
            }
            let bytes = bytes_of(data);
            table.push((out.len() as u64, bytes.len() as u64));
            out.extend_from_slice(bytes);
        }
        push(&mut out, &mut table, &node_h3);
        push(&mut out, &mut table, &offsets);
        push(&mut out, &mut table, &edge_targets);
        push(&mut out, &mut table, &shore_dist);
        push(&mut out, &mut table, &coast.runs);
        push(&mut out, &mut table, &coast.bbox);
        push(&mut out, &mut table, &coast.points);
        push(&mut out, &mut table, &coast.grid_offsets);
        push(&mut out, &mut table, &coast.grid_ids);
        while !out.len().is_multiple_of(8) {
            out.push(0);
        }

        out[..4].copy_from_slice(&MAGIC);
        out[VERSION_OFF] = self.version.len() as u8;
        out[VERSION_OFF + 1..VERSION_OFF + 1 + self.version.len()]
            .copy_from_slice(self.version.as_bytes());
        out[NUM_NODES_OFF..NUM_NODES_OFF + 4].copy_from_slice(&num_nodes.to_le_bytes());
        out[NUM_EDGES_OFF..NUM_EDGES_OFF + 4].copy_from_slice(&num_edges.to_le_bytes());
        out[NUM_RUNS_OFF..NUM_RUNS_OFF + 4]
            .copy_from_slice(&((coast.runs.len() - 1) as u32).to_le_bytes());
        for (i, (off, len)) in table.iter().enumerate() {
            out[TABLE_OFF + i * 16..TABLE_OFF + i * 16 + 8].copy_from_slice(&off.to_le_bytes());
            out[TABLE_OFF + i * 16 + 8..TABLE_OFF + i * 16 + 16]
                .copy_from_slice(&len.to_le_bytes());
        }
        out
    }

    /// Build an in-memory graph (tests and small regional builds).
    pub fn build(self) -> RoutingGraph {
        RoutingGraph::from_bytes(self.build_bytes()).expect("builder wrote an invalid graph image")
    }
}

/// Union-find component root per node over an edge list.
fn component_labels(n: usize, edges: &[(u32, u32)]) -> Vec<u32> {
    debug_assert!(n <= u32::MAX as usize);
    let mut parent: Vec<u32> = (0..n as u32).collect();
    let mut rank = vec![0u8; n];

    fn find(parent: &mut [u32], x: u32) -> u32 {
        let mut root = x;
        while parent[root as usize] != root {
            root = parent[root as usize];
        }
        // Path compression
        let mut cur = x;
        while cur != root {
            let next = parent[cur as usize];
            parent[cur as usize] = root;
            cur = next;
        }
        root
    }

    fn union(parent: &mut [u32], rank: &mut [u8], a: u32, b: u32) {
        let ra = find(parent, a);
        let rb = find(parent, b);
        if ra == rb {
            return;
        }
        if rank[ra as usize] < rank[rb as usize] {
            parent[ra as usize] = rb;
        } else if rank[ra as usize] > rank[rb as usize] {
            parent[rb as usize] = ra;
        } else {
            parent[rb as usize] = ra;
            rank[ra as usize] += 1;
        }
    }

    for &(a, b) in edges {
        union(&mut parent, &mut rank, a, b);
    }
    drop(rank);
    for i in 0..n as u32 {
        find(&mut parent, i);
    }
    parent
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square_graph() -> RoutingGraph {
        // Use real H3 cells at resolution 5, sorted by H3 index
        let c0 = h3o::LatLng::new(0.0, 0.0)
            .unwrap()
            .to_cell(h3o::Resolution::Five);
        let c1 = h3o::LatLng::new(0.0, 1.0)
            .unwrap()
            .to_cell(h3o::Resolution::Five);
        let c2 = h3o::LatLng::new(1.0, 0.0)
            .unwrap()
            .to_cell(h3o::Resolution::Five);
        let c3 = h3o::LatLng::new(1.0, 1.0)
            .unwrap()
            .to_cell(h3o::Resolution::Five);

        let mut cells: Vec<(u64, f64, f64)> = vec![
            (u64::from(c0), 0.0, 0.0),
            (u64::from(c1), 0.0, 1.0),
            (u64::from(c2), 1.0, 0.0),
            (u64::from(c3), 1.0, 1.0),
        ];
        cells.sort_by_key(|(h3, _, _)| *h3);

        let mut b = GraphBuilder::default();
        let mut ids = Vec::new();
        for (h3, _, _) in &cells {
            ids.push(b.add_node(*h3, 255));
        }

        // Find which sorted index corresponds to which original cell
        let idx_of = |target_h3: u64| -> u32 {
            cells
                .iter()
                .position(|(h3, _, _)| *h3 == target_h3)
                .unwrap() as u32
        };

        let n0 = idx_of(u64::from(c0));
        let n1 = idx_of(u64::from(c1));
        let n2 = idx_of(u64::from(c2));
        let n3 = idx_of(u64::from(c3));

        b.add_edge(n0, n1);
        b.add_edge(n1, n3);
        b.add_edge(n0, n2);
        b.add_edge(n2, n3);
        b.build()
    }

    #[test]
    fn graph_builder_counts() {
        let g = square_graph();
        assert_eq!(g.num_nodes(), 4);
        assert_eq!(g.num_edges(), 8); // 4 bidirectional = 8 directed
    }

    #[test]
    fn graph_neighbors() {
        let g = square_graph();
        // Just check node 0 has 2 neighbors
        let n0: Vec<(u32, f32)> = g.neighbors(0).collect();
        assert_eq!(n0.len(), 2);
    }

    #[test]
    fn graph_node_pos_h3_roundtrip() {
        let g = square_graph();
        // Each node should decode to a valid lat/lng
        for i in 0..g.num_nodes() {
            let (lat, lng) = g.node_pos(i);
            assert!((-90.0..=90.0).contains(&lat), "lat out of range: {}", lat);
            assert!((-180.0..=180.0).contains(&lng), "lng out of range: {}", lng);
        }
    }

    #[test]
    fn builder_produces_compact_format() {
        let c0 = h3o::LatLng::new(51.5, -0.1)
            .unwrap()
            .to_cell(h3o::Resolution::Five);
        let c1 = h3o::LatLng::new(48.8, 2.3)
            .unwrap()
            .to_cell(h3o::Resolution::Five);
        let c2 = h3o::LatLng::new(10.0, 10.0)
            .unwrap()
            .to_cell(h3o::Resolution::Five);

        let mut cells: Vec<(u64, f64, f64)> = vec![
            (u64::from(c0), 51.5, -0.1),
            (u64::from(c1), 48.8, 2.3),
            (u64::from(c2), 10.0, 10.0),
        ];
        cells.sort_by_key(|(h3, _, _)| *h3);

        let mut b = GraphBuilder::default();
        let mut ids = Vec::new();
        for (h3, _, _) in &cells {
            ids.push(b.add_node(*h3, 255));
        }

        let idx_of = |target_h3: u64| -> u32 {
            cells
                .iter()
                .position(|(h3, _, _)| *h3 == target_h3)
                .unwrap() as u32
        };

        let n0 = idx_of(u64::from(c0));
        let n1 = idx_of(u64::from(c1));
        let n2 = idx_of(u64::from(c2));

        b.add_edge(n0, n1);
        b.add_edge(n0, n2);

        let g = b.build();

        assert_eq!(g.num_nodes(), 3);
        assert_eq!(g.num_edges(), 4);

        let n0_neighbors: Vec<(u32, f32)> = g.neighbors(n0).collect();
        assert_eq!(n0_neighbors.len(), 2);

        let n1_neighbors: Vec<(u32, f32)> = g.neighbors(n1).collect();
        assert_eq!(n1_neighbors.len(), 1);
        assert_eq!(n1_neighbors[0].0, n0);
        let (lat0, lon0) = g.node_pos(n0);
        let (lat1, lon1) = g.node_pos(n1);
        let expected = crate::h3::haversine_nm(lat1, lon1, lat0, lon0);
        assert!((n1_neighbors[0].1 as f64 - expected).abs() < 1e-3);
    }

    fn square_graph_bytes() -> Vec<u8> {
        let c0 = h3o::LatLng::new(0.0, 0.0)
            .unwrap()
            .to_cell(h3o::Resolution::Five);
        let c1 = h3o::LatLng::new(0.0, 1.0)
            .unwrap()
            .to_cell(h3o::Resolution::Five);
        let mut cells = [u64::from(c0), u64::from(c1)];
        cells.sort_unstable();
        let mut b = GraphBuilder::default();
        let n0 = b.add_node(cells[0], 255);
        let n1 = b.add_node(cells[1], 255);
        b.add_edge(n0, n1);
        b.build_bytes()
    }

    #[test]
    fn build_bytes_layout_header() {
        let b = GraphBuilder::with_version("0.7.0 2026-10-03");
        let bytes = b.build_bytes();
        assert_eq!(&bytes[0..4], b"ASW\x04");
        assert_eq!(bytes[8] as usize, "0.7.0 2026-10-03".len());
        assert_eq!(&bytes[9..25], b"0.7.0 2026-10-03");
        assert_eq!(bytes.len() % 8, 0);
        let g = RoutingGraph::from_bytes(bytes).unwrap();
        assert_eq!(g.version(), "0.7.0 2026-10-03");
        assert_eq!(g.num_nodes(), 0);
        assert_eq!(g.coastline().run_count(), 0);
    }

    #[test]
    fn save_open_roundtrip_through_mmap() {
        let g = square_graph();
        let dir = std::env::temp_dir().join(format!("asw-v4-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("square.graph");
        g.save(&path).unwrap();
        let loaded = RoutingGraph::open(&path, false).unwrap();
        assert_eq!(loaded.num_nodes(), g.num_nodes());
        assert_eq!(loaded.num_edges(), g.num_edges());
        assert_eq!(loaded.node_h3s(), g.node_h3s());
        let a: Vec<(u32, f32)> = loaded.neighbors(0).collect();
        let b: Vec<(u32, f32)> = g.neighbors(0).collect();
        assert_eq!(a, b);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn coastline_roundtrips_through_the_file() {
        let b = GraphBuilder {
            coastline_runs: vec![vec![(28.4, 36.0), (28.4, 37.5)]],
            ..GraphBuilder::default()
        };
        let g = b.build();
        let coast = g.coastline();
        assert_eq!(coast.run_count(), 1);
        assert!(coast.crosses_land(28.3, 36.5, 28.5, 36.5));
        let pts: Vec<(f64, f64)> = coast.run_points(0).collect();
        assert_eq!(pts, vec![(28.4, 36.0), (28.4, 37.5)]);
    }

    #[test]
    fn from_bytes_rejects_v3_and_garbage() {
        let err = RoutingGraph::from_bytes(b"ASW\x03whatever".to_vec()).unwrap_err();
        assert!(
            err.to_string().contains("Unsupported ASW graph version 3"),
            "got: {err}"
        );
        assert!(RoutingGraph::from_bytes(vec![4, 0, 0, 0]).is_err());
        assert!(RoutingGraph::from_bytes(Vec::new()).is_err());
    }

    #[test]
    fn from_bytes_rejects_truncated_file() {
        let bytes = square_graph_bytes();
        let cut = bytes[..bytes.len() - 8].to_vec();
        let err = RoutingGraph::from_bytes(cut).unwrap_err();
        assert!(err.to_string().contains("beyond end of file"), "got: {err}");
    }

    #[test]
    fn from_bytes_rejects_misaligned_section() {
        let mut bytes = square_graph_bytes();
        // Section 0 offset lives at header byte 88; nudge it by 4.
        let off = u64::from_le_bytes(bytes[88..96].try_into().unwrap());
        bytes[88..96].copy_from_slice(&(off + 4).to_le_bytes());
        let err = RoutingGraph::from_bytes(bytes).unwrap_err();
        assert!(err.to_string().contains("aligned"), "got: {err}");
    }

    #[test]
    fn from_bytes_rejects_wrong_section_length() {
        let mut bytes = square_graph_bytes();
        // num_nodes at byte 72: claim one node more than stored.
        let n = u32::from_le_bytes(bytes[72..76].try_into().unwrap());
        bytes[72..76].copy_from_slice(&(n + 1).to_le_bytes());
        assert!(RoutingGraph::from_bytes(bytes).is_err());
    }

    /// Corrupt-but-well-formed files: section lengths match the counts, but
    /// a CSR table is not monotonic. Must fail at open, not panic at query.
    #[test]
    fn from_bytes_rejects_non_monotonic_csr_tables() {
        let bytes = square_graph_bytes();
        let g = RoutingGraph::from_bytes(bytes.clone()).unwrap();
        let sec = |i: usize| {
            let off = u64::from_le_bytes(bytes[88 + i * 16..96 + i * 16].try_into().unwrap());
            off as usize
        };
        // offsets[0] must be 0: write 1 into it.
        let mut b1 = bytes.clone();
        let o = sec(1);
        b1[o..o + 4].copy_from_slice(&1u32.to_le_bytes());
        assert!(RoutingGraph::from_bytes(b1).is_err(), "offsets[0] != 0");
        // grid_offsets: make cell 1 smaller than cell 0 (non-monotonic).
        let mut b2 = bytes.clone();
        let o = sec(7);
        b2[o..o + 4].copy_from_slice(&5u32.to_le_bytes());
        assert!(
            RoutingGraph::from_bytes(b2).is_err(),
            "grid_offsets not monotonic"
        );
        // coast_runs[0] must be 0.
        let o = sec(4);
        let mut b3 = bytes.clone();
        b3[o..o + 4].copy_from_slice(&1u32.to_le_bytes());
        assert!(RoutingGraph::from_bytes(b3).is_err(), "coast_runs[0] != 0");
        drop(g);
    }

    #[test]
    #[should_panic(expected = "strictly ascending")]
    fn builder_panics_on_unsorted_nodes() {
        let c0 = h3o::LatLng::new(0.0, 0.0)
            .unwrap()
            .to_cell(h3o::Resolution::Five);
        let c1 = h3o::LatLng::new(1.0, 1.0)
            .unwrap()
            .to_cell(h3o::Resolution::Five);
        let (lo, hi) = (
            u64::from(c0).min(u64::from(c1)),
            u64::from(c0).max(u64::from(c1)),
        );
        let mut b = GraphBuilder::default();
        b.add_node(hi, 255);
        b.add_node(lo, 255);
        let _ = b.build_bytes();
    }

    #[test]
    fn neighbor_weights_are_centre_to_centre_haversine() {
        let g = square_graph();
        for n in 0..g.num_nodes() {
            let (lat, lon) = g.node_pos(n);
            for (t, w) in g.neighbors(n) {
                let (tlat, tlon) = g.node_pos(t);
                let expected = crate::h3::haversine_nm(lat, lon, tlat, tlon);
                assert!(
                    (w as f64 - expected).abs() < 1e-3,
                    "edge {n}->{t}: {w} vs {expected}"
                );
            }
        }
    }

    #[test]
    fn neighbor_ids_match_neighbors_without_decoding() {
        let g = square_graph();
        for n in 0..g.num_nodes() {
            let ids: Vec<u32> = g.neighbor_ids(n).collect();
            let full: Vec<u32> = g.neighbors(n).map(|(t, _)| t).collect();
            assert_eq!(ids, full);
        }
    }

    #[test]
    fn res13_edge_weight_is_true_distance() {
        let center = h3o::LatLng::new(9.08, -79.68)
            .unwrap()
            .to_cell(h3o::Resolution::Thirteen);
        let neighbor = crate::h3::neighbors(center)[0];
        let (lat0, lon0) = crate::h3::cell_center(center);
        let (lat1, lon1) = crate::h3::cell_center(neighbor);
        let true_dist_nm = crate::h3::haversine_nm(lat0, lon0, lat1, lon1);
        assert!(true_dist_nm < 0.005);

        let mut cells = [u64::from(center), u64::from(neighbor)];
        cells.sort_unstable();
        let mut b = GraphBuilder::default();
        let n0 = b.add_node(cells[0], 255);
        let n1 = b.add_node(cells[1], 255);
        b.add_edge(n0, n1);
        let g = b.build();
        let (_, w) = g.neighbors(n0).next().unwrap();
        assert!(w > 0.0, "canal edges must never be free");
        assert!((w as f64 - true_dist_nm).abs() < 1e-6);
    }

    #[test]
    fn quantize_rounds_down_and_saturates() {
        assert_eq!(quantize_shore_dist(0.0), 0);
        assert_eq!(quantize_shore_dist(0.019), 0); // rounds down, not nearest
        assert_eq!(quantize_shore_dist(0.02), 1);
        assert_eq!(quantize_shore_dist(0.199), 9); // 9.95 -> 9
        assert_eq!(quantize_shore_dist(5.1), 255);
        assert_eq!(quantize_shore_dist(99.0), 255); // saturates
        assert_eq!(quantize_shore_dist(-1.0), 0); // clamps
    }

    #[test]
    fn prune_keeps_main_component_and_shore_dist() {
        // 3-node chain (main) + 1 isolated node, distinct shore_dist values.
        let coords = [(0.0, 0.0), (1.0, 0.0), (2.0, 0.0), (10.0, 10.0)];
        let mut entries: Vec<(u64, f64, f64, u8)> = coords
            .iter()
            .enumerate()
            .map(|(i, &(lat, lng))| {
                let cell = h3o::LatLng::new(lat, lng)
                    .unwrap()
                    .to_cell(h3o::Resolution::Five);
                (u64::from(cell), lat, lng, (i as u8 + 1) * 10) // 10,20,30,40
            })
            .collect();
        entries.sort_by_key(|(h3, _, _, _)| *h3);

        let mut b = GraphBuilder::default();
        let mut ids = Vec::new();
        for &(h3, _, _, q) in &entries {
            ids.push(b.add_node(h3, q));
        }
        // Chain the first three entries (by sorted order); leave the last isolated.
        b.add_edge(ids[0], ids[1]);
        b.add_edge(ids[1], ids[2]);
        let pruned = b.prune_to_main_component().build();
        assert_eq!(pruned.num_nodes(), 3);
        // Every surviving node keeps the shore_dist of the entry with its H3 index.
        for (i, &h3) in pruned.node_h3s().iter().enumerate() {
            let orig = entries.iter().find(|e| e.0 == h3).unwrap();
            assert_eq!(
                pruned.shore_dist(i as u32),
                orig.3,
                "node {i} shore_dist mismatch"
            );
        }
    }

    #[test]
    fn shore_dist_survives_save_load_roundtrip() {
        // Reuse the same construction pattern as the existing roundtrip test,
        // but with distinct shore_dist values per node.
        let c0 = h3o::LatLng::new(0.0, 0.0)
            .unwrap()
            .to_cell(h3o::Resolution::Five);
        let c1 = h3o::LatLng::new(5.0, 5.0)
            .unwrap()
            .to_cell(h3o::Resolution::Five);
        let mut cells = vec![
            (u64::from(c0), 0.0, 0.0, 7u8),
            (u64::from(c1), 5.0, 5.0, 200u8),
        ];
        cells.sort_by_key(|(h3, _, _, _)| *h3);

        let mut b = GraphBuilder::default();
        let mut ids = Vec::new();
        for &(h3, _, _, q) in &cells {
            ids.push(b.add_node(h3, q));
        }
        b.add_edge(ids[0], ids[1]);
        let loaded = RoutingGraph::from_bytes(b.build_bytes()).unwrap();
        let q: Vec<u8> = (0..loaded.num_nodes())
            .map(|i| loaded.shore_dist(i))
            .collect();
        let expected: Vec<u8> = cells.iter().map(|c| c.3).collect();
        assert_eq!(q, expected);
    }
}
