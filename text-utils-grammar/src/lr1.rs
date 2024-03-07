use std::{
    collections::{BTreeMap, HashMap},
    error::Error,
    fs::File,
    io::read_to_string,
    path::Path,
};

use cfgrammar::{
    yacc::{YaccGrammar, YaccGrammarError, YaccKind, YaccOriginalActionKind},
    NewlineCache, Spanned, TIdx,
};
use indexmap::IndexMap;
use itertools::Itertools;
use lrlex::{DefaultLexeme, DefaultLexerTypes, LRNonStreamingLexer};
use lrpar::{Lexeme, Node, RTParserBuilder};
use lrtable::{Action, Minimiser, StIdx, StateTable};
use regex::{escape, Regex};
use regex_automata::util::primitives::StateID;

use crate::{
    utils::{optimized_prefix_order, PrefixDFA, PrefixMatch},
    Constraint,
};

#[derive(Debug)]
enum Part {
    Literal(String),
    Regex(String),
}

fn extract_parts(pattern: &str) -> Vec<Part> {
    let mut parts = vec![];
    for part in pattern.split_whitespace() {
        if (part.starts_with('\'') && part.ends_with('\''))
            || (part.starts_with('"') && part.ends_with('"'))
        {
            // treat part as literal
            parts.push(Part::Literal(escape(&part[1..part.len() - 1])));
        } else {
            // treat part as regular expression
            parts.push(Part::Regex(part.to_string()));
        }
    }
    parts
}

// define function to recursively build pattern from parts
fn pattern_from_parts(
    name: &str,
    parts: &[Part],
    name_regex: &Regex,
    fragments: &HashMap<&str, Vec<Part>>,
    tokens: &IndexMap<&str, Vec<Part>>,
) -> Result<String, Box<dyn Error>> {
    let mut pattern = String::new();
    for part in parts {
        match part {
            Part::Literal(s) => pattern.push_str(s),
            Part::Regex(s) => {
                // find all tokens or framents in regex
                // and replace them with their pattern
                let mut replaced = String::new();
                let mut last_match = 0;
                for caps in name_regex.captures_iter(s) {
                    let m = caps.get(0).unwrap();
                    replaced.push_str(&s[last_match..m.start()]);
                    // surround token or fragment with parentheses to group it
                    replaced.push_str("(?:");
                    let _name = caps.get(1).unwrap().as_str();
                    if let Some(parts) = tokens.get(_name).or_else(|| fragments.get(_name)) {
                        let replacement =
                            pattern_from_parts(name, parts, name_regex, fragments, tokens)?;
                        replaced.push_str(&replacement);
                    } else {
                        return Err(format!(
                            "token or fragment {_name} within {name} not found in lexer"
                        )
                        .into());
                    }
                    replaced.push(')');
                    last_match = m.end();
                }
                replaced.push_str(&s[last_match..]);
                pattern.push_str(&replaced);
            }
        }
    }
    Ok(pattern)
}

type PdfaList = Vec<(PrefixDFA, Option<TIdx<u32>>)>;

fn format_yacc_error(grammar: &str, e: &YaccGrammarError) -> String {
    format!(
        "{} at {}",
        e,
        e.spans()
            .iter()
            .map(|s| if s.is_empty() {
                let start = s.start().saturating_sub(20);
                let end = grammar.len().min(s.end() + 20);
                let context = &grammar.as_bytes()[start..end];
                format!("middle of '{}'", String::from_utf8_lossy(context))
            } else {
                format!("'{}'", &grammar[s.start()..s.end()])
            })
            .join(" and ")
    )
}

fn load_grammar_and_pdfas(
    grammar: &str,
    grammar_kind: YaccKind,
    lexer: &str,
) -> Result<(YaccGrammar, PdfaList), Box<dyn Error>> {
    let grammar = YaccGrammar::new(grammar_kind, grammar).map_err(|e| {
        format!(
            "errors creating grammar:\n{}",
            e.iter().map(|e| format_yacc_error(grammar, e)).join("\n")
        )
    })?;

    // get token patterns and corresponding pdfas
    let token_name = Regex::new(r"\{([A-Z][A-Z0-9_]*)\}")?;
    let fragment_token_regex = Regex::new(r"(?m)^([A-Z][A-Z0-9_]*|;)\s+(.+)$")?;
    let sep = Regex::new("(?m)^%%$")?;
    let m = sep.find(lexer).ok_or("line with %% not found")?;

    // parse fragements
    let mut fragments = HashMap::new();
    for line in lexer[..m.start()].lines() {
        if line.is_empty() || line.trim_start().starts_with("//") {
            continue;
        }
        let cap = fragment_token_regex
            .captures(line)
            .ok_or(format!("invalid fragment line: {line}"))?;
        let name = cap.get(1).unwrap().as_str();
        if name == ";" {
            return Err("fragments cannot be named ;, which is reserved for ignore tokens".into());
        }
        let pattern = cap.get(2).unwrap().as_str();
        let parts = extract_parts(pattern);
        if fragments.insert(name, parts).is_some() {
            return Err(format!("duplicate fragment {name}").into());
        };
    }

    // parse tokens / terminals
    // use index map to preserve order
    let mut tokens = IndexMap::new();
    let mut ignore_tokens = vec![];
    for line in lexer[m.end()..].lines() {
        if line.is_empty() || line.trim_start().starts_with("//") {
            continue;
        }
        let cap = fragment_token_regex
            .captures(line)
            .ok_or(format!("invalid token line: {line}"))?;
        let name = cap.get(1).unwrap().as_str();
        let pattern = cap.get(2).unwrap().as_str();
        let parts = extract_parts(pattern);
        if parts.is_empty() {
            return Err(format!("invalid token pattern {pattern} for {name}").into());
        }
        if name == ";" {
            ignore_tokens.push(parts);
            continue;
        }
        if !ignore_tokens.is_empty() {
            return Err("ignore tokens must be at the end of the lexer file".into());
        }
        if grammar.token_idx(name).is_none() {
            eprintln!("token {name} not used in grammar, skipping...");
        };
        if tokens.insert(name, parts).is_some() {
            return Err(format!("duplicate token {name}").into());
        };
    }

    // build pdfas from fragments and tokens
    let mut pdfas = vec![];
    for (name, parts) in tokens.iter() {
        let pattern = pattern_from_parts(name, parts, &token_name, &fragments, &tokens)?;
        let pdfa = PrefixDFA::new(&pattern)?;
        if pdfa.is_match_state(pdfa.get_start_state()) {
            return Err(format!("token pattern {pattern} for {name} matches empty string").into());
        };
        pdfas.push((pdfa, grammar.token_idx(name)));
    }

    // add all unseen tokens from grammar as literal tokens to lexer
    for token in grammar
        .iter_tidxs()
        .filter_map(|tidx| grammar.token_name(tidx))
        .filter(|name| !fragments.contains_key(name) && !tokens.contains_key(name))
    {
        let tidx = grammar
            .token_idx(token)
            .ok_or(format!("token {token} not found in grammar"))?;
        let pdfa = PrefixDFA::new(&escape(token))?;
        pdfas.push((pdfa, Some(tidx)));
    }

    // add ignore pdfas at the end
    for parts in &ignore_tokens {
        let pattern = pattern_from_parts("ignore token", parts, &token_name, &fragments, &tokens)?;
        let pdfa = PrefixDFA::new(&pattern)?;
        if pdfa.is_match_state(pdfa.get_start_state()) {
            return Err(
                format!("token pattern {pattern} for ignore token matches empty string").into(),
            );
        };
        pdfas.push((pdfa, None));
    }

    Ok((grammar, pdfas))
}

