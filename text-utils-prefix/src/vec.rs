use itertools::Itertools;
use std::{cmp::Ordering, iter::empty};

use serde::{Deserialize, Serialize};

use crate::{ContinuationSearch, PrefixSearch};

#[derive(Serialize, Deserialize)]
pub struct PrefixVec<V> {
    data: Vec<(Box<[u8]>, V)>,
}

#[derive(Debug)]
pub struct PrefixVecStats {
    pub num_keys: usize,
}

impl<V> Default for PrefixVec<V> {
    fn default() -> Self {
        Self { data: vec![] }
    }
}

enum FindResult {
    Found(usize, usize),
    NotFound,
}

impl<V> PrefixVec<V> {
    pub fn stats(&self) -> PrefixVecStats {
        PrefixVecStats {
            num_keys: self.data.len(),
        }
    }

    #[inline]
    fn binary_search(
        &self,
        key: &u8,
        depth: usize,
        mut left: usize,
        mut right: usize,
    ) -> Result<usize, usize> {
        // perform binary search over bytes at given depth
        let mut size = right - left;
        while left < right {
            let mid = left + size / 2;

            let cur = &self.data[mid].0;

            let cmp = if depth >= cur.len() {
                Ordering::Greater
            } else {
                cur[depth].cmp(key)
            };

            if cmp == Ordering::Less {
                left = mid + 1;
            } else if cmp == Ordering::Greater {
                right = mid;
            } else {
                return Ok(mid);
            }
            size = right - left;
        }
        Err(left)
    }

    #[inline]
    fn range_search(
        &self,
        key: &u8,
        depth: usize,
        left: usize,
        right: usize,
    ) -> Option<(usize, usize)> {
        let idx = match self.binary_search(key, depth, left, right) {
            Err(_) => return None,
            Ok(idx) => idx,
        };

        let right_bound = |right_idx: usize| -> bool {
            right_idx < right
                && depth < self.data[right_idx].0.len()
                && self.data[right_idx].0[depth] <= *key
        };

        // exponential search to overshoot right bound
        let mut right_idx = idx;
        let mut i = 0;
        while right_bound(right_idx) {
            right_idx += 2usize.pow(i);
            i += 1;
        }

        // search linearly from the previous exponential search position
        // to find right bound
        right_idx = idx + 2usize.pow(i - 1);
        while right_bound(right_idx) {
            right_idx += 1;
        }

        // now do the same for the left bound, a little bit
        // different here since left is inclusive
        let left_bound = |left_idx: usize| -> bool {
            left_idx > left
                && depth < self.data[left_idx - 1].0.len()
                && self.data[left_idx - 1].0[depth] >= *key
        };
        let mut left_idx = idx;
        i = 0;
        while left_bound(left_idx) {
            left_idx = left_idx.saturating_sub(2usize.pow(i));
            i += 1;
        }

        if i > 0 {
            left_idx = idx.saturating_sub(2usize.pow(i - 1));
            while left_bound(left_idx) {
                left_idx -= 1;
            }
        }
        Some((left_idx, right_idx))
    }

    #[inline]
    fn find_range(
        &self,
        key: &[u8],
        mut left: usize,
        mut right: usize,
        start_depth: usize,
    ) -> FindResult {
        for (depth, k) in key.iter().enumerate() {
            let Some((new_left, new_right)) =
                self.range_search(k, start_depth + depth, left, right)
            else {
                return FindResult::NotFound;
            };
            left = new_left;
            right = new_right;
        }
        FindResult::Found(left, right)
    }
}

impl<V> PrefixSearch for PrefixVec<V> {
    type Value = V;

    #[inline]
    fn insert(&mut self, key: &[u8], value: V) -> Option<V> {
        match self
            .data
            .binary_search_by(|(prefix, _)| prefix.as_ref().cmp(key))
        {
            Ok(idx) => Some(std::mem::replace(&mut self.data[idx].1, value)),
            Err(idx) => {
                self.data
                    .insert(idx, (key.to_vec().into_boxed_slice(), value));
                None
            }
        }
    }

    fn delete(&mut self, key: &[u8]) -> Option<Self::Value> {
        let Ok(idx) = self
            .data
            .binary_search_by(|(prefix, _)| prefix.as_ref().cmp(key))
        else {
            return None;
        };
        let (_, value) = self.data.remove(idx);
        Some(value)
    }

    #[inline]
    fn get(&self, prefix: &[u8]) -> Option<&V> {
        match self.find_range(prefix, 0, self.data.len(), 0) {
            FindResult::Found(left, _) if left < self.data.len() => {
                let (key, value) = &self.data[left];
                if key.len() != prefix.len() {
                    None
                } else {
                    Some(value)
                }
            }
            _ => None,
        }
    }

