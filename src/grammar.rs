use std::sync::mpsc::channel;
use std::sync::{Arc, Mutex};
// use std::time::Instant;

use anyhow::anyhow;
use numpy::ndarray::Array1;
use numpy::IntoPyArray;
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList};
use rayon::spawn;
use text_utils_grammar::lr1::TokenAndSpan;
use text_utils_grammar::{
    Constraint, ExactLR1GrammarConstraint, LR1GrammarConstraint, LR1GrammarParser, LR1Parse,
    LR1State, RegularExpressionConstraint, RegularExpressionState,
};

#[derive(Clone)]
struct RegexInner {
    state: RegularExpressionState,
    indices: Array1<usize>,
    is_match: bool,
}

#[pyclass]
struct RegexConstraint {
    constraint: Arc<RegularExpressionConstraint>,
    inner: Arc<Mutex<RegexInner>>,
}

impl RegexConstraint {
    fn init(constraint: RegularExpressionConstraint) -> Self {
        let state = constraint.get_start_state();
        let indices = constraint.get_valid_continuations(&state).into();
        let is_match = constraint.is_match_state(&state);
        Self {
            constraint: Arc::new(constraint),
            inner: Arc::new(Mutex::new(RegexInner {
                state,
                indices,
                is_match,
            })),
        }
    }
}

#[pymethods]
impl RegexConstraint {
    #[new]
    fn new(pattern: &str, continuations: Vec<Vec<u8>>) -> anyhow::Result<Self> {
        RegularExpressionConstraint::new(pattern, continuations)
            .map(Self::init)
            .map_err(|e| {
                anyhow!(
                    "failed to create regular expression constraint from pattern '{}': {}",
                    pattern,
                    e
                )
            })
    }

    #[staticmethod]
    fn from_file(path: &str, continuations: Vec<Vec<u8>>) -> anyhow::Result<Self> {
        RegularExpressionConstraint::from_file(path, continuations)
            .map(Self::init)
            .map_err(|e| {
                anyhow!(
                    "failed to create regular expression constraint from file '{}': {}",
                    path,
                    e
                )
            })
    }

    fn reset(&self, prefix: Option<Vec<u8>>) -> anyhow::Result<()> {
        let Some(state) = self.constraint.get_state(&prefix.unwrap_or_default()) else {
            return Err(anyhow!("failed to reset to given prefix"));
        };
        self.inner
            .lock()
            .map(|mut inner| {
                inner.state = state;
                let indices = self.constraint.get_valid_continuations(&inner.state);
                inner.indices = indices.into();
                inner.is_match = self.constraint.is_match_state(&inner.state);
            })
            .map_err(|_| anyhow!("error locking inner state"))
    }

    fn clone(&self) -> anyhow::Result<Self> {
        self.inner
            .lock()
            .map(|inner| Self {
                constraint: self.constraint.clone(),
                inner: Arc::new(Mutex::new(inner.clone())),
            })
            .map_err(|_| anyhow!("error locking inner state"))
    }

    fn get(&self, py: Python<'_>) -> anyhow::Result<(PyObject, bool)> {
        self.inner
            .lock()
            .map(|inner| {
                (
                    inner.indices.clone().into_pyarray_bound(py).into_py(py),
                    inner.is_match,
                )
            })
            .map_err(|_| anyhow!("error locking inner state"))
    }

    fn is_match(&self) -> anyhow::Result<bool> {
        self.inner
            .lock()
            .map(|inner| inner.is_match)
            .map_err(|_| anyhow!("error locking inner state"))
    }

    fn next(&self, index: usize) -> anyhow::Result<()> {
        let inner = self.inner.clone();
        let constraint = self.constraint.clone();
        let (tx, rx) = channel();
        spawn(move || {
            let mut inner = inner.lock().expect("error locking inner state");
            tx.send(()).expect("failed to send on channel");
            let next_state = constraint
                .get_next_state(&inner.state, index)
                .expect("invalid continuation");
            inner.state = next_state;
            let indices = constraint.get_valid_continuations(&inner.state);
            inner.indices = indices.into();
            inner.is_match = constraint.is_match_state(&inner.state);
        });
        // wait until spawned thread signals that is has locked
        // the inner state, otherwise some unexpected behavior could occurr
        rx.recv()?;
        Ok(())
    }
}

enum LR1Type {
    Exact(ExactLR1GrammarConstraint),
    Regular(LR1GrammarConstraint),
}

#[derive(Clone)]
struct LR1Inner {
    state: LR1State,
    indices: Array1<usize>,
    is_match: bool,
}

#[pyclass]
struct LR1Constraint {
    constraint: Arc<LR1Type>,
    inner: Arc<Mutex<LR1Inner>>,
}