type Tokens = Vec<Option<TIdx<u32>>>;
type Span = (usize, usize);
type Spans = Vec<Span>;
type Matching = Vec<(usize, StateID)>;

enum TokenOrMatching {
    Token(usize, Option<TIdx<u32>>),
    Matching(Matching),
}

fn find_token_or_matching(
    prefix: &[u8],
    matching: &Matching,
    pdfas: &[(PrefixDFA, Option<TIdx<u32>>)],
) -> Option<TokenOrMatching> {
    let mut len = 0;
    let mut token = None;
    let mut found_token = false;
    let mut prefix_matches = vec![];
    for &(pidx, state) in matching {
        let (pdfa, tidx) = &pdfas[pidx];
        match pdfa.find_prefix_match(state, prefix) {
            PrefixMatch::None => continue,
            PrefixMatch::Maybe(state) => prefix_matches.push((pidx, state)),
            PrefixMatch::UpTo(end, _) => {
                if !found_token || end > len {
                    len = end;
                    token = tidx.as_ref().copied();
                    found_token = true;
                }
            }
        };
    }
    if !prefix_matches.is_empty() {
        if prefix_matches.iter().all(|(pidx, state)| {
            let (pdfa, _) = &pdfas[*pidx];
            pdfa.is_final_match_state(*state)
        }) {
            let (pidx, _) = prefix_matches[0];
            let (_, token) = &pdfas[pidx];
            Some(TokenOrMatching::Token(
                prefix.len(),
                token.as_ref().copied(),
            ))
        } else {
            Some(TokenOrMatching::Matching(prefix_matches))
        }
    } else if found_token {
        Some(TokenOrMatching::Token(len, token))
    } else {
        None
    }
}

type PrefixLexerOutput = (Tokens, Spans, Matching, Span);

#[inline]
fn prefix_lexer_with(
    continuation: &[u8],
    pdfas: &[(PrefixDFA, Option<TIdx<u32>>)],
    mut prefix_matches: Matching,
) -> Result<PrefixLexerOutput, Box<dyn Error>> {
    // returns a list of tokens and a list of indices of pdfas matching
    // the rest of the prefix, or None if no matching pdfa is found
    let mut tokens = vec![];
    let mut spans = vec![];
    let mut i = 0;
    // logic is that longest match wins
    while i < continuation.len() {
        match find_token_or_matching(&continuation[i..], &prefix_matches, pdfas) {
            Some(TokenOrMatching::Token(len, token)) => {
                tokens.push(token);
                spans.push((i, len));
                i += len;
                prefix_matches = initial_prefix_matches(pdfas);
            }
            Some(TokenOrMatching::Matching(matching)) => {
                prefix_matches = matching;
                break;
            }
            None => {
                return Err(format!(
                    "no matching token found from position {i}: '{}'",
                    String::from_utf8_lossy(&continuation[i..])
                )
                .into());
            }
        }
    }
    Ok((tokens, spans, prefix_matches, (i, continuation.len() - i)))
}

fn initial_prefix_matches(pdfas: &[(PrefixDFA, Option<TIdx<u32>>)]) -> Matching {
    pdfas
        .iter()
        .enumerate()
        .map(|(pidx, (pdfa, _))| (pidx, pdfa.get_start_state()))
        .collect()
}

fn prefix_lexer(
    prefix: &[u8],
    pdfas: &[(PrefixDFA, Option<TIdx<u32>>)],
) -> Result<PrefixLexerOutput, Box<dyn Error>> {
    // initially all pdfas are in the potential prefix matches, the start state
    let prefix_matches = initial_prefix_matches(pdfas);
    prefix_lexer_with(prefix, pdfas, prefix_matches)
}

fn lexer(
    text: &str,
    pdfas: &[(PrefixDFA, Option<TIdx<u32>>)],
) -> Result<(Tokens, Spans), Box<dyn Error>> {
    let (mut tokens, mut spans, last_matches, last_span) = prefix_lexer(text.as_bytes(), pdfas)?;
    if let Some(&token) = last_matches.iter().find_map(|&(pidx, state)| {
        let (pdfa, token) = &pdfas[pidx];
        if pdfa.is_match_state(state) {
            Some(token)
        } else {
            None
        }
    }) {
        assert!(
            last_span.1 > 0,
            "last span should not be empty in this case"
        );
        tokens.push(token);
        spans.push(last_span);
    }
    Ok((tokens, spans))
}

