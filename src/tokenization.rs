use crate::unicode::CS;
use crate::utils::{py_invalid_type_error, py_required_key_error, run_length_decode};
use anyhow::anyhow;
use itertools::Itertools;
use numpy::ndarray::{Array1, Array2};
use numpy::IntoPyArray;
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList, PyTuple};
use std::borrow::Borrow;
use std::collections::HashMap;
use std::hash::Hash;
use std::thread::sleep;
use std::time::Duration;

pub const UNK: &str = "<unk>";
pub const BOS: &str = "<bos>";
pub const EOS: &str = "<eos>";
pub const PAD: &str = "<pad>";
pub const SPECIAL_TOKENS: [&str; 4] = [UNK, BOS, EOS, PAD];
pub const DEFAULT_PREFIX_TOKENS: [&str; 1] = [BOS];
pub const DEFAULT_SUFFIX_TOKENS: [&str; 1] = [EOS];

#[pyclass]
pub struct SpecialTokens {}

#[pymethods]
impl SpecialTokens {
    #[classattr]
    const UNK: &str = UNK;
    #[classattr]
    const BOS: &str = BOS;
    #[classattr]
    const EOS: &str = EOS;
    #[classattr]
    const PAD: &str = PAD;
}

// language tokens
pub const LANG_UNK: &str = "[unk]";

#[pyclass]
pub struct LanguageTokens {}

#[pymethods]
impl LanguageTokens {
    #[classattr]
    const UNK: &str = LANG_UNK;
}

/// Config for special tokens and options regarding special tokens
#[derive(Debug, Clone)]
pub struct SpecialConfig {
    pub pad: String,
    pub tokens: Vec<String>,
    pub prefix: Vec<String>,
    pub suffix: Vec<String>,
}

impl Default for SpecialConfig {
    fn default() -> Self {
        Self {
            pad: PAD.to_string(),
            tokens: SPECIAL_TOKENS.iter().map(|s| s.to_string()).collect(),
            prefix: DEFAULT_PREFIX_TOKENS
                .iter()
                .map(|s| s.to_string())
                .collect(),
            suffix: DEFAULT_SUFFIX_TOKENS
                .iter()
                .map(|s| s.to_string())
                .collect(),
        }
    }
}

impl<'a> FromPyObject<'a> for SpecialConfig {
    fn extract(ob: &'a PyAny) -> PyResult<Self> {
        let d: &PyDict = ob.extract()?;
        Ok(Self {
            pad: if let Some(value) = d.get_item("pad") {
                value.extract()?
            } else {
                PAD.to_string()
            },
            tokens: if let Some(value) = d.get_item("tokens") {
                value.extract()?
            } else {
                SPECIAL_TOKENS.iter().map(|s| s.to_string()).collect()
            },
            prefix: if let Some(value) = d.get_item("prefix") {
                value.extract()?
            } else {
                DEFAULT_PREFIX_TOKENS
                    .iter()
                    .map(|s| s.to_string())
                    .collect()
            },
            suffix: if let Some(value) = d.get_item("suffix") {
                value.extract()?
            } else {
                DEFAULT_SUFFIX_TOKENS
                    .iter()
                    .map(|s| s.to_string())
                    .collect()
            },
        })
    }
}

/// This is a tokenizer config, containing language options
/// and the actual tokenize config inside it.
#[derive(Clone, Debug)]
pub struct TokenizerConfig {
    pub tokenize: TokenizeConfig,
    pub special: SpecialConfig,
    pub language: Option<LanguageConfig>,
}

impl<'a> FromPyObject<'a> for TokenizerConfig {
    fn extract(ob: &'a PyAny) -> PyResult<Self> {
        let d: &PyDict = ob.extract()?;
        Ok(Self {
            tokenize: d
                .get_item("tokenize")
                .ok_or_else(|| py_required_key_error("tokenize", "tokenizer config"))?
                .extract()?,
            special: if let Some(value) = d.get_item("special") {
                value.extract()?
            } else {
                SpecialConfig::default()
            },
            language: if let Some(value) = d.get_item("language") {
                Some(value.extract()?)
            } else {
                None
            },
        })
    }
}

/// This configures the language a tokenizer can work with
#[derive(Clone, Debug)]
pub struct LanguageConfig {
    add_language_token_to_prefix: bool,
    add_language_token_to_suffix: bool,
    languages: Vec<String>,
    default_language: String,
}

impl LanguageConfig {
    pub fn new(
        add_language_token_to_prefix: bool,
        add_language_token_to_suffix: bool,
        languages: Vec<String>,
        default_language: String,
    ) -> Self {
        Self {
            add_language_token_to_prefix,
            add_language_token_to_suffix,
            languages,
            default_language,
        }
    }
}

impl<'a> FromPyObject<'a> for LanguageConfig {
    fn extract(ob: &'a PyAny) -> PyResult<Self> {
        let d: &PyDict = ob.extract()?;
        Ok(Self {
            add_language_token_to_prefix: if let Some(value) =
                d.get_item("add_language_token_to_prefix")
            {
                value.extract()?
            } else {
                true
            },
            add_language_token_to_suffix: if let Some(value) =
                d.get_item("add_language_token_to_suffix")
            {
                value.extract()?
            } else {
                false
            },
            languages: if let Some(value) = d.get_item("languages") {
                value.extract()?
            } else {
                vec![]
            },
            default_language: if let Some(value) = d.get_item("default_language") {
                value.extract()?
            } else {
                LANG_UNK.to_string()
            },
        })
    }
}

