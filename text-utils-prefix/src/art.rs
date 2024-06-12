use std::{
    collections::HashMap,
    hash::Hash,
    iter::{empty, once},
    sync::Arc,
};

use serde::{Deserialize, Serialize};
use serde_with::serde_as;

use crate::PrefixSearch;

type Index<const N: usize> = Box<[u8; N]>;
type Children<V, const N: usize> = Box<[Option<Box<Node<V>>>; N]>;

#[serde_as]
#[derive(Debug, Serialize, Deserialize)]
enum NodeType<V> {
    Leaf(V),
    N4(Index<4>, Children<V, 4>, u8),
    N16(Index<16>, Children<V, 16>, u8),
    // N48(Index<256>, Children<V, 48>, u8),
    N48 {
        #[serde_as(as = "Box<[_; 256]>")]
        index: Index<256>,
        #[serde_as(as = "Box<[_; 48]>")]
        children: Children<V, 48>,
        num_children: u8,
    },
    N256 {
        #[serde_as(as = "Box<[_; 256]>")]
        children: Children<V, 256>,
        num_children: u16,
    }, // N256(Children<V, 256>, u16),
}

#[derive(Debug, Serialize, Deserialize)]
struct Node<V> {
    prefix: Box<[u8]>,
    inner: NodeType<V>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PathType {
    Key,
    Full,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AdaptiveRadixTrie<V> {
    root: Option<Node<V>>,
}

#[derive(Debug, Default)]
pub struct AdaptiveRadixTrieStats {
    pub max_depth: usize,
    pub avg_depth: f32,
    pub num_nodes: usize,
    pub num_keys: usize,
    pub node_info: HashMap<String, (usize, f32)>,
}

impl<V> AdaptiveRadixTrie<V> {
    pub fn stats(&self) -> AdaptiveRadixTrieStats {
        let mut dist = HashMap::from_iter(
            ["leaf", "n4", "n16", "n48", "n256"]
                .iter()
                .map(|&s| (s.to_string(), (0, 0.0))),
        );
        let Some(root) = &self.root else {
            return AdaptiveRadixTrieStats::default();
        };
        let mut stack = vec![(root, 0)];
        let mut max_depth = 0;
        let mut avg_depth = (0, 0.0);
        while let Some((node, depth)) = stack.pop() {
            max_depth = max_depth.max(depth);
            let name = match &node.inner {
                NodeType::Leaf(..) => "leaf",
                NodeType::N4(..) => "n4",
                NodeType::N16(..) => "n16",
                NodeType::N48 { .. } => "n48",
                NodeType::N256 { .. } => "n256",
            };
            let val = dist.get_mut(name).unwrap();
            val.0 += 1;
            let n = val.0 as f32;
            val.1 = (val.1 * (n - 1.0) + node.prefix.len() as f32) / n;
            avg_depth.0 += 1;
            let n = avg_depth.0 as f32;
            avg_depth.1 = (avg_depth.1 * (n - 1.0) + depth as f32) / n;
            stack.extend(node.children().map(|(_, child)| (child, depth + 1)));
        }
        AdaptiveRadixTrieStats {
            max_depth,
            avg_depth: avg_depth.1,
            num_nodes: dist.iter().map(|(_, (n, _))| n).sum(),
            num_keys: dist["leaf"].0,
            node_info: dist,
        }
    }
}

impl<V> Default for AdaptiveRadixTrie<V> {
    fn default() -> Self {
        Self { root: None }
    }
}

enum Matching {
    FullKey(usize),
    FullPrefix(u8),
    Exact,
    Partial(usize, u8),
}

impl<V> Node<V> {
    fn new_leaf(prefix: Vec<u8>, value: V) -> Self {
        Self {
            prefix: prefix.into_boxed_slice(),
            inner: NodeType::Leaf(value),
        }
    }

    fn new_inner(prefix: Vec<u8>) -> Self {
        Self {
            prefix: prefix.into_boxed_slice(),
            inner: NodeType::N4(
                Box::new(std::array::from_fn(|_| 0)),
                Box::new(std::array::from_fn(|_| None)),
                0,
            ),
        }
    }

    #[inline]
    fn is_leaf(&self) -> bool {
        matches!(self.inner, NodeType::Leaf(_))
    }

    #[inline]
    fn is_inner(&self) -> bool {
        !self.is_leaf()
    }

    #[inline]
    fn matching(&self, key: &mut impl Iterator<Item = u8>, offset: usize) -> Matching {
        let mut i = offset;
        while i < self.prefix.len() {
            let Some(k) = key.next() else {
                return Matching::FullKey(i);
            };
            if k != self.prefix[i] {
                return Matching::Partial(i, k);
            }
            i += 1;
        }
        if let Some(k) = key.next() {
            Matching::FullPrefix(k)
        } else {
            Matching::Exact
        }
    }

    #[inline]
    fn find_iter(&self, mut key: impl Iterator<Item = u8>) -> Option<&Self> {
        let mut node = self;
        loop {
            if node.is_leaf() {
                if let Matching::Exact = node.matching(&mut key, 0) {
                    return Some(node);
                }
                break;
            }

            let Matching::FullPrefix(k) = node.matching(&mut key, 0) else {
                // if we dont match the full node prefix,
                // we can return early
                return None;
            };

            let Some(child) = node.find_child(k) else {
                break;
            };
            node = child;
        }
        None
    }

    #[inline]
    fn has_child(&self, key: u8) -> bool {
        self.find_child(key).is_some()
    }

    fn children(&self) -> Box<dyn Iterator<Item = (u8, &Self)> + '_> {
        match &self.inner {
            NodeType::Leaf(_) => Box::new(empty()),
            NodeType::N4(keys, children, num_children) => Box::new(
                keys[..*num_children as usize]
                    .iter()
                    .copied()
                    .zip(&children[..*num_children as usize])
                    .map(|(k, child)| (k, child.as_deref().unwrap())),
            ),
            NodeType::N16(keys, children, num_children) => Box::new(
                keys[..*num_children as usize]
                    .iter()
                    .copied()
                    .zip(&children[..*num_children as usize])
                    .map(|(k, child)| (k, child.as_deref().unwrap())),
            ),
            NodeType::N48 {
                index,
                children,
                num_children,
            } => Box::new(index.iter().enumerate().filter_map(|(i, &idx)| {
                if idx < *num_children {
                    Some((i as u8, children[idx as usize].as_deref().unwrap()))
                } else {
                    None
                }
            })),

            NodeType::N256 { children, .. } => Box::new(
                children
                    .iter()
                    .enumerate()
                    .filter_map(|(i, child)| child.as_deref().map(|child| (i as u8, child))),
            ),
        }
    }

    #[inline]
    fn set_child(&mut self, key: u8, child: Self) {
        // potentially upgrade the current node before insertion, will change
        // nothing if the node does not need to be upgraded
        assert!(self.find_child(key).is_none());
        self.upgrade();
        match &mut self.inner {
            NodeType::Leaf(_) => unreachable!("should not happen"),
            NodeType::N4(keys, children, num_children) => {
                // also keep sorted order for n4 for easier upgrade
                let n = *num_children as usize;
                let idx = keys[..n].binary_search(&key).unwrap_err();
                if idx < n {
                    keys[idx..].rotate_right(1);
                    children[idx..].rotate_right(1);
                }
                keys[idx] = key;
                children[idx] = Some(Box::new(child));
                *num_children += 1;
            }
            NodeType::N16(keys, children, num_children) => {
                let n = *num_children as usize;
                let idx = keys[..n].binary_search(&key).unwrap_err();
                if idx < n {
                    keys[idx..].rotate_right(1);
                    children[idx..].rotate_right(1);
                }
                keys[idx] = key;
                children[idx] = Some(Box::new(child));
                *num_children += 1;
            }
            NodeType::N48 {
                index,
                children,
                num_children,
            } => {
                index[key as usize] = *num_children;
                children[*num_children as usize] = Some(Box::new(child));
                *num_children += 1;
            }
            NodeType::N256 {
                children,
                num_children,
            } => {
                children[key as usize] = Some(Box::new(child));
                *num_children += 1;
            }
        }
    }

    #[inline]
    fn remove_child(&mut self, key: u8) -> Self {
        assert!(self.find_child(key).is_some());
        let child = match &mut self.inner {
            NodeType::Leaf(_) => unreachable!("should not happen"),
            NodeType::N4(keys, children, num_children) => {
                let n = *num_children as usize;
                let idx = keys[..n].binary_search(&key).unwrap();
                keys[idx..].rotate_left(1);
                let child = children[idx].take().unwrap();
                children[idx..].rotate_left(1);
                *num_children -= 1;
                child
            }
            NodeType::N16(keys, children, num_children) => {
                let n = *num_children as usize;
                let idx = keys[..n].binary_search(&key).unwrap();
                keys[idx..].rotate_left(1);
                let child = children[idx].take().unwrap();
                children[idx..].rotate_left(1);
                *num_children -= 1;
                child
            }
            NodeType::N48 {
                index,
                children,
                num_children,
            } => {
                let k = key as usize;
                let idx = index[k];
                index[k] = u8::MAX;
                index.iter_mut().for_each(|i| {
                    if *i < 48 && *i > idx {
                        *i -= 1;
                    }
                });
                let idx = idx as usize;
                let child = children[idx].take().unwrap();
                children[idx..].rotate_left(1);
                *num_children -= 1;
                child
            }
            NodeType::N256 {
                children,
                num_children,
            } => {
                *num_children -= 1;
                children[key as usize].take().unwrap()
            }
        };
        // potentially downgrade the current node after removal, will change
        // nothing if the node does not need to be downgraded
        self.downgrade();
        // also potentially merge the current node after removal with single
        // child (can only happen with N4)
        self.merge();
        *child
    }

    #[inline]
    fn contains_prefix_iter(
        &self,
        mut key: impl Iterator<Item = u8>,
        mut offset: usize,
    ) -> Option<(&Self, usize)> {
        let mut node = self;
        loop {
            let k = match node.matching(&mut key, offset) {
                Matching::FullKey(n) => return Some((node, n)),
                Matching::Exact => return Some((node, node.prefix.len())),
                Matching::FullPrefix(k) => k,
                Matching::Partial(..) => break,
            };
            // reset offset after first node
            offset = 0;

            let Some(child) = node.find_child(k) else {
                break;
            };
            node = child;
        }
        None
    }

    #[inline]
    fn find_child(&self, key: u8) -> Option<&Self> {
        match &self.inner {
            NodeType::Leaf(..) => None,
            NodeType::N4(keys, children, num_children) => {
                for i in 0..*num_children {
                    let i = i as usize;
                    if keys[i] == key {
                        return children[i].as_deref();
                    }
                }
                None
            }
            NodeType::N16(keys, children, num_children) => {
                let idx = keys[..*num_children as usize].binary_search(&key).ok()?;
                children[idx].as_deref()
            }
            NodeType::N48 {
                index, children, ..
            } => children.get(index[key as usize] as usize)?.as_deref(),
            NodeType::N256 { children, .. } => children[key as usize].as_deref(),
        }
    }

    #[inline]
    fn find_child_mut(&mut self, key: u8) -> Option<&mut Self> {
        match &mut self.inner {
            NodeType::Leaf(_) => None,
            NodeType::N4(keys, children, num_children) => {
                for i in 0..*num_children {
                    let i = i as usize;
                    if keys[i] == key {
                        return children[i].as_deref_mut();
                    }
                }
                None
            }
            NodeType::N16(keys, children, num_children) => {
                let idx = keys[..*num_children as usize].binary_search(&key).ok()?;
                children[idx].as_deref_mut()
            }
            NodeType::N48 {
                index, children, ..
            } => children
                .get_mut(index[key as usize] as usize)?
                .as_deref_mut(),
            NodeType::N256 { children, .. } => children[key as usize].as_deref_mut(),
        }
    }

    #[inline]
    fn upgrade(&mut self) {
        self.inner = match &mut self.inner {
            NodeType::N4(keys, children, num_children) if *num_children == 4 => {
                // just move over because n4 is also sorted
                NodeType::N16(
                    Box::new(std::array::from_fn(|i| if i < 4 { keys[i] } else { 0 })),
                    Box::new(std::array::from_fn(|i| {
                        if i < 4 {
                            assert!(children[i].is_some());
                            std::mem::take(&mut children[i])
                        } else {
                            None
                        }
                    })),
                    4,
                )
            }
            NodeType::N16(keys, children, num_children) if *num_children == 16 => {
                let mut index = [u8::MAX; 256];
                for (i, k) in keys.iter().enumerate() {
                    index[*k as usize] = i as u8;
                }
                NodeType::N48 {
                    index: Box::new(index),
                    children: Box::new(std::array::from_fn(|i| {
                        if i < 16 {
                            assert!(children[i].is_some());
                            std::mem::take(&mut children[i])
                        } else {
                            None
                        }
                    })),
                    num_children: 16,
                }
            }
            NodeType::N48 {
                index,
                children,
                num_children,
            } if *num_children == 48 => NodeType::N256 {
                children: Box::new(std::array::from_fn(|i| {
                    let idx = index[i];
                    if idx < 48 {
                        assert!(children[idx as usize].is_some());
                        std::mem::take(&mut children[idx as usize])
                    } else {
                        None
                    }
                })),
                num_children: 48,
            },
            _ => return,
        };
    }

    #[inline]
    fn downgrade(&mut self) {
        self.inner = match &mut self.inner {
            NodeType::N16(keys, children, num_children) if *num_children == 4 => NodeType::N4(
                Box::new(std::array::from_fn(|i| keys[i])),
                Box::new(std::array::from_fn(|i| children[i].take())),
                4,
            ),
            NodeType::N48 {
                index,
                children,
                num_children,
            } if *num_children == 16 => {
                let mut keys = [0; 16];
                let mut new_children = std::array::from_fn(|_| None);
                index
                    .iter()
                    .enumerate()
                    .filter(|(_, &idx)| idx < 48)
                    .enumerate()
                    .for_each(|(i, (k, idx))| {
                        keys[i] = k as u8;
                        new_children[i] = children[*idx as usize].take();
                    });
                assert!(keys[..15].iter().zip(keys[1..].iter()).all(|(a, b)| a < b));
                assert!(new_children.iter().all(|c| c.is_some()));
                NodeType::N16(Box::new(keys), Box::new(new_children), 16)
            }
            NodeType::N256 {
                children,
                num_children,
            } if *num_children == 48 => {
                let mut index = [u8::MAX; 256];
                let mut new_children = std::array::from_fn(|_| None);
                children
                    .iter_mut()
                    .enumerate()
                    .filter(|(_, child)| child.is_some())
                    .enumerate()
                    .for_each(|(i, (b, child))| {
                        index[b] = i as u8;
                        new_children[i] = child.take();
                    });
                assert!(new_children.iter().all(|c| c.is_some()));
                NodeType::N48 {
                    index: Box::new(index),
                    children: Box::new(new_children),
                    num_children: 48,
                }
            }
            _ => return,
        };
    }

    #[inline]
    fn merge(&mut self) {
        let (k, child) = match &mut self.inner {
            NodeType::N4(keys, children, num_children) if *num_children == 1 => {
                (keys[0], children[0].take().unwrap())
            }
            _ => return,
        };
        let new_prefix: Vec<_> = self
            .prefix
            .iter()
            .copied()
            .chain(once(k))
            .chain(child.prefix.iter().copied())
            .collect();
        self.prefix = new_prefix.into_boxed_slice();
        self.inner = child.inner;
    }

    #[inline]
    fn leaves(
        &self,
        mut path: Vec<u8>,
        path_type: PathType,
    ) -> Box<dyn Iterator<Item = (Vec<u8>, &V)> + '_> {
        if path_type == PathType::Full {
            path.extend(self.prefix.iter().copied());
        }
        if let NodeType::Leaf(value) = &self.inner {
            if path_type == PathType::Full {
                // dont keep last element (null byte) for full paths
                path.pop();
            }
            return Box::new(once((path, value)));
        }
        Box::new(self.children().flat_map(move |(k, child)| {
            let mut key = path.clone();
            key.push(k);
            child.leaves(key, path_type)
        }))
    }
}