pub struct LR1GrammarParser {
    grammar: YaccGrammar<u32>,
    table: StateTable<u32>,
    pdfas: Vec<(PrefixDFA, Option<TIdx<u32>>)>,
}

#[derive(Debug, PartialEq)]
pub enum LR1Parse<'a> {
    Empty(&'a str),
    Terminal(&'a str, Span),
    NonTerminal(&'a str, Vec<LR1Parse<'a>>, Span),
}

impl LR1Parse<'_> {
    pub fn is_empty(&self) -> bool {
        matches!(self, LR1Parse::Empty(..))
    }

    pub fn span(&self) -> Option<&Span> {
        match self {
            LR1Parse::Empty(..) => None,
            LR1Parse::Terminal(.., span) => Some(span),
            LR1Parse::NonTerminal(.., span) => Some(span),
        }
    }

    pub fn pretty(&self, text: &str, collapse: bool) -> String {
        fn pretty_parse(parse: &LR1Parse<'_>, indent: usize, text: &str, collapse: bool) -> String {
            match parse {
                LR1Parse::Empty(name) => format!("{:indent$}{name} ''", ""),
                LR1Parse::Terminal(name, (start, len)) => {
                    format!("{:indent$}{name} '{}'", "", &text[*start..*start + *len],)
                }
                LR1Parse::NonTerminal(name, children, ..) => {
                    assert!(!children.is_empty());
                    if children.len() == 1 && collapse {
                        return pretty_parse(&children[0], indent, text, collapse);
                    }
                    let mut s = format!("{:indent$}{name}", "");
                    for child in children {
                        s.push('\n');
                        s.push_str(&pretty_parse(child, indent + 2, text, collapse));
                    }
                    s
                }
            }
        }
        pretty_parse(self, 0, text, collapse)
    }
}

impl LR1GrammarParser {
    pub fn new(grammar: &str, tokens: &str) -> Result<Self, Box<dyn Error>> {
        let (grammar, pdfas) = load_grammar_and_pdfas(
            grammar,
            YaccKind::Original(YaccOriginalActionKind::GenericParseTree),
            tokens,
        )?;
        let (_, table) = lrtable::from_yacc(&grammar, Minimiser::Pager)?;
        Ok(Self {
            grammar,
            table,
            pdfas,
        })
    }

    pub fn from_files(
        grammar_path: impl AsRef<Path>,
        tokens_path: impl AsRef<Path>,
    ) -> Result<Self, Box<dyn Error>> {
        let file = File::open(grammar_path.as_ref())?;
        let grammar = read_to_string(file)?;
        let file = File::open(tokens_path.as_ref())?;
        let tokens = read_to_string(file)?;
        Self::new(&grammar, &tokens)
    }

    pub fn lex(&self, text: &str) -> Result<Vec<&str>, Box<dyn Error>> {
        let (tokens, _) = lexer(text, &self.pdfas)?;
        Ok(tokens
            .into_iter()
            .filter_map(|tidx| tidx.and_then(|tidx| self.grammar.token_name(tidx)))
            .collect())
    }

    pub fn parse(&self, text: &str, collapse: bool) -> Result<LR1Parse<'_>, Box<dyn Error>> {
        let (tokens, spans) = lexer(text, &self.pdfas)?;
        let mut nlc = NewlineCache::new();
        nlc.feed(text);
        let lexer = LRNonStreamingLexer::new(
            text,
            tokens
                .into_iter()
                .zip(spans)
                .filter_map(|(tidx, (start, len))| {
                    tidx.map(|tidx| Ok(DefaultLexeme::new(tidx.as_storaget(), start, len)))
                })
                .collect(),
            nlc,
        );
        let parser: RTParserBuilder<'_, u32, DefaultLexerTypes> =
            RTParserBuilder::new(&self.grammar, &self.table);
        let (tree, errors) = parser.parse_generictree(&lexer);
        if !errors.is_empty() {
            return Err(format!(
                "errors parsing input:\n{}",
                errors
                    .iter()
                    .map(|e| e.pp(&lexer, &|tidx| self.grammar.token_epp(tidx)))
                    .join("\n")
            )
            .into());
        }
        let Some(tree) = tree else {
            return Err("failed to parse input".into());
        };
        // convert tree to lr1 parse
        fn node_to_lr1<'a>(
            grammar: &'a YaccGrammar,
            node: &Node<DefaultLexeme<u32>, u32>,
            collapse: bool,
        ) -> LR1Parse<'a> {
            match node {
                Node::Term { lexeme } => {
                    let span = lexeme.span();
                    let tidx = lexeme.tok_id();
                    let tname = grammar.token_name(TIdx(tidx)).unwrap();
                    LR1Parse::Terminal(tname, (span.start(), span.len()))
                }
                Node::Nonterm { ridx, nodes } => {
                    let rname = grammar.rule_name_str(*ridx);
                    let nodes: Vec<_> = nodes
                        .iter()
                        .filter_map(|node| {
                            let node = node_to_lr1(grammar, node, collapse);
                            if node.is_empty() {
                                None
                            } else {
                                Some(node)
                            }
                        })
                        .collect();
                    if nodes.is_empty() {
                        return LR1Parse::Empty(rname);
                    } else if nodes.len() == 1 && collapse {
                        return nodes.into_iter().next().unwrap();
                    }
                    let first_span = nodes.first().unwrap().span().unwrap();
                    let last_span = nodes.last().unwrap().span().unwrap();
                    let span = (first_span.0, last_span.0 + last_span.1 - first_span.0);
                    LR1Parse::NonTerminal(rname, nodes, span)
                }
            }
        }
        Ok(node_to_lr1(&self.grammar, &tree, collapse))
    }
}

pub struct ExactLR1GrammarConstraint {
    pub(crate) grammar: YaccGrammar<u32>,
    table: StateTable<u32>,
    pdfas: Vec<(PrefixDFA, Option<TIdx<u32>>)>,
    continuations: Vec<Vec<u8>>,
    permutation: Vec<usize>,
    skips: Vec<usize>,
}