/// This enum defines all tokenizers that are supported by this crate.
#[derive(Clone, Debug)]
pub enum TokenizeConfig {
    Character(CharTokenizerConfig),
    Byte(ByteTokenizerConfig),
    ByT5(ByteTokenizerConfig),
    BPE(BPETokenizerConfig),
    Dummy(Duration),
}

impl IntoPy<PyObject> for TokenizeConfig {
    fn into_py(self, py: Python<'_>) -> PyObject {
        let d: &PyDict = PyDict::new(py);
        let tokenizer_type = match self {
            TokenizeConfig::Character(cfg) => {
                d.set_item("use_graphemes", cfg.use_graphemes).unwrap();
                "character"
            }
            TokenizeConfig::Byte(cfg) => {
                d.set_item("use_graphemes", cfg.use_graphemes).unwrap();
                d.set_item("groups", cfg.groups.into_py(py)).unwrap();
                d.set_item("aggregation", cfg.aggregation.into_py(py))
                    .unwrap();
                "byte"
            }
            TokenizeConfig::ByT5(cfg) => {
                d.set_item("use_graphemes", cfg.use_graphemes).unwrap();
                d.set_item("groups", cfg.groups.into_py(py)).unwrap();
                d.set_item("aggregation", cfg.aggregation.into_py(py))
                    .unwrap();
                "byt5"
            }
            TokenizeConfig::BPE(cfg) => {
                d.set_item("use_graphemes", cfg.use_graphemes).unwrap();
                "bpe"
            }
            TokenizeConfig::Dummy(delay) => {
                d.set_item("delay", delay.as_millis()).unwrap();
                "dummy"
            }
        };
        d.set_item("type", tokenizer_type).unwrap();
        d.to_object(py)
    }
}

impl<'a> FromPyObject<'a> for TokenizeConfig {
    fn extract(ob: &'a PyAny) -> PyResult<Self> {
        let d: &PyDict = ob.extract()?;
        let Some(tokenizer_type) = d.get_item("type") else {
            return Err(py_required_key_error("type", "tokenizer config"));
        };
        let tokenizer_type: String = tokenizer_type.extract()?;
        let tokenizer_config = match tokenizer_type.as_str() {
            "character" => {
                let use_graphemes: bool = if let Some(value) = d.get_item("use_graphemes") {
                    value.extract()?
                } else {
                    true
                };
                TokenizeConfig::Character(CharTokenizerConfig { use_graphemes })
            }
            name @ ("byte" | "byt5") => {
                let use_graphemes: bool = if let Some(value) = d.get_item("use_graphemes") {
                    value.extract()?
                } else {
                    true
                };
                let Some(groups) = d.get_item("groups") else {
                    return Err(py_required_key_error("groups", format!("{name} tokenizer config")));
                };
                let agg: GroupAggregation = if let Some(value) = d.get_item("aggregation") {
                    value.extract()?
                } else {
                    GroupAggregation::Mean
                };
                let byte_cfg = ByteTokenizerConfig {
                    use_graphemes,
                    groups: groups.extract()?,
                    aggregation: agg,
                };
                if name == "byt5" {
                    TokenizeConfig::ByT5(byte_cfg)
                } else {
                    TokenizeConfig::Byte(byte_cfg)
                }
            }
            "bpe" => {
                let use_graphemes: bool = if let Some(value) = d.get_item("use_graphemes") {
                    value.extract()?
                } else {
                    true
                };
                TokenizeConfig::BPE(BPETokenizerConfig { use_graphemes })
            }
            "dummy" => {
                let millis: u64 = if let Some(value) = d.get_item("delay") {
                    value.extract()?
                } else {
                    0
                };
                TokenizeConfig::Dummy(Duration::from_millis(millis))
            }
            k => {
                return Err(py_invalid_type_error(k, "tokenizer"));
            }
        };
        Ok(tokenizer_config)
    }
}

pub type Grouping = (Vec<Vec<usize>>, GroupAggregation);
/// This enum defines all possible additional infos that can be returned by
/// a tokenizers tokenize function in addition to the token ids themselves.
#[derive(Clone, Debug)]
pub enum TokenizationInfo {
    /// No additional info.
    Empty,
    /// Token groups specify which subsequent tokens belong to the same group.
    /// Useful e.g. when defining a byte tokenizer that should also return
    /// information about which byte belongs to which character.
    TokenGroups(HashMap<String, Grouping>),
}

pub enum TensorizedTokenizationInfo {
    Empty,
    TokenGroups(HashMap<String, (SparseCoo, PaddingMask)>),
}

pub struct SparseCoo {
    indices: Array2<i32>,
    values: Array1<f32>,
    size: Vec<usize>,
    pub(crate) group_lengths: Vec<usize>,
}

impl IntoPy<PyObject> for SparseCoo {
    fn into_py(self, py: Python<'_>) -> PyObject {
        (
            self.indices.into_pyarray(py),
            self.values.into_pyarray(py),
            self.size,
            self.group_lengths,
        )
            .into_py(py)
    }
}

pub struct PaddingMask {
    inner: Array2<bool>,
}

impl IntoPy<PyObject> for PaddingMask {
    fn into_py(self, py: Python<'_>) -> PyObject {
        self.inner.into_pyarray(py).into_py(py)
    }
}

