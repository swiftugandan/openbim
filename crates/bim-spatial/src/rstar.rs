//! R*-tree (Beckmann, Kriegel, Schneider, Seeger 1990) with STR bulk loading
//! (Leutenegger, Lopez, Edgington 1997).

use crate::aabb::Aabb;
use std::cmp::Ordering;
use std::collections::BinaryHeap;

/// Maximum entries per node.
const MAX: usize = 16;
/// Minimum entries per non-root node (40% of MAX, as recommended for the R*-tree).
const MIN: usize = 6;
/// Entries removed for forced reinsertion on the first overflow of a level (30% of MAX).
const REINSERT: usize = 5;

trait Bounded {
    fn bbox(&self) -> Aabb;
}

impl<T> Bounded for (Aabb, T) {
    #[inline]
    fn bbox(&self) -> Aabb {
        self.0
    }
}

impl<T> Bounded for Node<T> {
    #[inline]
    fn bbox(&self) -> Aabb {
        self.bbox
    }
}

#[derive(Clone, Debug)]
struct Node<T> {
    bbox: Aabb,
    kids: Kids<T>,
}

#[derive(Clone, Debug)]
enum Kids<T> {
    /// Level 0: the stored entries.
    Leaf(Vec<(Aabb, T)>),
    /// Level > 0: child nodes, all one level lower.
    Inner(Vec<Node<T>>),
}

/// Something to be placed into a node of a given level: an item goes into a leaf
/// (level 0), a node of level `l` goes into a node of level `l + 1`.
enum Entry<T> {
    Item(Aabb, T),
    Node(Node<T>),
}

impl<T> Entry<T> {
    fn bbox(&self) -> Aabb {
        match self {
            Entry::Item(b, _) => *b,
            Entry::Node(n) => n.bbox,
        }
    }
}

fn bbox_of<E: Bounded>(v: &[E]) -> Aabb {
    v.iter().fold(Aabb::EMPTY, |acc, e| acc.union(&e.bbox()))
}

impl<T> Node<T> {
    fn leaf(items: Vec<(Aabb, T)>) -> Self {
        Node { bbox: bbox_of(&items), kids: Kids::Leaf(items) }
    }

    fn inner(children: Vec<Node<T>>) -> Self {
        Node { bbox: bbox_of(&children), kids: Kids::Inner(children) }
    }

    fn len(&self) -> usize {
        match &self.kids {
            Kids::Leaf(v) => v.len(),
            Kids::Inner(v) => v.len(),
        }
    }

    fn recompute_bbox(&mut self) {
        self.bbox = match &self.kids {
            Kids::Leaf(v) => bbox_of(v),
            Kids::Inner(v) => bbox_of(v),
        };
    }

    /// Moves all children of this node (which sits at `own_level`) out as entries, each
    /// tagged with the level of the node it must be inserted into: items target level
    /// 0, child nodes (level `own_level - 1`) target `own_level`.
    fn into_entries(self, own_level: usize, out: &mut Vec<(Entry<T>, usize)>) {
        match self.kids {
            Kids::Leaf(v) => out.extend(v.into_iter().map(|(b, t)| (Entry::Item(b, t), 0))),
            Kids::Inner(v) => out.extend(v.into_iter().map(|n| (Entry::Node(n), own_level))),
        }
    }
}

/// An R*-tree mapping closed boxes to items.
///
/// * [`RStarTree::insert`]: R* ChooseSubtree (minimum overlap enlargement just above the
///   leaves, minimum volume enlargement higher up), forced reinsertion of the 30%
///   entries farthest from the node centre on the first overflow of each level during
///   one insertion, and the R* split (axis by minimum margin sum, distribution by
///   minimum overlap, then minimum volume) otherwise.
/// * [`RStarTree::bulk_load`]: Sort-Tile-Recursive packing.
/// * [`RStarTree::remove`]: finds the entry, condenses underfull nodes by reinserting
///   their entries, and shortens the tree when the root has a single child.
#[derive(Clone, Debug)]
pub struct RStarTree<T> {
    root: Node<T>,
    /// Level of the root (0 when the root is a leaf).
    height: usize,
    len: usize,
}

impl<T> Default for RStarTree<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> RStarTree<T> {
    /// An empty tree.
    pub fn new() -> Self {
        RStarTree { root: Node::leaf(Vec::new()), height: 0, len: 0 }
    }