enum LR1Action {
    ShiftReduce(usize, StIdx<u32>),
    Accept,
    None,
}

fn shift_reduce(
    grammar: &YaccGrammar,
    table: &StateTable<u32>,
    stack: &[StIdx<u32>],
    token: TIdx<u32>,
) -> LR1Action {
    let Some(mut stidx) = stack.last().copied() else {
        return LR1Action::None;
    };
    // perform actions until the next shift,
    // can be implemented without actually
    // modifying the stack, because it will only ever
    // get smaller by reduces
    // stidx will always be the last element of the stack
    // (at position stack_end)
    let mut stack_end = stack.len() - 1;
    loop {
        match table.action(stidx, token) {
            Action::Shift(next_stidx) => {
                stidx = next_stidx;
                break;
            }
            Action::Reduce(pidx) => {
                let ridx = grammar.prod_to_rule(pidx);
                let rlen = grammar.prod(pidx).len();
                stack_end -= rlen - 1;
                let Some(new_stidx) = table.goto(stack[stack_end - 1], ridx) else {
                    return LR1Action::None;
                };
                stidx = new_stidx;
            }
            Action::Accept => return LR1Action::Accept,
            Action::Error => return LR1Action::None,
        };
    }
    LR1Action::ShiftReduce(stack_end + 1, stidx)
}

fn matchable_pdfas<'pdfa>(
    grammar: &YaccGrammar,
    table: &StateTable<u32>,
    pdfas: &'pdfa [(PrefixDFA, Option<TIdx<u32>>)],
    stack: &[StIdx<u32>],
) -> Vec<(usize, &'pdfa PrefixDFA)> {
    let Some(&last) = stack.last() else {
        return vec![];
    };
    let state_actions: Vec<_> = table.state_actions(last).collect();
    pdfas
        .iter()
        .enumerate()
        .filter_map(|(i, (pdfa, tidx))| {
            if let Some(tidx) = tidx {
                if !state_actions.contains(tidx)
                    || !matches!(
                        shift_reduce(grammar, table, stack, *tidx),
                        LR1Action::ShiftReduce(..)
                    )
                {
                    return None;
                }
            }
            Some((i, pdfa))
        })
        .collect()
}

fn filter_matching(
    matching: &mut Matching,
    grammar: &YaccGrammar,
    table: &StateTable<u32>,
    pdfas: &[(PrefixDFA, Option<TIdx<u32>>)],
    stack: &[StIdx<u32>],
) {
    matching.retain(|&(pidx, _)| {
        let (_, token) = &pdfas[pidx];
        if let Some(token) = token {
            if !matches!(
                shift_reduce(grammar, table, stack, *token),
                LR1Action::ShiftReduce(..)
            ) {
                return false;
            }
        }
        true
    })
}

fn drive(
    grammar: &YaccGrammar,
    table: &StateTable<u32>,
    mut stack: Vec<StIdx<u32>>,
    tokens: &[Option<TIdx<u32>>],
) -> Option<Vec<StIdx<u32>>> {
    let mut idx = 0;
    while idx < tokens.len() {
        let stidx = stack.last()?;
        let Some(tidx) = tokens[idx] else {
            idx += 1;
            continue;
        };
        match table.action(*stidx, tidx) {
            Action::Shift(stidx) => {
                stack.push(stidx);
                idx += 1;
            }
            Action::Reduce(pidx) => {
                let ridx = grammar.prod_to_rule(pidx);
                let keep = stack.len() - grammar.prod(pidx).len();
                stack.truncate(keep);
                let stidx = table.goto(*stack.last()?, ridx)?;
                stack.push(stidx);
            }
            Action::Accept => unreachable!("dont drive with eof token"),
            Action::Error => return None,
        }
    }
    Some(stack)
}

fn only_skippable_matching(matching: &Matching, pdfas: &[(PrefixDFA, Option<TIdx<u32>>)]) -> bool {
    matching.iter().all(|&(pidx, pdfa_state)| {
        let (pdfa, None) = &pdfas[pidx] else {
            return false;
        };
        pdfa.is_match_state(pdfa_state)
    })
}

fn is_accept_state(grammar: &YaccGrammar, table: &StateTable<u32>, stack: &[StIdx<u32>]) -> bool {
    matches!(
        shift_reduce(grammar, table, stack, grammar.eof_token_idx()),
        LR1Action::Accept
    )
}

fn is_match_state(
    grammar: &YaccGrammar,
    table: &StateTable<u32>,
    pdfas: &[(PrefixDFA, Option<TIdx<u32>>)],
    state: &LR1State,
) -> bool {
    is_accept_state(grammar, table, &state.stack)
        || state.matching.iter().any(|&(pidx, pdfa_state)| {
            let (pdfa, Some(token)) = &pdfas[pidx] else {
                return false;
            };
            if !pdfa.is_match_state(pdfa_state) {
                return false;
            }
            let LR1Action::ShiftReduce(keep, stidx) =
                shift_reduce(grammar, table, &state.stack, *token)
            else {
                return false;
            };
            let mut stack = state.stack[..keep].to_vec();
            stack.push(stidx);
            is_accept_state(grammar, table, &stack)
        })
}

impl ExactLR1GrammarConstraint {
    pub fn new(
        grammar: &str,
        tokens: &str,
        continuations: Vec<Vec<u8>>,
    ) -> Result<Self, Box<dyn Error>> {
        let (grammar, pdfas) = load_grammar_and_pdfas(
            grammar,
            YaccKind::Original(YaccOriginalActionKind::NoAction),
            tokens,
        )?;
        let (_, table) = lrtable::from_yacc(&grammar, Minimiser::Pager)?;
        let (permutation, skips) = optimized_prefix_order(&continuations);
        Ok(Self {
            continuations,
            grammar,
            pdfas,
            table,
            permutation,
            skips,
        })
    }