impl IntoPy<PyObject> for TensorizedTokenizationInfo {
    fn into_py(self, py: Python<'_>) -> PyObject {
        match self {
            TensorizedTokenizationInfo::Empty => PyDict::new(py),
            TensorizedTokenizationInfo::TokenGroups(matrices) => {
                let d = PyDict::new(py);
                for (name, (scoo, pad_mask)) in matrices {
                    let t = PyTuple::new(py, &[scoo.into_py(py), pad_mask.into_py(py)]);
                    d.set_item(name, t).unwrap();
                }
                d
            }
        }
        .into_py(py)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GroupAggregation {
    Mean,
    Sum,
}

impl IntoPy<PyObject> for GroupAggregation {
    fn into_py(self, py: Python<'_>) -> PyObject {
        match self {
            GroupAggregation::Mean => "mean",
            GroupAggregation::Sum => "sum",
        }
        .into_py(py)
    }
}

impl ToPyObject for GroupAggregation {
    fn to_object(&self, py: Python<'_>) -> PyObject {
        self.into_py(py)
    }
}

impl<'a> FromPyObject<'a> for GroupAggregation {
    fn extract(ob: &'a PyAny) -> PyResult<Self> {
        let agg: String = ob.extract()?;
        match agg.as_str() {
            "mean" => Ok(GroupAggregation::Mean),
            "sum" => Ok(GroupAggregation::Sum),
            k => Err(py_invalid_type_error(k, "group aggregation")),
        }
    }
}

type Values = Vec<Vec<f32>>;
type Indices = Vec<Vec<i32>>;
#[inline]
fn expand_grouping(s_idx: usize, groups: &Vec<Vec<usize>>, pow: i32) -> (Values, Indices, Indices) {
    let num_groups = groups[s_idx].len();
    if s_idx > 0 {
        let mut new_weights = vec![vec![]; num_groups];
        let mut new_group_indices = vec![];
        let mut new_seq_indices = vec![vec![]; num_groups];
        let (prev_weights, _, mut prev_seq_indices) = expand_grouping(s_idx - 1, groups, pow);
        assert_eq!(prev_weights.len(), groups[s_idx].iter().sum::<usize>());
        let mut cum_g = 0;
        for (i, &g) in groups[s_idx].iter().enumerate() {
            let fac = (g as f32).powi(pow);
            for j in cum_g..cum_g + g {
                new_weights[i].extend(prev_weights[j].iter().map(|w| w * fac));
                new_seq_indices[i].append(&mut prev_seq_indices[j]);
            }
            new_group_indices.push(vec![i as i32; new_weights[i].len()]);
            cum_g += g;
        }
        (new_weights, new_group_indices, new_seq_indices)
    } else {
        let mut weights = vec![];
        let mut group_indices = vec![];
        let mut seq_indices = vec![];
        let mut cum_g = 0;
        for (i, &g) in groups[s_idx].iter().enumerate() {
            let fac = (g as f32).powi(pow);
            weights.push(vec![fac; g]);
            group_indices.push(vec![i as i32; g]);
            let cum_g_i = cum_g as i32;
            seq_indices.push((cum_g_i..cum_g_i + g as i32).collect());
            cum_g += g;
        }
        (weights, group_indices, seq_indices)
    }
}

#[inline]
fn group_values(grouping: &Grouping) -> (Vec<f32>, Vec<i32>, Vec<i32>, usize) {
    let (groups, agg) = grouping;
    assert!(!groups.is_empty());
    let pow = match agg {
        GroupAggregation::Mean => -1,
        GroupAggregation::Sum => 0,
    };
    let s_idx = groups.len() - 1;
    let num_groups = groups[s_idx].len();
    let (values, group_indices, seq_indices) = expand_grouping(s_idx, groups, pow);
    (
        values.into_iter().flatten().collect(),
        group_indices.into_iter().flatten().collect(),
        seq_indices.into_iter().flatten().collect(),
        num_groups,
    )
}

pub fn token_groups_to_sparse_coo_matrix(
    groupings: &[&Grouping],
    lengths: &[usize],
) -> anyhow::Result<SparseCoo> {
    let mut indices = vec![vec![]; 3];
    let mut values = vec![];
    let mut group_lengths = vec![];

    for (i, grouping) in groupings.iter().enumerate() {
        let (mut v, mut g, mut s, l) = group_values(grouping);
        values.append(&mut v);
        indices[0].append(&mut vec![i as i32; g.len()]);
        indices[1].append(&mut g);
        indices[2].append(&mut s);
        group_lengths.push(l);
    }

    let max_group_length = group_lengths.iter().max().copied().unwrap_or(0);
    let max_length = lengths.iter().max().copied().unwrap_or(0);
    let size = vec![groupings.len(), max_group_length, max_length];
    Ok(SparseCoo {
        indices: Array2::from_shape_vec(
            (3, indices[0].len()),
            indices.into_iter().flatten().collect(),
        )?,
        values: Array1::from_vec(values),
        size,
        group_lengths,
    })
}

pub fn padding_mask(lengths: &[usize]) -> anyhow::Result<PaddingMask> {
    let max_length = lengths.iter().max().copied().unwrap_or(0);
    let padding_mask_vec: Vec<_> = lengths
        .iter()
        .flat_map(|l| {
            let mut pad = vec![false; *l];
            pad.append(&mut vec![true; max_length - l]);
            pad
        })
        .collect();
    Ok(PaddingMask {
        inner: Array2::from_shape_vec((lengths.len(), max_length), padding_mask_vec)?,
    })
}

impl IntoPy<PyObject> for TokenizationInfo {
    fn into_py(self, py: Python<'_>) -> PyObject {
        let d = PyDict::new(py);
        let info_type = match self {
            TokenizationInfo::Empty => "empty",
            TokenizationInfo::TokenGroups(token_groups) => {
                for (group_name, (stages, agg)) in token_groups.iter() {
                    let l = PyList::empty(py);
                    for groups in stages.iter() {
                        l.append(groups).unwrap();
                    }
                    let gd = PyDict::new(py);
                    gd.set_item("groups", l).unwrap();
                    gd.set_item("aggregation", agg.into_py(py)).unwrap();
                    d.set_item(group_name, gd).unwrap();
                }
                "token_groups"
            }
        };
        d.set_item("type", info_type).unwrap();
        d.into()
    }
}

impl<'a> FromPyObject<'a> for TokenizationInfo {
    fn extract(ob: &'a PyAny) -> PyResult<Self> {
        let d: &PyDict = ob.extract()?;
        let Some(info_type) = d.get_item("type") else {
            return Err(py_required_key_error("type", "tokenization info"));
        };
        let info_type: String = info_type.extract()?;
        let info = match info_type.as_str() {
            "empty" => TokenizationInfo::Empty,
            "token_groups" => {
                let mut token_groups = HashMap::new();
                for key in d.keys() {
                    let key_s: String = key.extract()?;
                    if key_s == "type" {
                        continue;
                    }
                    let gd = d.get_item(key).unwrap();
                    let groups = gd.get_item("groups")?.extract()?;
                    let agg = gd.get_item("aggregation")?.extract()?;
                    token_groups.insert(key_s, (groups, agg));
                }
                TokenizationInfo::TokenGroups(token_groups)
            }
            k => return Err(py_invalid_type_error(k, "tokenization info")),
        };
        Ok(info)
    }
}

/// A tokenization is defined to be a combination of token ids and some additional information.
/// This is returned by a tokenizers tokenize function.
#[derive(Debug, Clone)]
#[pyclass]
pub struct Tokenization {
    #[pyo3(get)]
    pub token_ids: Vec<u32>,
    #[pyo3(get)]
    pub info: TokenizationInfo,
}

impl Tokenization {
    pub fn new(token_ids: Vec<u32>, info: TokenizationInfo) -> Self {
        Tokenization { token_ids, info }
    }
}

/// A tokenization function in general takes in a &str and return a tokenization.
pub type TokenizationFn = Box<dyn Send + 'static + Fn(&str) -> Tokenization>;
/// A tokenizer is something that implements the tokenize trait
pub type Tokenizer = Box<dyn Send + 'static + Tokenize>;

/// The tokenize trait defines behavior that every tokenizer should support.
pub trait BaseTokenize: Send + Sync + 'static {
    fn num_prefix_tokens(&self) -> usize {
        self.prefix_token_ids().len()
            + match self.language_config().as_ref() {
                Some(cfg) => cfg.add_language_token_to_prefix as usize,
                None => 0,
            }
    }

    fn num_suffix_tokens(&self) -> usize {
        self.suffix_token_ids().len()
            + match self.language_config().as_ref() {
                Some(cfg) => cfg.add_language_token_to_suffix as usize,
                None => 0,
            }
    }

    fn prefix_token_ids(&self) -> &[u32];

    fn suffix_token_ids(&self) -> &[u32];

    fn pad_token_id(&self) -> u32;

    fn language_config(&self) -> Option<&LanguageConfig>;

    fn add_prefix_and_suffix(
        &self,
        mut token_ids: Vec<u32>,
        lang: Option<&str>,
    ) -> anyhow::Result<Vec<u32>> {
        let mut prefix = self.prefix_token_ids().to_vec();
        let mut suffix = self.suffix_token_ids().to_vec();
        if let Some(lang_cfg) = self.language_config() {
            let lang = lang.unwrap_or(&lang_cfg.default_language);
            let Some(lang_id) = self.special_token_to_id(lang) else {
                return Err(anyhow!(
                    "language {} is not supported by this tokenizer",
                    lang
                ));
            };
            if lang_cfg.add_language_token_to_prefix {
                prefix.push(lang_id);
            }
            if lang_cfg.add_language_token_to_suffix {
                suffix.push(lang_id);
            }
        }
        prefix.reserve_exact(token_ids.len() + suffix.len());
        prefix.append(&mut token_ids);
        prefix.append(&mut suffix);
        Ok(prefix)
    }

    fn special_token_to_id(&self, token: &str) -> Option<u32>;
}

pub trait Tokenize: BaseTokenize {
    fn vocab_size(&self) -> usize;

