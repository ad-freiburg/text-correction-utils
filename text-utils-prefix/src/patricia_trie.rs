use std::{
    collections::HashMap,
    iter::{empty, once},
};

use crate::{ContinuationSearch, PrefixSearch};

#[derive(Default, Debug)]
enum NodeType<V> {
    #[default]
    Empty,
    Leaf(V),
    Inner(Box<[Option<Box<Node<V>>>; 256]>),
}

#[derive(Debug)]
struct Node<V> {
    prefix: Box<[u8]>,
    inner: NodeType<V>,
}

#[derive(Debug)]
pub struct PatriciaTrie<V> {
    root: Option<Node<V>>,
}

#[derive(Debug)]
pub struct PatriciaTrieStats {
    pub depth: usize,
    pub num_nodes: usize,
    pub num_keys: usize,
    pub node_info: HashMap<String, (usize, f32)>,
}

impl<V> PatriciaTrie<V> {
    pub fn stats(&self) -> PatriciaTrieStats {
        let mut dist =
            HashMap::from_iter(["leaf", "inner"].iter().map(|&s| (s.to_string(), (0, 0.0))));
        let Some(root) = &self.root else {
            return PatriciaTrieStats {
                depth: 0,
                num_nodes: 0,
                num_keys: 0,
                node_info: dist,
            };
        };
        let mut stack = vec![(root, 0)];
        let mut max_depth = 0;
        while let Some((node, depth)) = stack.pop() {
            max_depth = max_depth.max(depth);
            let name = match &node.inner {
                NodeType::Empty => unreachable!("should not happen"),
                NodeType::Leaf(_) => "leaf",
                NodeType::Inner(..) => "inner",
            };
            let val = dist.get_mut(name).unwrap();
            val.0 += 1;
            let n = val.0 as f32;
            val.1 = (val.1 * (n - 1.0) + node.prefix.len() as f32) / n;
            stack.extend(node.children().map(|child| (child, depth + 1)));
        }
        PatriciaTrieStats {
            depth: max_depth,
            num_nodes: dist.iter().map(|(_, (n, _))| n).sum(),
            num_keys: dist["leaf"].0,
            node_info: dist,
        }
    }
}

impl<V> Default for PatriciaTrie<V> {
    fn default() -> Self {
        Self { root: None }
    }
}

impl<K, V> FromIterator<(K, V)> for PatriciaTrie<V>
where
    K: AsRef<[u8]>,
{
    fn from_iter<T: IntoIterator<Item = (K, V)>>(iter: T) -> Self {
        let mut trie = Self::default();
        for (k, v) in iter {
            trie.insert(k, v);
        }
        trie
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
            inner: NodeType::Inner(Box::new(std::array::from_fn(|_| None))),
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
        match &self.inner {
            NodeType::Empty | NodeType::Leaf(_) => false,
            NodeType::Inner(children) => children[key as usize].is_some(),
        }
    }

    fn children(&self) -> Box<dyn Iterator<Item = &Self> + '_> {
        match &self.inner {
            NodeType::Empty | NodeType::Leaf(_) => Box::new(empty()),
            NodeType::Inner(children) => {
                Box::new(children.iter().filter_map(|child| child.as_deref()))
            }
        }
    }

    #[inline]
    fn find_child(&self, key: u8) -> Option<&Self> {
        match &self.inner {
            NodeType::Empty | NodeType::Leaf(_) => None,
            NodeType::Inner(children) => children[key as usize].as_deref(),
        }
    }

    #[inline]
    fn find_child_mut(&mut self, key: u8) -> Option<&mut Self> {
        match &mut self.inner {
            NodeType::Empty | NodeType::Leaf(_) => None,
            NodeType::Inner(children) => children[key as usize].as_deref_mut(),
        }
    }

    #[inline]
    fn set_child(&mut self, key: u8, child: Self) {
        let NodeType::Inner(children) = &mut self.inner else {
            unreachable!("set child called on leaf node");
        };
        let pos = &mut children[key as usize];
        if pos.is_some() {
            unreachable!("should not happen");
        }
        *pos = Some(Box::new(child));
    }

    #[inline]
    fn contains_prefix_iter(
        &self,
        mut key: impl Iterator<Item = u8>,
        offset: usize,
    ) -> Option<(&Self, usize)> {
        let mut node = self;
        // extend given key with null byte
        // because its needed for the correctness of the algorithm
        // when it comes to key lookup
        loop {
            let k = match node.matching(&mut key, offset) {
                Matching::FullKey(n) => return Some((node, n)),
                Matching::Exact => return Some((node, node.prefix.len())),
                Matching::FullPrefix(k) => k,
                Matching::Partial(..) => break,
            };

            let Some(child) = node.find_child(k) else {
                break;
            };
            node = child;
        }
        None
    }
}