    pub fn from_files(
        grammar_path: impl AsRef<Path>,
        tokens_path: impl AsRef<Path>,
        continuations: Vec<Vec<u8>>,
    ) -> Result<Self, Box<dyn Error>> {
        let file = File::open(grammar_path.as_ref())?;
        let grammar = read_to_string(file)?;
        let file = File::open(tokens_path.as_ref())?;
        let tokens = read_to_string(file)?;
        Self::new(&grammar, &tokens, continuations)
    }

    pub fn only_skippable_matching(&self, state: &LR1State) -> bool {
        only_skippable_matching(&state.matching, &self.pdfas)
    }
}

#[derive(Clone, Default)]
pub struct LR1State {
    stack: Vec<StIdx<u32>>,
    matching: Matching,
}

impl LR1State {
    #[allow(dead_code)]
    pub fn next(&mut self, state: LR1NextState) {
        if let Some((keep, stidx, ..)) = state.action {
            self.stack.truncate(keep);
            self.stack.push(stidx);
        }
        self.matching = state.matching;
    }
}

#[derive(Clone, Default)]
pub struct LR1NextState {
    action: Option<(usize, StIdx<u32>, String)>,
    matching: Matching,
}

impl Constraint for ExactLR1GrammarConstraint {
    type State = LR1State;
    type NextState = LR1NextState;

    fn get_state(&self, prefix: &[u8]) -> Option<Self::State> {
        let (tokens, _, mut matching, _) = prefix_lexer(prefix, &self.pdfas).ok()?;
        let stack = drive(
            &self.grammar,
            &self.table,
            vec![self.table.start_state()],
            &tokens,
        )?;
        // the matching returned by prefix lexer is not a matching
        // that adheres to the grammar, so we need to filter it
        // further to only contain pdfas that are allowed to match
        // according to the grammar
        filter_matching(
            &mut matching,
            &self.grammar,
            &self.table,
            &self.pdfas,
            &stack,
        );
        if matching.is_empty() {
            return None;
        }
        Some(Self::State { stack, matching })
    }

    fn get_start_state(&self) -> Self::State {
        self.get_state(b"").expect("should not happen")
    }

    fn is_match_state(&self, state: &Self::State) -> bool {
        is_match_state(&self.grammar, &self.table, &self.pdfas, state)
    }

    fn get_valid_continuations_with_state(
        &self,
        state: &Self::State,
    ) -> (Vec<usize>, Vec<Self::NextState>) {
        assert!(!state.matching.is_empty());
        let mut conts = BTreeMap::new();

        // in case no pdfa is still matching for a continuation
        // we do the following:
        // 1. find all unskippable pdfas that are currently in matching state
        //    --> if there are none, skip
        // 2. select the one with the lowest index, as that would
        //    be the one picked by the lexer (length of match is the same
        //    for all pdfas)
        // 3. step with the corresponding token and return the action
        //    --> the action will later be used to create the next state
        let next = if let Some((LR1Action::ShiftReduce(keep, next_stidx), tidx)) = state
            .matching
            .iter()
            .find_map(|&(pidx, pdfa_state)| {
                let (pdfa, Some(tidx)) = &self.pdfas[pidx] else {
                    return None;
                };
                if pdfa.is_match_state(pdfa_state) {
                    Some(tidx)
                } else {
                    None
                }
            })
            .map(|&tidx| {
                (
                    shift_reduce(&self.grammar, &self.table, &state.stack, tidx),
                    tidx,
                )
            }) {
            let mut next_stack = state.stack[..keep].to_vec();
            next_stack.push(next_stidx);
            let next_matchable_pdfas =
                matchable_pdfas(&self.grammar, &self.table, &self.pdfas, &next_stack);
            let token_name = self.grammar.token_name(tidx).unwrap();
            Some((
                (keep, next_stidx, token_name.to_string()),
                next_matchable_pdfas,
            ))
        } else {
            None
        };

        let only_skippable_matching = only_skippable_matching(&state.matching, &self.pdfas);
        let matchable_pdfas =
            matchable_pdfas(&self.grammar, &self.table, &self.pdfas, &state.stack);

        // now check all continuations
        let mut i = 0;
        while i < self.permutation.len() {
            let skip = self.skips[i];
            let j = self.permutation[i];
            let cont = &self.continuations[j];
            i += 1;

            // get all pdfas that are still matching
            let mut still_matching: Vec<_> = vec![];
            for &(pidx, pdfa_state) in &state.matching {
                let (pdfa, _) = &self.pdfas[pidx];
                if let Some(state) = pdfa.drive(pdfa_state, cont) {
                    still_matching.push((pidx, state));
                }
            }

            // if we have some pdfas that are still matching, use
            // them in the next state; this corresponds the
            // longest matching rule in the lexer
            if !still_matching.is_empty() {
                conts.insert(
                    j,
                    LR1NextState {
                        action: None,
                        matching: still_matching,
                    },
                );
            } else if only_skippable_matching {
                // if no pdfas are still matching, check the ones who
                // are matchable in the current state and stay in that state
                let matching: Vec<_> = matchable_pdfas
                    .iter()
                    .filter_map(|&(i, pdfa)| {
                        pdfa.drive(pdfa.get_start_state(), cont)
                            .map(|state| (i, state))
                    })
                    .collect();

                if matching.is_empty() {
                    i += skip;
                    continue;
                }

                conts.insert(
                    j,
                    LR1NextState {
                        action: None,
                        matching,
                    },
                );
            } else if let Some((next_action, next_matchable_pdfas)) = &next {
                // if there are no pdfas still matching and no skippable pdfas matching,
                // check the matchable pdfas for the next state
                let next_matching: Vec<_> = next_matchable_pdfas
                    .iter()
                    .filter_map(|&(i, pdfa)| {
                        pdfa.drive(pdfa.get_start_state(), cont)
                            .map(|state| (i, state))
                    })
                    .collect();

                if next_matching.is_empty() {
                    i += skip;
                    continue;
                }

                conts.insert(
                    j,
                    LR1NextState {
                        action: Some(next_action.clone()),
                        matching: next_matching,
                    },
                );
            }
        }
        conts.into_iter().unzip()
    }
}