    fn tokenize(&self, s: &str, lang: Option<&str>) -> anyhow::Result<Tokenization>;

    fn de_tokenize(&self, token_ids: &[u32]) -> String;
}

/// A base struct for a tokenizer,
/// allows custom tokenizers to be built by setting config and state
pub struct BaseTokenizer<Config = (), State = ()> {
    prefix_token_ids: Vec<u32>,
    suffix_token_ids: Vec<u32>,
    pad_token_id: u32,
    state: State,
    config: Config,
    language_config: Option<LanguageConfig>,
    special_vocab: Vocab<String>,
}

impl<Config, State> BaseTokenize for BaseTokenizer<Config, State>
where
    Self: Send + Sync + 'static,
{
    fn prefix_token_ids(&self) -> &[u32] {
        &self.prefix_token_ids
    }

    fn suffix_token_ids(&self) -> &[u32] {
        &self.suffix_token_ids
    }

    fn pad_token_id(&self) -> u32 {
        self.pad_token_id
    }

    fn language_config(&self) -> Option<&LanguageConfig> {
        self.language_config.as_ref()
    }

    fn special_token_to_id(&self, token: &str) -> Option<u32> {
        self.special_vocab.token_to_id(token)
    }
}

impl<Config, State> BaseTokenizer<Config, State> {
    fn new_base_tokenizer(
        special_offset: u32,
        special_config: SpecialConfig,
        language_config: Option<LanguageConfig>,
        config: Config,
        state: State,
    ) -> Self {
        let languages = if let Some(lang_cfg) = language_config.as_ref() {
            let mut l = vec![lang_cfg.default_language.clone()];
            l.extend(lang_cfg.languages.iter().cloned());
            l
        } else {
            vec![]
        };
        let special_vocab = Vocab::build(
            special_config
                .tokens
                .into_iter()
                .chain(vec![special_config.pad.clone()].into_iter())
                .chain(special_config.prefix.clone().into_iter())
                .chain(special_config.suffix.clone().into_iter())
                .chain(languages.into_iter()),
            special_offset,
        );
        let prefix_token_ids = special_config
            .prefix
            .iter()
            .map(|tok| special_vocab.token_to_id(tok).unwrap())
            .collect();
        let suffix_token_ids = special_config
            .suffix
            .iter()
            .map(|tok| special_vocab.token_to_id(tok).unwrap())
            .collect();
        BaseTokenizer {
            prefix_token_ids,
            suffix_token_ids,
            pad_token_id: special_vocab.token_to_id(&special_config.pad).unwrap(),
            language_config,
            special_vocab,
            config,
            state,
        }
    }
}

pub struct Vocab<Token> {
    vocab: HashMap<Token, u32>,
    reverse_vocab: HashMap<u32, Token>,
}

impl<Token> Vocab<Token>
where
    Token: PartialEq + Eq + Hash + Clone,
{
    fn build(tokens: impl IntoIterator<Item = Token>, start_id: u32) -> Self {
        let vocab: HashMap<Token, u32> = tokens
            .into_iter()
            .unique()
            .enumerate()
            .map(|(tok_id, tok)| (tok, start_id + tok_id as u32))
            .collect();
        let reverse_vocab = vocab
            .iter()
            .map(|(token, token_id)| (*token_id, token.clone()))
            .collect();
        Self {
            vocab,
            reverse_vocab,
        }
    }

    fn size(&self) -> usize {
        self.vocab.len()
    }

    fn token_to_id<K>(&self, token: &K) -> Option<u32>
    where
        K: Hash + Eq + ?Sized,
        Token: Borrow<K>,
    {
        self.vocab.get(token).copied()
    }

    fn id_to_token(&self, id: &u32) -> Option<&Token> {
        self.reverse_vocab.get(id)
    }
}

pub type VocabFreeTokenizer<Config> = BaseTokenizer<Config>;

impl<Config> VocabFreeTokenizer<Config>
where
    Config: Send + Sync + 'static,
{
    pub fn new_vocab_free_tokenizer(
        special_offset: u32,
        special_config: SpecialConfig,
        language_config: Option<LanguageConfig>,
        config: Config,
    ) -> Self {
        Self::new_base_tokenizer(special_offset, special_config, language_config, config, ())
    }
}

pub type VocabTokenizer<Token, Config> = BaseTokenizer<Config, (String, Vocab<Token>)>;

trait VocabTokenize<Token> {
    fn split(&self, s: &str) -> (Vec<Option<Token>>, TokenizationInfo);