    /// Number of stored entries.
    #[inline]
    pub fn len(&self) -> usize {
        self.len
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Bounds of all entries (empty box for an empty tree).
    pub fn bounds(&self) -> Aabb {
        self.root.bbox
    }

    /// Builds a packed tree with Sort-Tile-Recursive: entries are sorted into
    /// `ceil(n/M)^(1/3)` slabs along x, each slab into runs along y, each run into
    /// nodes along z; the same packing is repeated on the node level until one node
    /// remains. Group sizes are balanced so every non-root node holds at least M/2.
    pub fn bulk_load(items: Vec<(Aabb, T)>) -> Self {
        let len = items.len();
        if len <= MAX {
            return RStarTree { root: Node::leaf(items), height: 0, len };
        }
        let mut nodes: Vec<Node<T>> = str_groups(items).into_iter().map(Node::leaf).collect();
        let mut height = 0;
        while nodes.len() > MAX {
            nodes = str_groups(nodes).into_iter().map(Node::inner).collect();
            height += 1;
        }
        RStarTree { root: Node::inner(nodes), height: height + 1, len }
    }

    /// Inserts `item` with bounding box `b`.
    pub fn insert(&mut self, b: Aabb, item: T) {
        self.insert_entry(Entry::Item(b, item), 0);
        self.len += 1;
    }

    /// Inserts `entry` into a node at level `target`, handling every forced reinsertion
    /// this triggers. The overflow-treatment flags are shared by all of them, so each
    /// level is reinserted at most once per call.
    fn insert_entry(&mut self, entry: Entry<T>, target: usize) {
        let mut reinserted = vec![false; self.height + 1];
        let mut pending = vec![(entry, target)];
        while let Some((e, t)) = pending.pop() {
            if t > self.height {
                // Only after the tree shrank under a pending orphan: insert its contents.
                match e {
                    Entry::Node(n) => n.into_entries(t - 1, &mut pending),
                    Entry::Item(..) => unreachable!("items always target level 0"),
                }
                continue;
            }
            let h = self.height;
            if let Some(sibling) = insert_rec(&mut self.root, h, h, e, t, &mut reinserted, &mut pending) {
                let old = std::mem::replace(&mut self.root, Node::leaf(Vec::new()));
                self.root = Node::inner(vec![old, sibling]);
                self.height += 1;
                reinserted.push(false);
            }
        }
    }

    /// Removes one entry whose box equals `b` and whose item equals `item`. Returns
    /// whether such an entry existed.
    pub fn remove(&mut self, b: &Aabb, item: &T) -> bool
    where
        T: PartialEq,
    {
        let mut orphans = Vec::new();
        if !remove_rec(&mut self.root, self.height, b, item, &mut orphans) {
            return false;
        }
        self.len -= 1;
        if let Kids::Inner(v) = &self.root.kids {
            if v.is_empty() {
                self.root = Node::leaf(Vec::new());
                self.height = 0;
            }
        }
        // Reinsert the entries of eliminated (underfull) nodes at their original level.
        for (e, t) in orphans {
            self.insert_entry(e, t);
        }
        // Shorten the tree while the root has a single child.
        loop {
            let single = match &mut self.root.kids {
                Kids::Inner(v) if v.len() == 1 => v.pop(),
                _ => None,
            };
            match single {
                Some(child) => {
                    self.root = child;
                    self.height -= 1;
                }
                None => break,
            }
        }
        true
    }

    /// Every item whose box intersects `q` (closed: touching counts), each once.
    pub fn query_intersects(&self, q: &Aabb) -> Vec<&T> {
        let mut out = Vec::new();
        if !self.root.bbox.intersects(q) {
            return out;
        }
        let mut stack: Vec<&Node<T>> = vec![&self.root];
        while let Some(n) = stack.pop() {
            match &n.kids {
                Kids::Leaf(v) => out.extend(v.iter().filter(|(b, _)| b.intersects(q)).map(|(_, t)| t)),
                Kids::Inner(v) => stack.extend(v.iter().filter(|c| c.bbox.intersects(q))),
            }
        }
        out
    }

    /// The `min(k, len)` items closest to `p`, by ascending Euclidean distance from `p`
    /// to their box (0 when `p` is inside). Best-first search over a priority queue
    /// keyed by the minimum distance of nodes and entries, so the order is exact.
    pub fn nearest(&self, p: [f64; 3], k: usize) -> Vec<&T> {
        let mut out = Vec::with_capacity(k.min(self.len));
        if k == 0 || self.len == 0 {
            return out;
        }
        let mut heap: BinaryHeap<Cand<'_, T>> = BinaryHeap::new();
        heap.push(Cand { d2: self.root.bbox.distance2_to_point(p), what: What::Node(&self.root) });
        while let Some(c) = heap.pop() {
            match c.what {
                What::Item(t) => {
                    out.push(t);
                    if out.len() == k {
                        break;
                    }
                }
                What::Node(n) => match &n.kids {
                    Kids::Leaf(v) => {
                        for (b, t) in v {
                            heap.push(Cand { d2: b.distance2_to_point(p), what: What::Item(t) });
                        }
                    }
                    Kids::Inner(v) => {
                        for c in v {
                            heap.push(Cand { d2: c.bbox.distance2_to_point(p), what: What::Node(c) });
                        }
                    }
                },
            }
        }
        out
    }

    /// Iterates over all `(box, item)` entries in tree order.
    pub fn iter(&self) -> impl Iterator<Item = (&Aabb, &T)> {
        let mut stack: Vec<&Node<T>> = vec![&self.root];
        let mut cur: std::slice::Iter<'_, (Aabb, T)> = [].iter();
        std::iter::from_fn(move || loop {
            if let Some((b, t)) = cur.next() {
                return Some((b, t));
            }
            let n = stack.pop()?;
            match &n.kids {
                Kids::Leaf(v) => cur = v.iter(),
                Kids::Inner(v) => stack.extend(v.iter()),
            }
        })
    }

    #[cfg(test)]
    fn check_invariants(&self) {
        fn rec<T>(n: &Node<T>, level: usize, is_root: bool, count: &mut usize) {
            assert_eq!(n.bbox, match &n.kids {
                Kids::Leaf(v) => bbox_of(v),
                Kids::Inner(v) => bbox_of(v),
            });
            assert!(n.len() <= MAX);
            if !is_root {
                assert!(n.len() >= MIN, "underfull node: {}", n.len());
            }
            match &n.kids {
                Kids::Leaf(v) => {
                    assert_eq!(level, 0, "leaves must all be at level 0");
                    *count += v.len();
                }
                Kids::Inner(v) => {
                    assert!(level > 0);
                    if is_root {
                        assert!(v.len() >= 2, "inner root needs two children");
                    }
                    for c in v {
                        rec(c, level - 1, false, count);
                    }
                }
            }
        }
        let mut count = 0;
        rec(&self.root, self.height, true, &mut count);
        assert_eq!(count, self.len);
    }
}

enum What<'a, T> {
    Node(&'a Node<T>),
    Item(&'a T),
}

struct Cand<'a, T> {
    d2: f64,
    what: What<'a, T>,
}

impl<T> PartialEq for Cand<'_, T> {
    fn eq(&self, o: &Self) -> bool {
        self.cmp(o) == Ordering::Equal
    }
}
impl<T> Eq for Cand<'_, T> {}
impl<T> PartialOrd for Cand<'_, T> {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}
impl<T> Ord for Cand<'_, T> {
    /// Reversed so the max-heap pops the smallest distance; at equal distance items
    /// pop before nodes (any order would be exact, this just finishes sooner).
    fn cmp(&self, o: &Self) -> Ordering {
        o.d2.total_cmp(&self.d2).then_with(|| {
            let rank = |w: &What<'_, T>| matches!(w, What::Item(_)) as u8;
            rank(&self.what).cmp(&rank(&o.what))
        })
    }
}