pub struct LR1GrammarConstraint {
    grammar: YaccGrammar<u32>,
    table: StateTable<u32>,
    pdfas: Vec<(PrefixDFA, Option<TIdx<u32>>)>,
    continuations: Vec<Vec<u8>>,
    permutation: Vec<usize>,
    skips: Vec<usize>,
}

impl LR1GrammarConstraint {
    pub fn new(
        grammar: &str,
        tokens: &str,
        continuations: Vec<Vec<u8>>,
    ) -> Result<Self, Box<dyn Error>> {
        let (grammar, pdfas) = load_grammar_and_pdfas(
            grammar,
            YaccKind::Original(YaccOriginalActionKind::NoAction),
            tokens,
        )?;
        let (_, table) = lrtable::from_yacc(&grammar, Minimiser::Pager)?;
        let (permutation, skips) = optimized_prefix_order(&continuations);
        Ok(Self {
            continuations,
            grammar,
            pdfas,
            table,
            permutation,
            skips,
        })
    }

    pub fn from_files(
        grammar_path: impl AsRef<Path>,
        tokens_path: impl AsRef<Path>,
        continuations: Vec<Vec<u8>>,
    ) -> Result<Self, Box<dyn Error>> {
        let file = File::open(grammar_path.as_ref())?;
        let grammar = read_to_string(file)?;
        let file = File::open(tokens_path.as_ref())?;
        let tokens = read_to_string(file)?;
        Self::new(&grammar, &tokens, continuations)
    }

    pub fn only_skippable_matching(&self, state: &LR1State) -> bool {
        only_skippable_matching(&state.matching, &self.pdfas)
    }
}

impl Constraint for LR1GrammarConstraint {
    type State = LR1State;
    type NextState = LR1State;

    fn get_state(&self, prefix: &[u8]) -> Option<Self::State> {
        let (tokens, _, mut matching, _) = prefix_lexer(prefix, &self.pdfas).ok()?;
        let stack = drive(
            &self.grammar,
            &self.table,
            vec![self.table.start_state()],
            &tokens,
        )?;
        filter_matching(
            &mut matching,
            &self.grammar,
            &self.table,
            &self.pdfas,
            &stack,
        );
        if matching.is_empty() {
            return None;
        }
        Some(Self::State { stack, matching })
    }

    fn get_start_state(&self) -> Self::State {
        self.get_state(b"").expect("should not happen")
    }

    fn is_match_state(&self, state: &Self::State) -> bool {
        is_match_state(&self.grammar, &self.table, &self.pdfas, state)
    }

    fn get_valid_continuations_with_state(
        &self,
        state: &Self::State,
    ) -> (Vec<usize>, Vec<Self::NextState>) {
        let mut conts = BTreeMap::new();

        // now check all continuations
        let mut i = 0;
        while i < self.permutation.len() {
            let skip = self.skips[i];
            let j = self.permutation[i];
            let cont = &self.continuations[j];
            i += 1;

            let Ok((tokens, _, mut next_matching, _)) =
                prefix_lexer_with(cont, &self.pdfas, state.matching.clone())
            else {
                i += skip;
                continue;
            };

            let Some(next_stack) = drive(&self.grammar, &self.table, state.stack.clone(), &tokens)
            else {
                i += skip;
                continue;
            };

            filter_matching(
                &mut next_matching,
                &self.grammar,
                &self.table,
                &self.pdfas,
                &next_stack,
            );
            if next_matching.is_empty() {
                i += skip;
                continue;
            }

            conts.insert(
                j,
                LR1State {
                    stack: next_stack,
                    matching: next_matching,
                },
            );
        }

        conts.into_iter().unzip()
    }
}

#[cfg(test)]
mod test {
    use itertools::Itertools;

    use super::*;
    use std::{collections::HashMap, fs, path::PathBuf};

    fn load_continuations() -> Vec<Vec<u8>> {
        let dir = env!("CARGO_MANIFEST_DIR");
        let continuations_json =
            fs::read(PathBuf::from(dir).join("resources/test/continuations.json"))
                .expect("failed to read file");

        // use serde to deserialize continuations array from json
        serde_json::from_slice::<Vec<String>>(&continuations_json)
            .unwrap()
            .into_iter()
            .map(|c| c.as_bytes().to_vec())
            .collect()
    }

    fn get_calc_pfdas() -> (
        Vec<(PrefixDFA, Option<TIdx<u32>>)>,
        HashMap<TIdx<u32>, &'static str>,
    ) {
        // this simulates the pdfas we would get from a calc.l file
        (
            vec![
                (PrefixDFA::new("\\(").unwrap(), Some(TIdx(0))),
                (PrefixDFA::new("\\)").unwrap(), Some(TIdx(1))),
                (PrefixDFA::new("\\+").unwrap(), Some(TIdx(2))),
                (PrefixDFA::new("\\*").unwrap(), Some(TIdx(3))),
                (PrefixDFA::new("[0-9]+").unwrap(), Some(TIdx(4))),
                (PrefixDFA::new("[ ]+").unwrap(), None),
                (PrefixDFA::new("[\n\t]+").unwrap(), None),
            ],
            HashMap::from([
                (TIdx(0), "LP"),
                (TIdx(1), "RP"),
                (TIdx(2), "PLUS"),
                (TIdx(3), "TIMES"),
                (TIdx(4), "INT"),
            ]),
        )
    }

    fn get_ab_pdfas() -> (
        Vec<(PrefixDFA, Option<TIdx<u32>>)>,
        HashMap<TIdx<u32>, &'static str>,
    ) {
        (
            vec![
                (PrefixDFA::new("a").unwrap(), Some(TIdx(0))),
                (PrefixDFA::new("aa").unwrap(), Some(TIdx(1))),
                (PrefixDFA::new("ab").unwrap(), Some(TIdx(2))),
                (PrefixDFA::new("b").unwrap(), Some(TIdx(3))),
                (PrefixDFA::new("bb").unwrap(), Some(TIdx(4))),
                (PrefixDFA::new("ab").unwrap(), Some(TIdx(5))),
                (PrefixDFA::new("[ ]+").unwrap(), None),
                (PrefixDFA::new("[\n\t]+").unwrap(), None),
            ],
            HashMap::from([
                (TIdx(0), "A"),
                (TIdx(1), "AA"),
                (TIdx(2), "AB1"),
                (TIdx(3), "B"),
                (TIdx(4), "BB"),
                (TIdx(5), "AB2"),
            ]),
        )
    }

