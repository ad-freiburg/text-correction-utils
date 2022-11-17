use std::fs::read_to_string;
use std::path::Path;
use std::sync::{Arc, Mutex};
use serde::{Deserialize, Serialize};
use crate::tokenization::{Tokenization, tokenizer, Tokenizer, TokenizerConfig};
use crate::data::preprocessing::{labeling, LabelingConfig, LabelingFn, preprocessing, PreprocessingConfig, PreprocessingFn};

pub mod preprocessing;
pub mod loading;

#[derive(Clone, Debug, PartialEq)]
pub struct TextData {
    original: String,
    processed: String,
    language: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Label {
    Classification(usize),
    SeqClassification(Vec<usize>),
    Seq2Seq(Vec<usize>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    data: TextData,
    tokenization: Tokenization,
    label: Option<Label>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Batch {
    items: Vec<Item>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct PipelineConfig {
    preprocessing: Vec<PreprocessingConfig>,
    labeling: Option<LabelingConfig>,
    tokenizer: TokenizerConfig,
}

pub struct Pipeline {
    // Preprocessing a FnMut so we have to wrap it here to be thread safe
    preprocessing_fn: Arc<Mutex<PreprocessingFn>>,
    label_fn: Option<LabelingFn>,
    tokenizer: Tokenizer,
}

impl Pipeline {
    pub fn new(cfg: PipelineConfig) -> Self {
        Pipeline {
            preprocessing_fn: Arc::new(Mutex::new(preprocessing(cfg.preprocessing))),
            label_fn: if cfg.labeling.is_some() {
                Some(labeling(cfg.labeling.unwrap()))
            } else {
                None
            },
            tokenizer: tokenizer(cfg.tokenizer),
        }
    }

    pub fn apply(&self, item: TextData) -> Item {
        let data;
        {
            let mut p_fn = self.preprocessing_fn.lock().unwrap();
            data = p_fn(item);
        }
        let label = if self.label_fn.is_some() {
            Some((self.label_fn.as_ref().unwrap())(&data))
        } else {
            None
        };
        let tokenization = self.tokenizer.tokenize(&data.processed);
        Item {
            data,
            label,
            tokenization,
        }
    }
}

fn read_yaml(path: &Path) -> String {
    read_to_string(path)
        .expect(&format!("could not read yaml file at {:?}", path))
}

fn parse_yaml<'a, T: Deserialize<'a>>(yaml: &'a str) -> T {
    serde_yaml::from_str(yaml)
        .expect(&format!("could not deserialize from yaml string\n{}", yaml))
}

pub fn pipeline_from_yaml(path: &Path) -> Pipeline {
    pipeline_from_str(&read_yaml(path))
}

pub fn pipeline_from_str(s: &str) -> Pipeline {
    let cfg: PipelineConfig = parse_yaml(s);
    Pipeline::new(cfg)
}

pub fn preprocessing_from_yaml(path: &Path) -> PreprocessingFn {
    preprocessing_from_str(&read_yaml(path))
}

pub fn preprocessing_from_str(s: &str) -> PreprocessingFn {
    let fns: Vec<PreprocessingConfig> = serde_yaml::from_str(s)
        .expect(&format!("could not deserialize from yaml string\n{}", s));
    preprocessing(fns)
}

pub fn labeling_from_yaml(path: &Path) -> LabelingFn {
    labeling_from_str(&read_yaml(path))
}

pub fn labeling_from_str(s: &str) -> LabelingFn {
    let cfg: LabelingConfig = serde_yaml::from_str(s)
        .expect(&format!("could not deserialize from yaml string\n{}", s));
    labeling(cfg)
}