/// Inserts `entry` below `node` (at `level`) into a node at level `target`. Returns a
/// new sibling when `node` was split; entries evicted for forced reinsertion are
/// appended to `pending`.
fn insert_rec<T>(
    node: &mut Node<T>,
    level: usize,
    root_level: usize,
    entry: Entry<T>,
    target: usize,
    reinserted: &mut [bool],
    pending: &mut Vec<(Entry<T>, usize)>,
) -> Option<Node<T>> {
    if level == target {
        match (&mut node.kids, entry) {
            (Kids::Leaf(v), Entry::Item(b, t)) => v.push((b, t)),
            (Kids::Inner(v), Entry::Node(n)) => v.push(n),
            _ => unreachable!("entry kind does not match node level"),
        }
    } else {
        let eb = entry.bbox();
        let Kids::Inner(children) = &mut node.kids else { unreachable!("leaf above target level") };
        let i = choose_subtree(children, &eb, level == 1);
        if let Some(sibling) = insert_rec(&mut children[i], level - 1, root_level, entry, target, reinserted, pending) {
            children.push(sibling);
        }
    }

    if node.len() <= MAX {
        node.recompute_bbox();
        return None;
    }
    // Overflow treatment.
    if level != root_level && !reinserted[level] {
        reinserted[level] = true;
        let center = node.bbox.center();
        match &mut node.kids {
            Kids::Leaf(v) => {
                for e in evict_farthest(v, center) {
                    pending.push((Entry::Item(e.0, e.1), level));
                }
            }
            Kids::Inner(v) => {
                for n in evict_farthest(v, center) {
                    pending.push((Entry::Node(n), level));
                }
            }
        }
        node.recompute_bbox();
        return None;
    }
    let sibling = match &mut node.kids {
        Kids::Leaf(v) => Node::leaf(rstar_split(v)),
        Kids::Inner(v) => Node::inner(rstar_split(v)),
    };
    node.recompute_bbox();
    Some(sibling)
}