    #[test]
    fn test_lexer() {
        let (pdfas, map) = get_calc_pfdas();
        assert!(lexer("2 - 1", &pdfas).is_err());
        let (tokens, spans) = lexer("(1 + 28)*\n3", &pdfas).unwrap();
        assert_eq!(
            tokens
                .into_iter()
                .filter_map(|tidx| tidx.map(|tidx| map[&tidx]))
                .collect_vec(),
            vec!["LP", "INT", "PLUS", "INT", "RP", "TIMES", "INT"]
        );
        assert_eq!(
            spans,
            vec![
                (0, 1),
                (1, 1),
                (2, 1),
                (3, 1),
                (4, 1),
                (5, 2),
                (7, 1),
                (8, 1),
                (9, 1),
                (10, 1)
            ]
        );
        let (pdfas, map) = get_ab_pdfas();
        let (tokens, spans) = lexer("aabb", &pdfas).unwrap();
        assert_eq!(
            tokens
                .into_iter()
                .filter_map(|tidx| tidx.map(|tidx| map[&tidx]))
                .collect_vec(),
            vec!["AA", "BB"]
        );
        assert_eq!(spans, vec![(0, 2), (2, 2)]);
        let (tokens, spans) = lexer("abb", &pdfas).unwrap();
        assert_eq!(
            tokens
                .into_iter()
                .filter_map(|tidx| tidx.map(|tidx| map[&tidx]))
                .collect_vec(),
            vec!["AB1", "B"]
        );
        assert_eq!(spans, vec![(0, 2), (2, 1)]);
        assert!(lexer("abac", &pdfas).is_err());
    }

    fn combine_prefix_lexer_outputs(
        output1: PrefixLexerOutput,
        output2: PrefixLexerOutput,
    ) -> PrefixLexerOutput {
        let (mut combined_lexemes, mut combined_spans, _, mut last_span) = output1;
        let (lexemes2, spans2, matching, last_span2) = output2;
        combined_lexemes.extend(lexemes2);
        if let Some(first2) = spans2.first() {
            combined_spans.push((last_span.0, last_span.1 + first2.1));
            combined_spans.extend(
                spans2
                    .into_iter()
                    .skip(1)
                    .map(|(start, len)| (last_span.0 + last_span.1 + start, len)),
            );
            last_span = (last_span.0 + last_span.1 + last_span2.0, last_span2.1);
        } else {
            assert!(last_span2.0 == 0);
            last_span = (last_span.0, last_span.1 + last_span2.1);
        }
        (combined_lexemes, combined_spans, matching, last_span)
    }

    #[test]
    fn test_prefix_lexer_with() {
        let (pdfas, _) = get_calc_pfdas();

        let texts = [
            "(1 + 28)*\n3".as_bytes(),
            b"  10 +   5",
            b" ",
            b"(((3 + 4)) * 6)",
        ];

        for text in texts {
            let (lexemes, spans, matching, last_span) = prefix_lexer(text, &pdfas).unwrap();

            for i in 0..=text.len() {
                let output1 = prefix_lexer(&text[..i], &pdfas).unwrap();
                let output2 = prefix_lexer_with(&text[i..], &pdfas, output1.2.clone()).unwrap();
                let (combined_lexemes, combined_spans, combined_matching, combined_last_span) =
                    combine_prefix_lexer_outputs(output1, output2);
                println!("text: {:?}", String::from_utf8_lossy(text));
                println!("text1: {:?}", String::from_utf8_lossy(&text[..i]));
                println!("text2: {:?}", String::from_utf8_lossy(&text[i..]));
                assert_eq!(lexemes, combined_lexemes);
                assert_eq!(matching, combined_matching);
                assert_eq!(spans, combined_spans);
                assert_eq!(last_span, combined_last_span);
            }
        }
    }

    #[test]
    fn test_prefix_lexer() {
        let (pdfas, map) = get_calc_pfdas();
        let (lexemes, spans, matching, last_span) = prefix_lexer(b"(1 + 28)*\n3", &pdfas).unwrap();
        assert_eq!(
            lexemes
                .iter()
                .cloned()
                .filter_map(|tidx| tidx.map(|tidx| map[&tidx]))
                .collect_vec(),
            vec!["LP", "INT", "PLUS", "INT", "RP", "TIMES"]
        );
        assert_eq!(
            spans,
            vec![
                (0, 1),
                (1, 1),
                (2, 1),
                (3, 1),
                (4, 1),
                (5, 2),
                (7, 1),
                (8, 1),
                (9, 1)
            ]
        );
        assert_eq!(matching.len(), 1);
        assert_eq!(last_span, (10, 1));
        let (idx, state) = matching[0];
        assert_eq!(idx, 4);
        let (pdfa, tidx) = &pdfas[idx];
        assert_eq!(map[tidx.as_ref().unwrap()], "INT");
        assert!(pdfa.is_match_state(state));

        let (lexemes, spans, matching, last_span) = prefix_lexer(b"", &pdfas).unwrap();
        assert!(lexemes.is_empty());
        assert!(spans.is_empty());
        assert_eq!(
            matching,
            pdfas
                .iter()
                .enumerate()
                .map(|(i, (pdfa, _))| (i, pdfa.get_start_state()))
                .collect_vec()
        );
        assert_eq!(last_span, (0, 0));

        let (lexemes, spans, matching, last_span) = prefix_lexer(b"    (", &pdfas).unwrap();
        assert_eq!(lexemes.into_iter().filter(|tidx| tidx.is_some()).count(), 1);
        assert_eq!(spans.len(), 2);
        assert_eq!(matching.len(), 7);
        assert_eq!(last_span, (5, 0));

        let (pdfas, map) = get_ab_pdfas();
        let (lexemes, spans, matching, last_span) = prefix_lexer(b"aabb", &pdfas).unwrap();
        assert_eq!(
            lexemes
                .into_iter()
                .filter_map(|tidx| tidx.map(|tidx| map[&tidx]))
                .collect_vec(),
            vec!["AA", "BB"]
        );
        assert_eq!(spans, vec![(0, 2), (2, 2)]);
        assert_eq!(matching.len(), 8);
        assert_eq!(last_span, (4, 0));

        let (lexemes, spans, matching, last_span) = prefix_lexer(b"aab", &pdfas).unwrap();
        assert_eq!(
            lexemes
                .into_iter()
                .filter_map(|tidx| tidx.map(|tidx| map[&tidx]))
                .collect_vec(),
            vec!["AA"]
        );
        assert_eq!(spans, vec![(0, 2)]);
        assert_eq!(matching.len(), 2);
        let (idx, state) = matching[0];
        assert_eq!(idx, 3);
        let (pdfa, tidx) = &pdfas[idx];
        assert_eq!(map[tidx.as_ref().unwrap()], "B");
        assert!(pdfa.is_match_state(state));
        assert_eq!(last_span, (2, 1));
        let (idx, state) = matching[1];
        assert_eq!(idx, 4);
        let (pdfa, tidx) = &pdfas[idx];
        assert_eq!(map[tidx.as_ref().unwrap()], "BB");
        assert!(!pdfa.is_match_state(state));
    }