impl LR1Type {
    fn get_state(&self, prefix: &[u8]) -> Option<LR1State> {
        match self {
            LR1Type::Exact(inner) => inner.get_state(prefix),
            LR1Type::Regular(inner) => inner.get_state(prefix),
        }
    }

    fn get_start_state(&self) -> LR1State {
        match self {
            LR1Type::Exact(inner) => inner.get_start_state(),
            LR1Type::Regular(inner) => inner.get_start_state(),
        }
    }

    fn get_valid_continuations(&self, state: &LR1State) -> Array1<usize> {
        match self {
            LR1Type::Exact(inner) => inner.get_valid_continuations(state),
            LR1Type::Regular(inner) => inner.get_valid_continuations(state),
        }
        .into()
    }

    fn get_next_state(&self, state: &LR1State, continuation: usize) -> Option<LR1State> {
        match self {
            LR1Type::Exact(inner) => inner.get_next_state(state, continuation),
            LR1Type::Regular(inner) => inner.get_next_state(state, continuation),
        }
    }

    fn is_match_state(&self, state: &LR1State) -> bool {
        match self {
            LR1Type::Exact(inner) => inner.is_match_state(state),
            LR1Type::Regular(inner) => inner.is_match_state(state),
        }
    }

    fn only_skippable_matching(&self, state: &LR1State) -> bool {
        match self {
            LR1Type::Exact(inner) => inner.only_skippable_matching(state),
            LR1Type::Regular(inner) => inner.only_skippable_matching(state),
        }
    }
}

impl LR1Constraint {
    fn init(constraint: LR1Type) -> Self {
        let state = constraint.get_start_state();
        let indices = constraint.get_valid_continuations(&state);
        let is_match = constraint.is_match_state(&state);
        Self {
            constraint: Arc::new(constraint),
            inner: Arc::new(Mutex::new(LR1Inner {
                state,
                indices,
                is_match,
            })),
        }
    }
}

#[pymethods]
impl LR1Constraint {
    #[new]
    #[pyo3(signature = (grammar, lexer, continuations, exact=false))]
    fn new(
        grammar: &str,
        lexer: &str,
        continuations: Vec<Vec<u8>>,
        exact: bool,
    ) -> anyhow::Result<Self> {
        let constraint = if exact {
            LR1Type::Exact(
                ExactLR1GrammarConstraint::new(grammar, lexer, continuations)
                    .map_err(|e| anyhow!("failed to create LR(1) grammar constraint: {}", e))?,
            )
        } else {
            LR1Type::Regular(
                LR1GrammarConstraint::new(grammar, lexer, continuations)
                    .map_err(|e| anyhow!("failed to create LR(1) grammar constraint: {}", e))?,
            )
        };
        Ok(Self::init(constraint))
    }

    #[staticmethod]
    #[pyo3(signature = (grammar_path, lexer_path, continuations, exact=false))]
    fn from_files(
        grammar_path: &str,
        lexer_path: &str,
        continuations: Vec<Vec<u8>>,
        exact: bool,
    ) -> anyhow::Result<Self> {
        let constraint = if exact {
            LR1Type::Exact(
                ExactLR1GrammarConstraint::from_files(grammar_path, lexer_path, continuations)
                    .map_err(|e| anyhow!("failed to create LR(1) grammar constraint: {}", e))?,
            )
        } else {
            LR1Type::Regular(
                LR1GrammarConstraint::from_files(grammar_path, lexer_path, continuations)
                    .map_err(|e| anyhow!("failed to create LR(1) grammar constraint: {}", e))?,
            )
        };
        Ok(Self::init(constraint))
    }

    fn reset(&self, prefix: Option<Vec<u8>>) -> anyhow::Result<()> {
        let Some(state) = self.constraint.get_state(&prefix.unwrap_or_default()) else {
            return Err(anyhow!("failed to reset to given prefix"));
        };
        self.inner
            .lock()
            .map(|mut inner| {
                inner.state = state;
                let indices = self.constraint.get_valid_continuations(&inner.state);
                inner.indices = indices;
                inner.is_match = self.constraint.is_match_state(&inner.state);
            })
            .map_err(|_| anyhow!("error locking inner state"))
    }

    fn clone(&self) -> anyhow::Result<Self> {
        self.inner
            .lock()
            .map(|inner| Self {
                constraint: self.constraint.clone(),
                inner: Arc::new(Mutex::new(inner.clone())),
            })
            .map_err(|_| anyhow!("error locking inner state"))
    }