    fn join(&self, tokens: &[&Token]) -> String;
}

impl<Token, Config> VocabTokenizer<Token, Config>
where
    Token: PartialEq + Eq + Hash + Send + Sync + Clone + 'static,
    Config: Send + Sync + 'static,
{
    pub fn new_vocab_tokenizer(
        tokens: Vec<Token>,
        unk_token: String,
        mut special_config: SpecialConfig,
        language_config: Option<LanguageConfig>,
        config: Config,
    ) -> Self {
        let vocab = Vocab::build(tokens, 0);
        // add unk token to special config
        special_config.tokens.push(unk_token.clone());
        Self::new_base_tokenizer(
            vocab.size() as u32,
            special_config,
            language_config,
            config,
            (unk_token, vocab),
        )
    }

    pub fn unk_token_id(&self) -> u32 {
        self.special_vocab.token_to_id(&self.state.0).unwrap()
    }
}

impl<Token, Config> Tokenize for VocabTokenizer<Token, Config>
where
    Token: PartialEq + Eq + Hash + Send + Sync + Clone + 'static,
    Config: Send + Sync + 'static,
    Self: VocabTokenize<Token>,
{
    fn vocab_size(&self) -> usize {
        self.state.1.size() + self.special_vocab.size()
    }

    fn tokenize(&self, s: &str, lang: Option<&str>) -> anyhow::Result<Tokenization> {
        let (tokens, tokenization_info) = self.split(s);
        let token_ids = tokens
            .iter()
            .map(|token| {
                if let Some(token) = token {
                    self.state
                        .1
                        .token_to_id(token)
                        .unwrap_or_else(|| self.unk_token_id())
                } else {
                    self.unk_token_id()
                }
            })
            .collect::<Vec<_>>();
        let token_ids = self.add_prefix_and_suffix(token_ids, lang)?;
        Ok(Tokenization::new(token_ids, tokenization_info))
    }

    fn de_tokenize(&self, token_ids: &[u32]) -> String {
        let tokens = token_ids
            .iter()
            .filter_map(|token_id| self.state.1.id_to_token(token_id))
            .collect::<Vec<_>>();
        self.join(&tokens)
    }
}

/// Dummy tokenizer that just waits a specified time in its tokenize function.
/// Used for testing only.
pub type DummyTokenizer = VocabFreeTokenizer<Duration>;

impl DummyTokenizer {
    fn new(delay: Duration) -> Self {
        Self::new_vocab_free_tokenizer(0, SpecialConfig::default(), None, delay)
    }
}

impl Tokenize for DummyTokenizer {
    fn vocab_size(&self) -> usize {
        0
    }