    #[inline]
    fn contains_prefix(&self, prefix: &[u8]) -> bool {
        matches!(self.find_range(prefix, 0, self.data.len(), 0), FindResult::Found(left, _) if left < self.data.len())
    }

    fn path(&self, prefix: &[u8]) -> Vec<(usize, &Self::Value)> {
        let mut left = 0;
        let mut right = self.data.len();
        let mut path = vec![];
        // empty path explicitly
        match self.data.first() {
            Some((key, value)) if key.is_empty() => {
                path.push((0, value));
            }
            _ => (),
        }
        for (i, k) in prefix.iter().enumerate() {
            let Some((l, r)) = self.range_search(k, i, left, right) else {
                break;
            };
            left = l;
            right = r;
            match self.data.get(left) {
                Some((key, value)) if key.len() == i + 1 => {
                    path.push((i + 1, value));
                }
                _ => (),
            }
        }
        path
    }

    fn iter_continuations(&self, prefix: &[u8]) -> Box<dyn Iterator<Item = (Vec<u8>, &V)> + '_> {
        match self.find_range(prefix, 0, self.data.len(), 0) {
            FindResult::Found(left, right) => Box::new(
                self.data[left..right]
                    .iter()
                    .map(|(key, value)| (key.to_vec(), value)),
            ),
            FindResult::NotFound => Box::new(empty()),
        }
    }
}

impl<K, V> FromIterator<(K, V)> for PrefixVec<V>
where
    K: AsRef<[u8]>,
{
    fn from_iter<T: IntoIterator<Item = (K, V)>>(iter: T) -> Self {
        let mut pfx = Self::default();
        // remove duplicates
        for (key, value) in iter
            .into_iter()
            .sorted_by(|(k1, _), (k2, _)| k1.as_ref().cmp(k2.as_ref()))
        {
            let key = key.as_ref();
            match pfx.data.last_mut() {
                None => {
                    pfx.data.push((key.to_vec().into_boxed_slice(), value));
                }
                Some((k, _)) if k.as_ref() != key => {
                    pfx.data.push((key.to_vec().into_boxed_slice(), value));
                }
                Some((_, v)) => *v = value,
            }
        }
        pfx
    }
}

pub(crate) struct Node {
    indices: Vec<usize>,
    children: [Option<Box<Node>>; 256],
}

impl Default for Node {
    fn default() -> Self {
        Self {
            indices: vec![],
            children: std::array::from_fn(|_| None),
        }
    }
}

pub struct ContinuationsVec<V> {
    pub(crate) vec: PrefixVec<V>,
    pub(crate) continuation_trie: Node,
}

impl<V> ContinuationsVec<V> {
    pub fn new<C>(vec: PrefixVec<V>, continuations: &[C]) -> Self
    where
        C: AsRef<[u8]>,
    {
        let mut continuation_trie = Node::default();
        for (i, continuation) in continuations.iter().enumerate() {
            let mut node = &mut continuation_trie;
            for byte in continuation.as_ref() {
                node = node.children[*byte as usize].get_or_insert_with(Box::default);
            }
            node.indices.push(i);
        }
        Self {
            vec,
            continuation_trie,
        }
    }
}

impl<V> PrefixSearch for ContinuationsVec<V> {
    type Value = V;

    fn insert(&mut self, key: &[u8], value: V) -> Option<V> {
        self.vec.insert(key, value)
    }

    fn delete(&mut self, key: &[u8]) -> Option<Self::Value> {
        self.vec.delete(key)
    }

    fn get(&self, prefix: &[u8]) -> Option<&V> {
        self.vec.get(prefix)
    }

    fn contains_prefix(&self, prefix: &[u8]) -> bool {
        self.vec.contains_prefix(prefix)
    }

    fn path(&self, prefix: &[u8]) -> Vec<(usize, &Self::Value)> {
        self.vec.path(prefix)
    }

    fn iter_continuations(&self, prefix: &[u8]) -> Box<dyn Iterator<Item = (Vec<u8>, &V)> + '_> {
        self.vec.iter_continuations(prefix)
    }
}

impl<V> ContinuationSearch for ContinuationsVec<V> {
    fn contains_continuations(&self, prefix: &[u8]) -> Vec<usize> {
        let FindResult::Found(left, right) = self.vec.find_range(prefix, 0, self.vec.data.len(), 0)
        else {
            return vec![];
        };

        self.vec.data[left..right]
            .iter()
            .flat_map(|(value, _)| {
                let mut indices = vec![];
                let mut node = &self.continuation_trie;
                indices.extend(node.indices.iter().copied());
                for byte in &value[prefix.len()..] {
                    if let Some(child) = &node.children[*byte as usize] {
                        node = child;
                        indices.extend(node.indices.iter().copied());
                    } else {
                        break;
                    }
                }
                indices
            })
            .unique()
            .collect()
    }
}