impl<V> PrefixSearch for PatriciaTrie<V> {
    type Value = V;

    fn insert<K>(&mut self, key: K, value: V)
    where
        K: AsRef<[u8]>,
    {
        let mut key = key.as_ref().iter().copied().chain(once(0));
        // empty tree
        let Some(root) = &mut self.root else {
            // insert leaf at root
            self.root = Some(Node::new_leaf(key.collect(), value));
            return;
        };
        let mut node = root;
        loop {
            let matching = node.matching(&mut key, 0);
            if node.is_leaf() {
                let (inner_prefix, new_prefix, n, k) = match matching {
                    Matching::FullKey(_) => unreachable!("should not happen"),
                    Matching::FullPrefix(_) => unreachable!("should not happen"),
                    Matching::Partial(n, k) => (
                        node.prefix[..n].to_vec(),
                        node.prefix[n + 1..].to_vec(),
                        n,
                        k,
                    ),
                    Matching::Exact => {
                        // exact match, only replace leaf value
                        node.inner = NodeType::Leaf(value);
                        return;
                    }
                };
                let mut inner = Node::new_inner(inner_prefix);
                let NodeType::Leaf(node_value) = std::mem::take(&mut node.inner) else {
                    unreachable!("should not happen");
                };
                inner.set_child(node.prefix[n], Node::new_leaf(new_prefix, node_value));
                inner.set_child(k, Node::new_leaf(key.collect(), value));
                *node = inner;
                break;
            } else if let Matching::FullPrefix(k) = matching {
                // full prefix match, either go to next child
                // or append leaf with rest of key
                if node.has_child(k) {
                    node = node.find_child_mut(k).expect("should not happen");
                    continue;
                }
                node.set_child(k, Node::new_leaf(key.collect(), value));
            } else if let Matching::Partial(n, k) = matching {
                // partial prefix match, introduce new inner node
                let mut inner = Node::new_inner(node.prefix[..n].to_vec());
                let mut new_node = Node::new_inner(node.prefix[n + 1..].to_vec());
                new_node.inner = std::mem::take(&mut node.inner);
                inner.set_child(node.prefix[n], new_node);
                inner.set_child(k, Node::new_leaf(key.collect(), value));
                *node = inner;
            }
            break;
        }
    }

    fn delete<K>(&mut self, key: K) -> Option<V>
    where
        K: AsRef<[u8]>,
    {
        let Some(root) = &mut self.root else {
            return None;
        };

        // handle special case where root is leaf
        if root.is_leaf() {
            let NodeType::Leaf(value) = std::mem::take(&mut root.inner) else {
                unreachable!("should not happen");
            };
            self.root = None;
            return Some(value);
        }

        let mut node = root;
        let mut key = key.as_ref().iter().copied().chain(once(0));
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
            let NodeType::Inner(children) = &mut node.inner else {
                unreachable!("should not happen");
            };
            let child = std::mem::take(&mut children[k as usize])?;
            let NodeType::Leaf(value) = child.inner else {
                unreachable!("should not happen");
            };
            let child_indices: Vec<_> = children
                .iter()
                .enumerate()
                .filter_map(|(i, child)| child.as_ref().map(|_| i))
                .collect();
            assert!(!child_indices.is_empty());
            if child_indices.len() == 1 {
                // if we only have one child left, we can merge
                // the child into the current node
                let single_child_k = child_indices.into_iter().next().unwrap();
                let single_child = std::mem::take(&mut children[single_child_k])?;
                let new_prefix: Vec<_> = node
                    .prefix
                    .iter()
                    .copied()
                    .chain(once(single_child_k as u8))
                    .chain(single_child.prefix.iter().copied())
                    .collect();
                node.prefix = new_prefix.into_boxed_slice();
                node.inner = single_child.inner;
            }
            return Some(value);
        }
        None
    }

    fn get<K>(&self, key: K) -> Option<&V>
    where
        K: AsRef<[u8]>,
    {
        let Some(root) = &self.root else {
            return None;
        };

        let key = key.as_ref().iter().copied().chain(once(0));
        root.find_iter(key).and_then(|node| match &node.inner {
            NodeType::Leaf(v) => Some(v),
            _ => None,
        })
    }

    fn contains_prefix<P>(&self, prefix: P) -> bool
    where
        P: AsRef<[u8]>,
    {
        let Some(root) = &self.root else {
            return false;
        };

        let key = prefix.as_ref().iter().copied();
        root.contains_prefix_iter(key, 0).is_some()
    }
}