    fn tokenize(&self, _: &str, _: Option<&str>) -> anyhow::Result<Tokenization> {
        sleep(self.config);
        Ok(Tokenization::new(vec![], TokenizationInfo::Empty))
    }

    fn de_tokenize(&self, _: &[u32]) -> String {
        "".to_string()
    }
}

/// A tokenizer based on the ascii characters, digits, and punctuations marks.
/// Can e.g. be used to efficiently (meaning small vocab size) represent most
/// English texts.
#[derive(Debug, Clone)]
pub struct CharTokenizerConfig {
    use_graphemes: bool,
}
pub type CharTokenizer = VocabTokenizer<char, CharTokenizerConfig>;

const CHARS: &str = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789\"\"!\"#$%&\'()*+,-./:;<=>?@[\\]^_`{|}~\"\" ";

impl VocabTokenize<char> for CharTokenizer {
    fn split(&self, s: &str) -> (Vec<Option<char>>, TokenizationInfo) {
        let tokens = CS::new(s, self.config.use_graphemes)
            .chars()
            .map(|c| {
                // Character always has at least one char so this is safe
                let mut c_iter = c.code_points();
                let char = c_iter.next().unwrap();
                // return unk if Character has another char because
                // our tokens in the vocab are all single char tokens
                if c_iter.next().is_some() {
                    None
                } else {
                    Some(char)
                }
            })
            .collect();
        (tokens, TokenizationInfo::Empty)
    }

    fn join(&self, tokens: &[&char]) -> String {
        tokens.iter().join("")
    }
}

impl CharTokenizer {
    pub fn new(
        config: CharTokenizerConfig,
        special_config: SpecialConfig,
        language_config: Option<LanguageConfig>,
    ) -> Self {
        Self::new_vocab_tokenizer(
            CHARS.chars().collect(),
            UNK.to_string(),
            special_config,
            language_config,
            config,
        )
    }
}

#[derive(Debug, Clone)]
pub struct BPETokenizerConfig {
    use_graphemes: bool,
}

pub type BPETokenizer = VocabTokenizer<Vec<u8>, BPETokenizerConfig>;

impl BPETokenizer {
    pub fn new(
        config: BPETokenizerConfig,
        special_config: SpecialConfig,
        language_config: Option<LanguageConfig>,
    ) -> Self {
        todo!()
    }
}

impl VocabTokenize<Vec<u8>> for BPETokenizer {
    fn split(&self, s: &str) -> (Vec<Option<Vec<u8>>>, TokenizationInfo) {
        todo!()
    }

    fn join(&self, tokens: &[&Vec<u8>]) -> String {
        todo!()
    }
}

#[derive(Clone, Debug)]
pub enum ByteGroups {
    Bytes,
    CodePoints,
}

impl IntoPy<PyObject> for ByteGroups {
    fn into_py(self, py: Python) -> PyObject {
        match self {
            ByteGroups::Bytes => "bytes",
            ByteGroups::CodePoints => "code_points",
        }
        .into_py(py)
    }
}

impl<'a> FromPyObject<'a> for ByteGroups {
    fn extract(ob: &'a PyAny) -> PyResult<Self> {
        let s: String = ob.extract()?;
        let groups = match s.as_str() {
            "bytes" => ByteGroups::Bytes,
            "code_points" => ByteGroups::CodePoints,
            k => return Err(py_invalid_type_error(k, "byte groups")),
        };
        Ok(groups)
    }
}

#[derive(Clone, Debug)]
pub struct ByteTokenizerConfig {
    use_graphemes: bool,
    groups: ByteGroups,
    aggregation: GroupAggregation,
}

pub type ByteTokenizer = VocabFreeTokenizer<ByteTokenizerConfig>;

impl ByteTokenizer {
    pub fn new(
        config: ByteTokenizerConfig,
        special_config: SpecialConfig,
        language_config: Option<LanguageConfig>,
    ) -> Self {
        Self::new_with(config, special_config, language_config)
    }

    fn new_with(
        config: ByteTokenizerConfig,
        special_config: SpecialConfig,
        language_config: Option<LanguageConfig>,
    ) -> Self {
        Self::new_vocab_free_tokenizer(256, special_config, language_config, config)
    }

    fn split(&self, s: &str) -> (Vec<u32>, HashMap<String, Grouping>) {
        let tokens = s.as_bytes().iter().map(|b| *b as u32).collect();
        let groups = match self.config.groups {
            ByteGroups::Bytes => {
                let cs = CS::new(s, self.config.use_graphemes);
                let mut groups = vec![1; self.num_prefix_tokens()];
                groups.extend(run_length_decode(&cs.rle_cluster_lengths));
                groups.extend(vec![1; self.num_suffix_tokens()]);
                HashMap::from([(
                    "byte_groups".to_string(),
                    (vec![groups], self.config.aggregation),
                )])
            }
            ByteGroups::CodePoints => {
                let cs = CS::new(s, self.config.use_graphemes);
                let mut byte_groups = vec![1; self.num_prefix_tokens()];
                let mut code_point_groups = vec![1; self.num_prefix_tokens()];
                for char in cs.chars() {
                    let mut num_chars = 0;
                    for code_point in char.code_points() {
                        byte_groups.push(code_point.len_utf8());
                        num_chars += 1;
                    }
                    code_point_groups.push(num_chars);
                }
                byte_groups.extend(vec![1; self.num_suffix_tokens()]);
                code_point_groups.extend(vec![1; self.num_suffix_tokens()]);
                HashMap::from([(
                    "code_point_groups".to_string(),
                    (
                        vec![byte_groups, code_point_groups],
                        self.config.aggregation,
                    ),
                )])
            }
        };
        (tokens, groups)
    }
}

impl Tokenize for ByteTokenizer {
    fn vocab_size(&self) -> usize {
        256 + self.special_vocab.size()
    }