impl<V> PrefixSearch for AdaptiveRadixTrie<V> {
    type Value = V;

    fn insert(&mut self, key: &[u8], value: V) -> Option<V> {
        let mut key = key.iter().filter(|&b| *b > 0).copied().chain(once(0));
        // empty tree
        let Some(root) = &mut self.root else {
            // insert leaf at root
            self.root = Some(Node::new_leaf(key.collect(), value));
            return None;
        };
        let mut node = root;
        loop {
            match node.matching(&mut key, 0) {
                Matching::FullKey(_) => unreachable!("should not happen"),
                Matching::FullPrefix(k) => {
                    // full prefix match, either go to next child
                    // or append leaf with rest of key
                    if node.has_child(k) {
                        node = node.find_child_mut(k).unwrap();
                        continue;
                    }
                    node.set_child(k, Node::new_leaf(key.collect(), value));
                    break;
                }
                Matching::Partial(n, k) => {
                    let inner_prefix = node.prefix[..n].to_vec();
                    let old_prefix = node.prefix[n + 1..].to_vec();
                    let p_k = node.prefix[n];

                    let mut old_node = std::mem::replace(node, Node::new_inner(inner_prefix));
                    old_node.prefix = old_prefix.into();
                    node.set_child(k, Node::new_leaf(key.collect(), value));
                    node.set_child(p_k, old_node);
                    break;
                }
                Matching::Exact => {
                    // exact match, only replace leaf value
                    let NodeType::Leaf(node_value) =
                        std::mem::replace(&mut node.inner, NodeType::Leaf(value))
                    else {
                        unreachable!("should not happen");
                    };
                    return Some(node_value);
                }
            };
        }
        None
    }