/// Removes the `REINSERT` entries whose centres are farthest from `center` and returns
/// them ordered from farthest to closest, so popping the pending stack reinserts the
/// closest first ("close reinsert").
fn evict_farthest<E: Bounded>(v: &mut Vec<E>, center: [f64; 3]) -> Vec<E> {
    let dist = |e: &E| {
        let c = e.bbox().center();
        (0..3).map(|k| (c[k] - center[k]) * (c[k] - center[k])).sum::<f64>()
    };
    // Closest first, so the farthest are at the tail.
    v.sort_by(|a, b| dist(a).total_cmp(&dist(b)));
    let mut out = v.split_off(v.len() - REINSERT);
    out.reverse();
    out
}

/// R* ChooseSubtree. Just above the leaves: minimum overlap enlargement, then minimum
/// volume enlargement, then minimum volume. Higher up: minimum volume enlargement, then
/// minimum volume. Margin (enlargement, then size) breaks the remaining ties, which
/// matters for degenerate (zero-volume) boxes.
fn choose_subtree<T>(children: &[Node<T>], b: &Aabb, children_are_leaves: bool) -> usize {
    let mut best = 0;
    let mut best_key = [f64::INFINITY; 5];
    for (i, c) in children.iter().enumerate() {
        let grown = c.bbox.union(b);
        let vol = c.bbox.volume();
        let vol_enl = grown.volume() - vol;
        let margin = c.bbox.margin();
        let margin_enl = grown.margin() - margin;
        let overlap_enl = if children_are_leaves {
            let mut d = 0.0;
            for (j, o) in children.iter().enumerate() {
                if j != i {
                    d += grown.overlap_volume(&o.bbox) - c.bbox.overlap_volume(&o.bbox);
                }
            }
            d
        } else {
            0.0
        };
        let key = [overlap_enl, vol_enl, vol, margin_enl, margin];
        if key.partial_cmp(&best_key) == Some(Ordering::Less) {
            best_key = key;
            best = i;
        }
    }
    best
}

/// R* split of an overflowing node (`MAX + 1` entries): the axis with the smallest sum
/// of margins over all candidate distributions (both sorts, by lower and by upper
/// bound) is chosen; along it, the distribution with the least overlap, then the least
/// total volume, wins. `v` keeps the first group; the second is returned.
fn rstar_split<E: Bounded>(v: &mut Vec<E>) -> Vec<E> {
    let n = v.len();
    let lo = MIN.min(n / 2).max(1);
    let hi = n - lo; // first-group sizes lo..=hi
    let mut pre = vec![Aabb::EMPTY; n + 1];
    let mut suf = vec![Aabb::EMPTY; n + 1];
    let fill = |v: &[E], pre: &mut [Aabb], suf: &mut [Aabb]| {
        for i in 0..n {
            pre[i + 1] = pre[i].union(&v[i].bbox());
        }
        for i in (0..n).rev() {
            suf[i] = suf[i + 1].union(&v[i].bbox());
        }
    };
    let sort = |v: &mut [E], axis: usize, by_upper: bool| {
        if by_upper {
            v.sort_by(|a, b| {
                let (a, b) = (a.bbox(), b.bbox());
                a.max[axis].total_cmp(&b.max[axis]).then(a.min[axis].total_cmp(&b.min[axis]))
            });
        } else {
            v.sort_by(|a, b| {
                let (a, b) = (a.bbox(), b.bbox());
                a.min[axis].total_cmp(&b.min[axis]).then(a.max[axis].total_cmp(&b.max[axis]))
            });
        }
    };

    // ChooseSplitAxis.
    let mut best_axis = 0;
    let mut best_margin = f64::INFINITY;
    for axis in 0..3 {
        let mut s = 0.0;
        for by_upper in [false, true] {
            sort(v, axis, by_upper);
            fill(v, &mut pre, &mut suf);
            for k in lo..=hi {
                s += pre[k].margin() + suf[k].margin();
            }
        }
        if s < best_margin {
            best_margin = s;
            best_axis = axis;
        }
    }

    // ChooseSplitIndex.
    let mut best = (f64::INFINITY, f64::INFINITY, false, lo);
    for by_upper in [false, true] {
        sort(v, best_axis, by_upper);
        fill(v, &mut pre, &mut suf);
        for k in lo..=hi {
            let overlap = pre[k].overlap_volume(&suf[k]);
            let vol = pre[k].volume() + suf[k].volume();
            if (overlap, vol) < (best.0, best.1) {
                best = (overlap, vol, by_upper, k);
            }
        }
    }
    sort(v, best_axis, best.2);
    v.split_off(best.3)
}