    fn tokenize(&self, s: &str, lang: Option<&str>) -> anyhow::Result<Tokenization> {
        let (bytes, token_groups) = self.split(s);

        Ok(Tokenization::new(
            self.add_prefix_and_suffix(bytes, lang)?,
            TokenizationInfo::TokenGroups(token_groups),
        ))
    }

    fn de_tokenize(&self, token_ids: &[u32]) -> String {
        let bytes: Vec<u8> = token_ids
            .iter()
            .filter_map(|t| if *t < 256u32 { Some(*t as u8) } else { None })
            .collect();
        String::from_utf8_lossy(&bytes).to_string()
    }
}

pub struct ByT5Tokenizer {
    inner: ByteTokenizer,
}

impl ByT5Tokenizer {
    pub fn new(config: ByteTokenizerConfig) -> Self {
        let inner = ByteTokenizer::new_with(
            config,
            SpecialConfig {
                pad: "<pad>".into(),
                tokens: vec!["<pad>".into(), "</s>".into(), "<unk>".into()],
                prefix: vec![],
                suffix: vec!["</s>".into()],
            },
            None,
        );
        Self { inner }
    }
}

impl BaseTokenize for ByT5Tokenizer {
    fn prefix_token_ids(&self) -> &[u32] {
        self.inner.prefix_token_ids()
    }

    fn suffix_token_ids(&self) -> &[u32] {
        self.inner.suffix_token_ids()
    }

    fn language_config(&self) -> Option<&LanguageConfig> {
        self.inner.language_config()
    }

    fn special_token_to_id(&self, token: &str) -> Option<u32> {
        self.inner.special_token_to_id(token)
    }

    fn pad_token_id(&self) -> u32 {
        self.inner.pad_token_id()
    }
}

impl Tokenize for ByT5Tokenizer {
    fn vocab_size(&self) -> usize {
        self.inner.vocab_size()
    }

    fn tokenize(&self, s: &str, lang: Option<&str>) -> anyhow::Result<Tokenization> {
        let Tokenization { token_ids, info } = self.inner.tokenize(s, lang)?;
        // adapt token ids to byt5 format
        // --> shift byte token_ids from 0..255 to 3..258
        // --> shift eos token
        let num_token_ids = token_ids.len();
        let mut token_ids: Vec<_> = token_ids
            .into_iter()
            .take(num_token_ids - 1)
            .map(|t| {
                assert!(t < 256);
                t + 3
            })
            .collect();
        token_ids.push(1);
        Ok(Tokenization::new(token_ids, info))
    }

    fn de_tokenize(&self, token_ids: &[u32]) -> String {
        self.inner.de_tokenize(token_ids)
    }
}

pub fn tokenizer(cfg: TokenizerConfig) -> Tokenizer {
    match cfg.tokenize {
        TokenizeConfig::Character(char_cfg) => {
            Box::new(CharTokenizer::new(char_cfg, cfg.special, cfg.language))
        }
        TokenizeConfig::Byte(byte_cfg) => {
            Box::new(ByteTokenizer::new(byte_cfg, cfg.special, cfg.language))
        }
        TokenizeConfig::ByT5(byte_cfg) => Box::new(ByT5Tokenizer::new(byte_cfg)),
        TokenizeConfig::BPE(bpe_cfg) => {
            Box::new(BPETokenizer::new(bpe_cfg, cfg.special, cfg.language))
        }
        TokenizeConfig::Dummy(d) => Box::new(DummyTokenizer::new(d)),
    }
}

#[pyclass]
#[pyo3(name = "Tokenizer")]
struct PyTokenizer {
    tokenizer: Tokenizer,
}

#[pymethods]
impl PyTokenizer {
    #[staticmethod]
    fn from_config(config: TokenizerConfig) -> Self {
        PyTokenizer {
            tokenizer: tokenizer(config),
        }
    }

    #[pyo3(signature = (s, lang = None))]
    fn tokenize(&self, s: &str, lang: Option<&str>) -> anyhow::Result<Tokenization> {
        self.tokenizer.tokenize(s, lang)
    }

    fn special_token_to_id(&self, token: &str) -> Option<u32> {
        self.tokenizer.special_token_to_id(token)
    }

    fn de_tokenize(&self, token_ids: Vec<u32>) -> String {
        self.tokenizer.de_tokenize(&token_ids)
    }

    fn vocab_size(&self) -> usize {
        self.tokenizer.vocab_size()
    }

    fn num_prefix_tokens(&self) -> usize {
        self.tokenizer.num_prefix_tokens()
    }