    fn delete(&mut self, key: &[u8]) -> Option<V> {
        let root = self.root.as_mut()?;

        // handle special case where root is leaf
        if root.is_leaf() {
            let Some(Node {
                inner: NodeType::Leaf(value),
                ..
            }) = self.root.take()
            else {
                unreachable!("should not happen");
            };
            return Some(value);
        }

        let mut node = root;
        let mut key = key.iter().filter(|&b| *b > 0).copied().chain(once(0));
        loop {
            let matching = node.matching(&mut key, 0);

            let Matching::FullPrefix(k) = matching else {
                // on inner nodes we always need full prefix matching
                return None;
            };

            // return if we dont find a child
            let child = node.find_child(k)?;

            // traverse down if child is inner
            if child.is_inner() {
                node = node.find_child_mut(k)?;
                continue;
            }

            // handle case if child is leaf
            let Matching::Exact = child.matching(&mut key, 0) else {
                break;
            };
            // key is an exact match for a leaf
            let Node {
                inner: NodeType::Leaf(value),
                ..
            } = node.remove_child(k)
            else {
                unreachable!("should not happen");
            };
            return Some(value);
        }
        None
    }

    fn get(&self, key: &[u8]) -> Option<&V> {
        let root = &self.root.as_ref()?;

        let key = key.iter().filter(|&b| *b > 0).copied().chain(once(0));
        root.find_iter(key).and_then(|node| match &node.inner {
            NodeType::Leaf(v) => Some(v),
            _ => None,
        })
    }