impl<V> ContinuationSearch for PatriciaTrie<V> {
    fn continuations<'a, P>(&'a self, prefix: P) -> impl Iterator<Item = (Vec<u8>, &'a V)>
    where
        P: AsRef<[u8]>,
        V: 'a,
    {
        empty()
    }

    fn contains_continuation<P, C>(&self, prefix: P, continuation: C) -> bool
    where
        P: AsRef<[u8]>,
        C: AsRef<[u8]>,
    {
        let Some(root) = &self.root else {
            return false;
        };

        let key = prefix
            .as_ref()
            .iter()
            .chain(continuation.as_ref().iter())
            .copied();
        root.contains_prefix_iter(key, 0).is_some()
    }

    fn contains_continuations<P, C>(&self, prefix: P, continuations: &[C]) -> Vec<usize>
    where
        P: AsRef<[u8]>,
        C: AsRef<[u8]>,
    {
        let Some(root) = &self.root else {
            return vec![];
        };

        let key = prefix.as_ref().iter().copied();
        let Some((node, n)) = root.contains_prefix_iter(key, 0) else {
            return vec![];
        };

        continuations
            .iter()
            .enumerate()
            .filter_map(|(i, c)| {
                let key = c.as_ref().iter().copied();
                if node.contains_prefix_iter(key, n).is_some() {
                    Some(i)
                } else {
                    None
                }
            })
            .collect()
    }

    fn contains_continuations_optimized<P, C>(
        &self,
        prefix: P,
        continuations: &[C],
        permutation: &[usize],
        skips: &[usize],
    ) -> Vec<usize>
    where
        P: AsRef<[u8]>,
        C: AsRef<[u8]>,
    {
        let mut result = vec![];
        let Some(root) = &self.root else {
            return result;
        };

        let key = prefix.as_ref().iter().copied();
        let Some((node, n)) = root.contains_prefix_iter(key, 0) else {
            return result;
        };

        let mut i = 0;
        while let Some(&j) = permutation.get(i) {
            let continuation = continuations[j].as_ref();
            if node
                .contains_prefix_iter(continuation.iter().copied(), n)
                .is_some()
            {
                result.push(j);
            } else {
                i += skips[i];
            }
            i += 1;
        }

        result
    }
}

#[cfg(test)]
mod test {
    use crate::patricia_trie::Node;
    use crate::{patricia_trie::PatriciaTrie, PrefixSearch};
    use std::fs;
    use std::path::PathBuf;

    #[test]
    fn test_trie() {
        println!(
            "size of patricia trie node: {}, box array: {}, box slice: {}, vec: {}",
            std::mem::size_of::<Node<i32>>(),
            std::mem::size_of::<Box<[usize; 256]>>(),
            std::mem::size_of::<Box<[usize]>>(),
            std::mem::size_of::<Vec<usize>>()
        );
        let mut trie = PatriciaTrie::default();
        assert_eq!(trie.get(b"hello"), None);
        assert_eq!(trie.get(b""), None);
        assert!(!trie.contains_prefix(b""));
        trie.insert(b"hello", 1);
        assert_eq!(trie.delete(b"hello"), Some(1));
        assert_eq!(trie.delete(b"hello "), None);
        trie.insert(b"hello", 1);
        trie.insert(b"hell", 2);
        trie.insert(b"hello world", 3);
        assert_eq!(trie.get(b"hello"), Some(&1));
        assert_eq!(trie.get(b"hell"), Some(&2));
        assert_eq!(trie.get(b"hello world"), Some(&3));
        assert_eq!(trie.contains_prefix(b"hell"), true);
        assert_eq!(trie.contains_prefix(b"hello"), true);
        assert_eq!(trie.contains_prefix(b""), true);
        assert_eq!(trie.contains_prefix(b"hello world!"), false);
        assert_eq!(trie.contains_prefix(b"test"), false);
        assert_eq!(trie.delete(b"hello"), Some(1));
        assert_eq!(trie.get(b"hello"), None);
        let stats = trie.stats();
        assert_eq!(stats.num_keys, 2);

        let dir = env!("CARGO_MANIFEST_DIR");
        let index = fs::read_to_string(PathBuf::from(dir).join("resources/test/index.txt"))
            .expect("failed to read file");
        let n = 100_000;
        let words: Vec<_> = index.lines().map(|s| s.as_bytes()).take(n).collect();

        let mut trie: PatriciaTrie<_> = words.iter().enumerate().map(|(i, w)| (w, i)).collect();
        let stats = trie.stats();
        println!("{:#?}", stats);
        assert_eq!(stats.num_keys, n);
        for (i, word) in words.iter().enumerate() {
            assert_eq!(trie.get(word), Some(&i));
            for j in 0..word.len() {
                assert!(trie.contains_prefix(&word[..=j]));
            }
        }
        for (i, word) in words.iter().enumerate() {
            let even = i % 2 == 0;
            if even {
                assert_eq!(trie.delete(word), Some(i));
                assert_eq!(trie.get(word), None);
            } else {
                assert_eq!(trie.get(word), Some(&i));
            }
        }
        let stats = trie.stats();
        assert_eq!(stats.num_keys, n / 2);
    }
}