    fn get(&self, py: Python<'_>) -> anyhow::Result<(PyObject, bool)> {
        self.inner
            .lock()
            .map(|inner| {
                let indices =
                    if inner.is_match && self.constraint.only_skippable_matching(&inner.state) {
                        // should stop, return empty indices
                        vec![].into()
                    } else {
                        inner.indices.clone()
                    }
                    .into_pyarray_bound(py)
                    .into_py(py);
                (indices, inner.is_match)
            })
            .map_err(|_| anyhow!("error locking inner state"))
    }

    fn is_match(&self) -> anyhow::Result<bool> {
        self.inner
            .lock()
            .map(|inner| inner.is_match)
            .map_err(|_| anyhow!("error locking inner state"))
    }

    fn next(&self, index: usize) -> anyhow::Result<()> {
        let inner = self.inner.clone();
        let constraint = self.constraint.clone();
        let (tx, rx) = channel();
        spawn(move || {
            let mut inner = inner.lock().expect("error locking inner state");
            tx.send(()).expect("failed to send on channel");
            inner.state = constraint
                .get_next_state(&inner.state, index)
                .expect("invalid continuation");
            let indices = constraint.get_valid_continuations(&inner.state);
            inner.indices = indices;
            inner.is_match = constraint.is_match_state(&inner.state);
        });
        // wait until spawned thread signals that is has locked
        // the inner state, otherwise some unexpected behavior could occurr
        rx.recv()?;
        Ok(())
    }
}

#[pyclass]
pub struct LR1Parser {
    inner: LR1GrammarParser,
}

#[pymethods]
impl LR1Parser {
    #[new]
    fn new(grammar: &str, lexer: &str) -> anyhow::Result<Self> {
        let inner = LR1GrammarParser::new(grammar, lexer).map_err(|e| {
            anyhow!(
                "failed to create LR(1) grammar parser from grammar {} and lexer {}: {}",
                grammar,
                lexer,
                e
            )
        })?;
        Ok(Self { inner })
    }

    #[staticmethod]
    fn from_files(grammar_path: &str, lexer_path: &str) -> anyhow::Result<Self> {
        let inner = LR1GrammarParser::from_files(grammar_path, lexer_path).map_err(|e| {
            anyhow!(
                "failed to create LR(1) grammar parser from files {} and {}: {}",
                grammar_path,
                lexer_path,
                e
            )
        })?;
        Ok(Self { inner })
    }

    #[pyo3(signature = (input, skip_empty = false, collapse_single=false))]
    fn parse_pretty(
        &self,
        input: &str,
        skip_empty: bool,
        collapse_single: bool,
    ) -> anyhow::Result<String> {
        let parse = self
            .inner
            .parse(input, skip_empty, collapse_single)
            .map_err(|e| anyhow!("failed to parse input: {e}"))?;
        Ok(parse.pretty(input, skip_empty, collapse_single))
    }

    #[pyo3(signature = (input, skip_empty = false, collapse_single=false))]
    fn parse(
        slf: PyRef<'_, Self>,
        py: Python<'_>,
        input: &str,
        skip_empty: bool,
        collapse_single: bool,
    ) -> anyhow::Result<PyObject> {
        let parse = slf
            .inner
            .parse(input, skip_empty, collapse_single)
            .map_err(|e| anyhow!("failed to parse input: {e}"))?;
        Ok(parse_into_py(input, &parse, py)?)
    }

    fn lex(&self, input: &str) -> anyhow::Result<Vec<TokenAndSpan>> {
        self.inner
            .lex(input)
            .map_err(|e| anyhow!("failed to lex input: {e}"))
    }
}

fn parse_into_py(text: &str, parse: &LR1Parse<'_>, py: Python<'_>) -> PyResult<PyObject> {
    let dict = PyDict::new_bound(py);
    match parse {
        LR1Parse::Empty(name) => {
            dict.set_item("name", name)?;
        }
        LR1Parse::Terminal(name, span) => {
            dict.set_item("name", name)?;
            let &(start, len) = span;
            dict.set_item("value", &text[start..start + len])?;
            dict.set_item("byte_span", (start, start + len))?;
        }
        LR1Parse::NonTerminal(name, children) => {
            dict.set_item("name", name)?;
            let children = PyList::new_bound(
                py,
                children
                    .iter()
                    .map(|c| parse_into_py(text, c, py))
                    .collect::<PyResult<Vec<_>>>()?,
            );
            dict.set_item("children", children)?;
        }
    };
    Ok(dict.into())
}

/// A submodule containing python implementations of regex and CFG (LR1) constraints
pub(super) fn add_submodule(py: Python, parent_module: &Bound<'_, PyModule>) -> PyResult<()> {
    let m = PyModule::new_bound(py, "grammar")?;
    m.add_class::<RegexConstraint>()?;
    m.add_class::<LR1Constraint>()?;
    m.add_class::<LR1Parser>()?;
    parent_module.add_submodule(&m)?;

    Ok(())
}