    fn contains(&self, prefix: &[u8]) -> bool {
        let Some(root) = &self.root else {
            return false;
        };

        let key = prefix.iter().filter(|&b| *b > 0).copied();
        root.contains_prefix_iter(key, 0).is_some()
    }

    fn values_along_path(&self, prefix: &[u8]) -> Vec<(usize, &Self::Value)> {
        let Some(root) = &self.root else {
            return vec![];
        };

        let mut path = vec![];
        let mut node = root;
        let mut key = prefix.iter().filter(|&b| *b > 0).copied();
        let mut i = 0;
        loop {
            match node.matching(&mut key, 0) {
                Matching::FullKey(_) => break,
                Matching::FullPrefix(k) => {
                    i += node.prefix.len();
                    if let Some(leaf) = node.find_child(0) {
                        let NodeType::Leaf(v) = &leaf.inner else {
                            unreachable!("should not happen");
                        };
                        path.push((i, v));
                    }
                    let Some(child) = node.find_child(k) else {
                        break;
                    };
                    node = child;
                    i += 1;
                }
                Matching::Exact => {
                    let Some(child) = node.find_child(0) else {
                        break;
                    };
                    let NodeType::Leaf(v) = &child.inner else {
                        unreachable!("should not happen");
                    };
                    path.push((i + node.prefix.len(), v));
                    break;
                }
                Matching::Partial(..) => break,
            };
        }
        path
    }