    fn load_lrk_grammar(name: &str) -> (PathBuf, PathBuf, Vec<PathBuf>) {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("grammars")
            .join(name);
        let grammar = dir.as_path().join(format!("{name}.y"));
        let lexer = dir.as_path().join(format!("{name}.l"));
        // load all examples from grammars/<name>/examples/
        let examples = fs::read_dir(dir.as_path().join("examples"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect();
        (grammar, lexer, examples)
    }

    #[test]
    fn test_lrk_parser() {
        let (grammar, lexer, _) = load_lrk_grammar("calc");
        let lrk = LR1GrammarParser::from_files(grammar, lexer).unwrap();
        assert!(lrk.parse("2 - 1", false).is_err());
        let text = "(1 + 28)*\n3";
        let parse = lrk.parse(text, false).unwrap();
        println!("{}", parse.pretty(text, true));
    }

    fn drive_with_tokens(
        grammar: &YaccGrammar,
        table: &StateTable<u32>,
        tokens: &[Option<TIdx<u32>>],
    ) -> bool {
        drive(grammar, table, vec![table.start_state()], tokens).is_some()
    }

    fn check_continuations(
        lrk: &LR1GrammarConstraint,
        prefix: &[u8],
        continuations: &[Vec<u8>],
    ) -> LR1State {
        let state = lrk.get_state(prefix).unwrap();
        let (cont_indices, _) = lrk.get_valid_continuations_with_state(&state);
        println!(
            "matching {}, {} conts: {:#?}",
            lrk.is_match_state(&state),
            cont_indices.len(),
            cont_indices
                .iter()
                .map(|i| String::from_utf8_lossy(&continuations[*i]))
                .collect_vec()
        );
        for i in cont_indices {
            let full: Vec<_> = prefix
                .iter()
                .copied()
                .chain(continuations[i].clone())
                .collect();
            let (tokens, ..) = prefix_lexer(&full, &lrk.pdfas).unwrap();
            assert!(drive_with_tokens(&lrk.grammar, &lrk.table, &tokens));
        }
        state
    }

    #[test]
    fn test_lrk_constraint() {
        let conts = load_continuations();

        let (grammar, lexer, _) = load_lrk_grammar("json");
        let lrk = LR1GrammarConstraint::from_files(grammar, lexer, conts.clone()).unwrap();
        assert!(lrk.get_state(b"\"id\": \"1\"").is_none());
        assert!(lrk.get_state(b"{\"id\": \"1\"}}").is_none());
        assert!(lrk.get_state(b"\"id\"").is_some());
        let state = check_continuations(&lrk, b"{\"id\": \"1\"", &conts);
        assert!(!lrk.is_match_state(&state));
        let state = check_continuations(&lrk, b"{\"id\": \"1\"}", &conts);
        assert!(lrk.is_match_state(&state));

        let (grammar, lexer, _) = load_lrk_grammar("calc");
        let lrk = ExactLR1GrammarConstraint::from_files(grammar, lexer, conts.clone()).unwrap();
        let state = lrk.get_start_state();
        let (cont_indices, _) = lrk.get_valid_continuations_with_state(&state);
        println!(
            "matching {}, {} conts: {:#?}",
            lrk.is_match_state(&state),
            cont_indices.len(),
            cont_indices
                .iter()
                .map(|i| String::from_utf8_lossy(&conts[*i]))
                .collect_vec()
        );
        let state = lrk.get_state(b"1").unwrap();
        let (cont_indices, _) = lrk.get_valid_continuations_with_state(&state);
        println!(
            "matching {}, {} conts: {:#?}",
            lrk.is_match_state(&state),
            cont_indices.len(),
            cont_indices
                .iter()
                .take(10)
                .map(|i| String::from_utf8_lossy(&conts[*i]))
                .collect_vec()
        );

        let (grammar, lexer, _) = load_lrk_grammar("test");
        let lrk = LR1GrammarConstraint::from_files(grammar, lexer, conts.clone()).unwrap();
        let state = lrk.get_state(b"  SELECT  TEST").unwrap();
        let (cont_indices, _) = lrk.get_valid_continuations_with_state(&state);
        println!(
            "matching {}, {} conts: {:#?}",
            lrk.is_match_state(&state),
            cont_indices.len(),
            cont_indices
                .iter()
                .map(|i| String::from_utf8_lossy(&conts[*i]))
                .collect_vec()
        );
    }
}