    fn num_suffix_tokens(&self) -> usize {
        self.tokenizer.num_suffix_tokens()
    }
}

/// A submodule containing functionality to tokenize text into tokens.
/// Currently supported tokenization schemes are:
/// - character level tokenization
/// - byte level tokenization
pub(super) fn add_submodule(py: Python<'_>, parent_module: &PyModule) -> PyResult<()> {
    let m = PyModule::new(py, "tokenization")?;
    m.add_class::<PyTokenizer>()?;
    m.add_class::<Tokenization>()?;
    m.add_class::<SpecialTokens>()?;
    m.add_class::<LanguageTokens>()?;
    parent_module.add_submodule(m)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use numpy::ndarray::{Array1, Array2};

    use crate::tokenization::{
        ByteGroups, ByteTokenizer, CharTokenizer, SparseCoo, Tokenization, TokenizationInfo,
        Tokenize, BOS, EOS,
    };

    use super::{token_groups_to_sparse_coo_matrix, ByT5Tokenizer, GroupAggregation};

    #[test]
    fn test_char_tokenizer() {
        let pfx = vec![BOS];
        let sfx = vec![EOS];
        let tok = CharTokenizer::new_vocab_tokenizer(true, &pfx, &sfx, None);
        let text = "a täst";
        let Tokenization { token_ids, .. } = tok.tokenize(text, None).unwrap();
        assert_eq!(token_ids.len(), 6 + 2);
        assert_eq!(token_ids[4], tok.unk_token_id());
        assert_eq!(tok.de_tokenize(&token_ids), "a tst".to_string());
    }

    #[test]
    fn test_byte_tokenizer() {
        let pfx = vec![BOS];
        let sfx = vec![EOS];
        let tok = ByteTokenizer::new(
            true,
            ByteGroups::Bytes,
            GroupAggregation::Mean,
            &pfx,
            &sfx,
            None,
        );
        let text = "a täst";
        let Tokenization { token_ids, info } = tok.tokenize(text, None).unwrap();
        assert_eq!(
            token_ids[1..token_ids.len() - 1]
                .iter()
                .map(|tok| *tok as u8)
                .collect::<Vec<u8>>(),
            text.as_bytes().clone()
        );
        match info {
            TokenizationInfo::Empty => panic!("wrong info"),
            TokenizationInfo::TokenGroups(groups) => {
                assert_eq!(
                    groups,
                    HashMap::from([(
                        "byte_groups".to_string(),
                        (vec![vec![1, 1, 1, 1, 2, 1, 1, 1]], GroupAggregation::Mean)
                    )])
                )
            }
        };
        assert_eq!(token_ids.len(), 7 + 2);
        assert_eq!(tok.de_tokenize(&token_ids), text.to_string());
        let tok = ByteTokenizer::new(
            true,
            ByteGroups::CodePoints,
            GroupAggregation::Mean,
            &pfx,
            &sfx,
            None,
        );
        let text = "a täst";
        let Tokenization { token_ids, info } = tok.tokenize(text, None).unwrap();
        assert_eq!(
            token_ids[1..token_ids.len() - 1]
                .iter()
                .map(|tok| *tok as u8)
                .collect::<Vec<u8>>(),
            text.as_bytes().clone()
        );
        match info {
            TokenizationInfo::Empty => panic!("wrong info"),
            TokenizationInfo::TokenGroups(groups) => {
                assert_eq!(
                    groups,
                    HashMap::from([(
                        "code_point_groups".to_string(),
                        (
                            vec![vec![1, 1, 1, 1, 2, 1, 1, 1], vec![1, 1, 1, 1, 1, 1, 1, 1]],
                            GroupAggregation::Mean
                        )
                    )])
                )
            }
        };
    }

    #[test]
    fn test_byt5_tokenizer() {
        let tok = ByT5Tokenizer::new(true, ByteGroups::Bytes, GroupAggregation::Mean);
        let Tokenization { token_ids, info: _ } = tok.tokenize("a täst", None).unwrap();
        assert_eq!(token_ids, vec![100, 35, 119, 198, 167, 118, 119, 1]);
    }

    #[test]
    fn test_token_groups_to_sparse_coo_matrix() {
        // one stage grouping
        let grouping = (vec![vec![1, 1, 1, 1, 2, 1, 1, 1]], GroupAggregation::Mean);
        let SparseCoo {
            indices,
            values,
            size,
            group_lengths,
        } = token_groups_to_sparse_coo_matrix(&[&grouping], &[9]).unwrap();
        assert_eq!(
            indices,
            Array2::from_shape_vec(
                (3, 9),
                vec![
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 2, 3, 4, 4, 5, 6, 7, 0, 1, 2, 3, 4, 5, 6, 7, 8
                ]
            )
            .unwrap()
        );
        assert_eq!(size, vec![1, 8, 9]);
        assert_eq!(
            values,
            Array1::from_vec(vec![1.0, 1.0, 1.0, 1.0, 0.5, 0.5, 1.0, 1.0, 1.0])
        );
        assert_eq!(group_lengths, vec![8]);

        // two stage grouping
        let grouping = (
            vec![vec![1, 1, 1, 1, 2, 1, 1, 1], vec![4, 4]],
            GroupAggregation::Mean,
        );
        let SparseCoo {
            indices,
            values,
            size,
            group_lengths,
        } = token_groups_to_sparse_coo_matrix(&[&grouping], &[9]).unwrap();
        assert_eq!(
            indices,
            Array2::from_shape_vec(
                (3, 9),
                vec![
                    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 1, 0, 1, 2, 3, 4, 5, 6, 7, 8
                ]
            )
            .unwrap()
        );
        assert_eq!(size, vec![1, 2, 9]);
        assert_eq!(
            values,
            Array1::from_vec(vec![0.25, 0.25, 0.25, 0.25, 0.125, 0.125, 0.25, 0.25, 0.25])
        );
        assert_eq!(group_lengths, vec![2]);
    }
}