    fn continuations(&self, prefix: &[u8]) -> Box<dyn Iterator<Item = (Vec<u8>, &V)> + '_> {
        let Some(root) = &self.root else {
            return Box::new(empty());
        };
        let mut node = root;
        let mut key = prefix.iter().filter(|&b| *b > 0).copied();
        let mut prefix = vec![];
        loop {
            let k = match node.matching(&mut key, 0) {
                Matching::FullKey(_) | Matching::Exact => {
                    break;
                }
                Matching::FullPrefix(k) => {
                    prefix.extend(node.prefix.iter().copied());
                    k
                }
                Matching::Partial(..) => return Box::new(empty()),
            };

            let Some(child) = node.find_child(k) else {
                return Box::new(empty());
            };
            prefix.push(k);
            node = child;
        }

        node.leaves(prefix, PathType::Full)
    }
}

impl<V> AdaptiveRadixTrie<V> {
    fn continuation_indices(
        &self,
        prefix: &[u8],
        continuations: &[Vec<u8>],
        permutation: &[usize],
        skips: &[usize],
    ) -> Vec<usize> {
        let mut result = vec![];
        let Some(root) = &self.root else {
            return vec![];
        };

        let key = prefix.iter().filter(|&b| *b > 0).copied();
        let Some((node, n)) = root.contains_prefix_iter(key, 0) else {
            return result;
        };

        let mut i = 0;
        while let Some(&j) = permutation.get(i) {
            if node
                .contains_prefix_iter(continuations[j].iter().filter(|&b| *b > 0).copied(), n)
                .is_some()
            {
                result.push(j);
            } else {
                i += skips[i];
            }
            i += 1;
        }

        result.sort();
        result
    }