/// Sort-Tile-Recursive grouping of `v` into `ceil(n / MAX)` groups of balanced size.
fn str_groups<E: Bounded>(mut v: Vec<E>) -> Vec<Vec<E>> {
    let n = v.len();
    let g = n.div_ceil(MAX);
    // Group i covers v[bound(i)..bound(i + 1)].
    let bound = |i: usize| i * n / g;
    let key = |e: &E, axis: usize| {
        let b = e.bbox();
        b.min[axis] + b.max[axis]
    };
    let slabs = ((g as f64).cbrt().ceil() as usize).clamp(1, g);
    v.sort_unstable_by(|a, b| key(a, 0).total_cmp(&key(b, 0)));
    for j in 0..slabs {
        let (g0, g1) = (j * g / slabs, (j + 1) * g / slabs);
        if g0 == g1 {
            continue;
        }
        v[bound(g0)..bound(g1)].sort_unstable_by(|a, b| key(a, 1).total_cmp(&key(b, 1)));
        let gs = g1 - g0;
        let runs = ((gs as f64).sqrt().ceil() as usize).clamp(1, gs);
        for r in 0..runs {
            let (h0, h1) = (g0 + r * gs / runs, g0 + (r + 1) * gs / runs);
            if h0 < h1 {
                v[bound(h0)..bound(h1)].sort_unstable_by(|a, b| key(a, 2).total_cmp(&key(b, 2)));
            }
        }
    }
    let mut groups = Vec::with_capacity(g);
    for i in (1..g).rev() {
        groups.push(v.split_off(bound(i)));
    }
    groups.push(v);
    groups.reverse();
    groups
}

