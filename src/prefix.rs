use std::{
    fs::File,
    io::{BufRead, BufReader},
};

use anyhow::anyhow;
use pyo3::prelude::*;
use rayon::prelude::*;

use crate::{
    prefix_tree::{Node, PrefixTreeNode},
    prefix_vec::{FindResult, PrefixVec},
    utils::SerializeMsgPack,
};

pub trait PrefixTreeSearch<V> {
    fn size(&self) -> usize;

    fn insert(&mut self, key: &[u8], value: V);

    fn get(&self, prefix: &[u8]) -> Option<&V>;

    fn get_mut(&mut self, prefix: &[u8]) -> Option<&mut V>;

    fn contains(&self, prefix: &[u8]) -> bool;

    fn get_continuations(&self, prefix: &[u8]) -> Box<dyn Iterator<Item = (Vec<u8>, &V)> + '_>;
}

pub type Continuations = Vec<Vec<u8>>;
pub type ContinuationTree = Node<Vec<usize>>;

#[pyclass]
#[pyo3(name = "Vec")]
pub struct PyPrefixVec {
    inner: PrefixVec<String>,
    cont: Option<(Continuations, ContinuationTree)>,
}

#[pymethods]
impl PyPrefixVec {
    #[new]
    fn new() -> Self {
        Self {
            inner: PrefixVec::default(),
            cont: None,
        }
    }

    fn __len__(&self) -> usize {
        self.inner.size()
    }

    #[staticmethod]
    fn load(path: &str) -> anyhow::Result<Self> {
        let inner = PrefixVec::load(path)?;
        Ok(Self { inner, cont: None })
    }

    fn set_continuations(&mut self, continuations: Vec<Vec<u8>>) {
        // calculate interdependencies between continuations
        // e.g. if one continuation start with abc and is not
        // a valid one, then all continuations starting with abc
        // are also not valid

        // build tree
        let mut cont_tree: Node<Vec<usize>> = Node::default();
        // cont_tree.set_value((None, vec![]));
        // now insert the index along path for each continuation
        for (i, cont) in continuations.iter().enumerate() {
            let mut node = &mut cont_tree;
            for key in cont {
                if node.get_child(key).is_none() {
                    node.set_child(key, Node::default());
                }
                node = node.get_child_mut(key).unwrap();
            }
            if let Some(val) = node.get_value_mut() {
                val.push(i);
            } else {
                node.set_value(vec![i]);
            }
        }
        self.cont = Some((continuations, cont_tree));
    }

    fn save(&mut self, path: &str) -> anyhow::Result<()> {
        self.inner.save(path)?;
        Ok(())
    }

    #[staticmethod]
    fn from_file(path: &str) -> anyhow::Result<Self> {
        let file = File::open(path)?;
        let inner = BufReader::new(file)
            .lines()
            .filter_map(|line| match line {
                Err(_) => None,
                Ok(s) => {
                    let splits: Vec<_> = s.split('\t').collect();
                    assert!(splits.len() >= 3, "invalid line: {}", s);
                    let value = splits[0].trim();
                    Some(
                        splits[2..]
                            .iter()
                            .map(|&s| (s.trim().as_bytes().to_vec(), value.to_string()))
                            .collect::<Vec<_>>(),
                    )
                }
            })
            .flatten()
            .collect();
        Ok(Self { inner, cont: None })
    }

    fn insert(&mut self, key: Vec<u8>, value: String) {
        self.inner.insert(&key, value);
    }

    fn contains(&self, prefix: Vec<u8>) -> bool {
        self.inner.contains(&prefix)
    }

    fn batch_contains(&self, prefixes: Vec<Vec<u8>>) -> Vec<bool> {
        prefixes
            .into_iter()
            .map(|prefix| self.inner.contains(&prefix))
            .collect()
    }

    fn get(&self, key: Vec<u8>) -> Option<&str> {
        self.inner.get(&key).map(|s| s.as_ref())
    }

    fn batch_get(&self, keys: Vec<Vec<u8>>) -> Vec<Option<&str>> {
        keys.into_iter()
            .map(|key| self.inner.get(&key).map(|s| s.as_ref()))
            .collect()
    }

    fn get_continuations(&self, prefix: Vec<u8>) -> Vec<(Vec<u8>, &str)> {
        self.inner
            .get_continuations(&prefix)
            .map(|(s, v)| (s.to_vec(), v.as_ref()))
            .collect()
    }

    fn continuation_mask(&self, prefix: &[u8]) -> anyhow::Result<(Vec<bool>, bool)> {
        let Some((continuations, cont_tree)) = self.cont.as_ref() else {
            return Err(anyhow!("no continuations set"));
        };
        let data = match self.inner.find_range(prefix, 0, self.inner.size(), 0) {
            FindResult::NotFound(..) => return Ok((vec![false; continuations.len()], false)),
            FindResult::Found(left, right) => &self.inner.data[left..right],
        };
        let mut mask = vec![false; continuations.len()];
        for (cont, _) in data {
            for cont_indices in cont_tree.get_path(&cont[prefix.len()..]) {
                for idx in cont_indices {
                    mask[*idx] = true;
                }
            }
        }
        Ok((mask, !data.is_empty() && data[0].0.len() == prefix.len()))
    }

    fn batch_continuation_mask(
        &self,
        prefixes: Vec<Vec<u8>>,
    ) -> anyhow::Result<(Vec<Vec<bool>>, Vec<bool>)> {
        prefixes
            .into_par_iter()
            .map(|prefix| self.continuation_mask(&prefix))
            .collect()
    }

    fn batch_get_continuations(&self, prefixes: Vec<Vec<u8>>) -> Vec<Vec<(Vec<u8>, &str)>> {
        prefixes
            .into_par_iter()
            .map(|prefix| {
                self.inner
                    .get_continuations(&prefix)
                    .map(|(s, v)| (s.to_vec(), v.as_ref()))
                    .collect()
            })
            .collect()
    }

    fn at(&self, idx: usize) -> Option<(Vec<u8>, &str)> {
        self.inner
            .data
            .get(idx)
            .map(|(s, v)| (s.to_vec(), v.as_ref()))
    }
}

/// A submodule containing python implementations of a prefix tree
pub(super) fn add_submodule(py: Python, parent_module: &PyModule) -> PyResult<()> {
    let m = PyModule::new(py, "prefix")?;
    m.add_class::<PyPrefixVec>()?;
    parent_module.add_submodule(m)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::{prefix::PrefixTreeSearch, prefix_tree::Node, prefix_vec::PrefixVec};

    #[test]
    fn test_prefix() {
        let trees: Vec<Box<dyn PrefixTreeSearch<i32>>> =
            vec![Box::new(Node::default()), Box::new(PrefixVec::default())];
        for mut tree in trees {
            tree.insert(b"hello", 1);
            assert!(tree.contains(b"hello"));
            assert!(tree.contains(b"hell"));
            assert!(!tree.contains(b"helloo"));
            assert!(tree.get(b"hell").is_none());
            assert_eq!(tree.get(b"hello"), Some(&1));
            tree.insert(b"hello", 2);
            assert_eq!(tree.get(b"hello"), Some(&2));
        }
    }
}