    fn paths(&self) -> impl Iterator<Item = (Vec<u8>, &V)> {
        match &self.root {
            Some(root) => {
                Box::new(root.leaves(vec![], PathType::Key)) as Box<dyn Iterator<Item = _>>
            }
            None => Box::new(empty()),
        }
    }

    #[inline]
    fn key_from_path(&self, path: &[u8]) -> Option<Vec<u8>> {
        let mut node = self.root.as_ref()?;
        let mut key: Vec<_> = node.prefix.iter().copied().collect();
        for &k in path {
            let Some(child) = node.find_child(k) else {
                return None;
            };
            key.push(k);
            key.extend(child.prefix.iter().copied());
            node = child;
        }
        if node.is_leaf() && key.last() == Some(&0) {
            key.pop();
            Some(key)
        } else {
            None
        }
    }
}

impl<K, V> FromIterator<(K, V)> for AdaptiveRadixTrie<V>
where
    K: AsRef<[u8]>,
{
    fn from_iter<I: IntoIterator<Item = (K, V)>>(iter: I) -> Self {
        let mut trie = AdaptiveRadixTrie::default();
        for (key, value) in iter {
            trie.insert(key.as_ref(), value);
        }
        trie
    }
}

pub type Paths = Box<[Box<[u8]>]>;

#[derive(Debug, Serialize, Deserialize)]
pub struct AdaptiveRadixContinuationTrie<V>
where
    V: Hash + Eq,
{
    pub(crate) trie: AdaptiveRadixTrie<Arc<V>>,
    value_paths: HashMap<Arc<V>, Paths>,
}

impl<V> AdaptiveRadixContinuationTrie<V>
where
    V: Hash + Eq,
{
    fn value_paths(trie: &AdaptiveRadixTrie<Arc<V>>) -> HashMap<Arc<V>, Paths> {
        let mut value_paths: HashMap<Arc<V>, Vec<Box<[u8]>>> = HashMap::new();
        for (path, value) in trie.paths() {
            if let Some(paths) = value_paths.get_mut(value) {
                paths.push(path.into_boxed_slice());
            } else {
                value_paths.insert(value.clone(), vec![path.into_boxed_slice()]);
            }
        }
        value_paths
            .into_iter()
            .map(|(k, v)| (k, v.into_boxed_slice()))
            .collect()
    }

    pub fn new<K: AsRef<[u8]>>(data: impl IntoIterator<Item = (K, V)>) -> Self {
        let trie = data.into_iter().map(|(k, v)| (k, Arc::new(v))).collect();
        let value_paths = Self::value_paths(&trie);
        Self { trie, value_paths }
    }

    pub fn continuation_indices(
        &self,
        prefix: &[u8],
        continuations: &[Vec<u8>],
        permutation: &[usize],
        skips: &[usize],
    ) -> Vec<usize> {
        self.trie
            .continuation_indices(prefix, continuations, permutation, skips)
    }

    pub fn get(&self, prefix: &[u8]) -> Option<&V> {
        self.trie.get(prefix).map(Arc::as_ref)
    }

    pub fn sub_index_by_values<Val>(&self, values: impl IntoIterator<Item = Val>) -> Self
    where
        V: From<Val>,
    {
        let trie = values
            .into_iter()
            .map(|v| v.into())
            .filter_map(|v| self.value_paths.get(&v).map(|path| (v, path)))
            .flat_map(|(value, paths)| {
                let value: Arc<V> = Arc::new(value);
                paths.iter().filter_map(move |path| {
                    let key = self.trie.key_from_path(path)?;
                    Some((key, value.clone()))
                })
            })
            .collect();
        Self {
            value_paths: Self::value_paths(&trie),
            trie,
        }
    }
}

impl<K, V> FromIterator<(K, V)> for AdaptiveRadixContinuationTrie<V>
where
    K: AsRef<[u8]>,
    V: Hash + Eq,
{
    fn from_iter<I: IntoIterator<Item = (K, V)>>(iter: I) -> Self {
        Self::new(iter)
    }
}