/// Removes one matching entry below `node` (at `level`). Children that become underfull
/// are cut out and their entries appended to `orphans` with their target level.
fn remove_rec<T: PartialEq>(
    node: &mut Node<T>,
    level: usize,
    b: &Aabb,
    item: &T,
    orphans: &mut Vec<(Entry<T>, usize)>,
) -> bool {
    match &mut node.kids {
        Kids::Leaf(v) => {
            let Some(i) = v.iter().position(|(bb, t)| bb == b && t == item) else { return false };
            v.swap_remove(i);
        }
        Kids::Inner(children) => {
            let mut found = false;
            for i in 0..children.len() {
                if !children[i].bbox.contains(b) {
                    continue;
                }
                if remove_rec(&mut children[i], level - 1, b, item, orphans) {
                    if children[i].len() < MIN {
                        let c = children.swap_remove(i);
                        c.into_entries(level - 1, orphans);
                    }
                    found = true;
                    break;
                }
            }
            if !found {
                return false;
            }
        }
    }
    node.recompute_bbox();
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u64) -> u64 {
        *seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        *seed >> 33
    }

    fn boxes(seed: &mut u64, n: usize) -> Vec<Aabb> {
        (0..n)
            .map(|_| {
                let min = [(lcg(seed) % 200) as f64 * 0.5, (lcg(seed) % 200) as f64 * 0.5, (lcg(seed) % 200) as f64 * 0.5];
                let s = [(lcg(seed) % 6) as f64 * 0.5, (lcg(seed) % 6) as f64 * 0.5, (lcg(seed) % 6) as f64 * 0.5];
                Aabb::new(min, [min[0] + s[0], min[1] + s[1], min[2] + s[2]])
            })
            .collect()
    }

    fn check(tree: &RStarTree<usize>, live: &[(Aabb, usize)], seed: &mut u64) {
        tree.check_invariants();
        for _ in 0..50 {
            let q = boxes(seed, 1)[0];
            let q = Aabb::new(q.min, [q.max[0] + 5.0, q.max[1] + 3.0, q.max[2] + 4.0]);
            let mut got: Vec<usize> = tree.query_intersects(&q).into_iter().copied().collect();
            got.sort();
            let mut want: Vec<usize> = live.iter().filter(|(b, _)| b.intersects(&q)).map(|(_, i)| *i).collect();
            want.sort();
            assert_eq!(got, want);

            let p = q.min;
            let k = 1 + (lcg(seed) % 20) as usize;
            let got: Vec<f64> = tree.nearest(p, k).iter().map(|i| live.iter().find(|e| e.1 == **i).unwrap().0.distance_to_point(p)).collect();
            let mut all: Vec<f64> = live.iter().map(|(b, _)| b.distance_to_point(p)).collect();
            all.sort_by(f64::total_cmp);
            all.truncate(k);
            assert_eq!(got, all);
        }
    }

    #[test]
    fn incremental_insert_remove_matches_brute_force() {
        let mut seed = 1;
        let mut tree = RStarTree::new();
        let bs = boxes(&mut seed, 3000);
        let mut live = Vec::new();
        for (i, b) in bs.iter().enumerate() {
            tree.insert(*b, i);
            live.push((*b, i));
        }
        assert_eq!(tree.len(), 3000);
        check(&tree, &live, &mut seed);
        for i in (0..3000).filter(|i| i % 3 != 0) {
            assert!(tree.remove(&bs[i], &i));
        }
        assert!(!tree.remove(&bs[1], &1));
        assert!(!tree.remove(&Aabb::new([-1.0; 3], [0.0; 3]), &0));
        live.retain(|(_, i)| i % 3 == 0);
        assert_eq!(tree.len(), live.len());
        check(&tree, &live, &mut seed);
        for i in (0..3000).filter(|i| i % 3 == 0) {
            assert!(tree.remove(&bs[i], &i));
        }
        assert!(tree.is_empty());
        tree.check_invariants();
        assert!(tree.query_intersects(&Aabb::new([-1e9; 3], [1e9; 3])).is_empty());
        assert!(tree.nearest([0.0; 3], 4).is_empty());
    }

    #[test]
    fn bulk_load_then_mutate() {
        let mut seed = 2;
        let bs = boxes(&mut seed, 5000);
        let mut live: Vec<(Aabb, usize)> = bs.iter().copied().zip(0..).collect();
        let mut tree = RStarTree::bulk_load(live.clone());
        assert_eq!(tree.len(), 5000);
        assert_eq!(tree.iter().count(), 5000);
        check(&tree, &live, &mut seed);
        for i in (0..5000).step_by(2) {
            assert!(tree.remove(&bs[i], &i));
        }
        live.retain(|(_, i)| i % 2 == 1);
        for (j, b) in boxes(&mut seed, 1000).into_iter().enumerate() {
            tree.insert(b, 10_000 + j);
            live.push((b, 10_000 + j));
        }
        assert_eq!(tree.len(), live.len());
        check(&tree, &live, &mut seed);
    }

    #[test]
    fn duplicates_touching_and_small_trees() {
        let mut tree = RStarTree::new();
        let b = Aabb::new([0.0; 3], [1.0; 3]);
        for i in 0..40 {
            tree.insert(b, i % 2); // many identical entries
        }
        tree.check_invariants();
        assert_eq!(tree.query_intersects(&Aabb::from_point([1.0, 1.0, 1.0])).len(), 40);
        assert_eq!(tree.nearest([0.5; 3], 100).len(), 40);
        for _ in 0..20 {
            assert!(tree.remove(&b, &0));
        }
        assert!(!tree.remove(&b, &0));
        assert_eq!(tree.len(), 20);
        tree.check_invariants();
        let small = RStarTree::bulk_load(vec![(b, 'a'), (Aabb::new([2.0; 3], [3.0; 3]), 'b')]);
        assert_eq!(small.nearest([2.5; 3], 5), vec![&'b', &'a']);
        assert!(RStarTree::<u8>::bulk_load(Vec::new()).is_empty());
    }
}
