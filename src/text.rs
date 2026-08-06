//! Tokenisation and sentence splitting.
//!
//! A token is a run of non-whitespace, split into three parts: leading
//! punctuation, the token proper, and trailing punctuation. Keeping the parts
//! separate matters downstream: trailing punctuation drives phrasing, while
//! the token proper is what gets normalised and looked up.
//!
//! Sentence splitting is heuristic and intentionally conservative: a period
//! only ends a sentence when the following token looks like a new sentence and
//! the preceding token does not look like an abbreviation.

/// One token of input text.
#[derive(Clone, Debug, Default)]
pub struct Token {
    /// Whitespace that preceded this token. Blank lines are a sentence break,
    /// so this is kept rather than discarded.
    pub whitespace: String,
    pub prepunctuation: String,
    pub name: String,
    pub punctuation: String,
}

const WHITESPACE: &str = " \t\n\r";
const PREPUNCTUATION: &str = "\"'`({[";
const POSTPUNCTUATION: &str = "\"'`.,:;!?(){}[]";

/// Split text into sentences of tokens.
///
/// Always returns at least the tokens it found; empty input yields no
/// sentences.
pub fn tokenize(text: &str) -> Vec<Vec<Token>> {
    sentences(text).collect()
}

/// The same split, one sentence at a time.
///
/// Preferred when the caller synthesises as it goes: the whole input is still
/// in memory, but the tokens for it are not.
pub fn sentences(text: &str) -> Sentences<'_> {
    Sentences {
        stream: TokenStream::new(text),
        pending: None,
    }
}

/// Iterator over the sentences of a text. See [`sentences`].
pub struct Sentences<'a> {
    stream: TokenStream<'a>,
    /// The token that ended the previous sentence by starting this one. The
    /// split is decided between two tokens, so one always arrives early.
    pending: Option<Token>,
}

impl Iterator for Sentences<'_> {
    type Item = Vec<Token>;

    fn next(&mut self) -> Option<Vec<Token>> {
        let mut current: Vec<Token> = self.pending.take().into_iter().collect();
        for token in self.stream.by_ref() {
            if let Some(last) = current.last() {
                if is_sentence_break(last, &token) {
                    self.pending = Some(token);
                    return Some(current);
                }
            }
            current.push(token);
        }
        (!current.is_empty()).then_some(current)
    }
}

/// Group a stream of tokens into sentences, for callers that produce tokens
/// themselves rather than from one string.
///
/// Holds one sentence plus one token, whatever the length of the input.
#[derive(Default)]
pub struct SentenceBuilder {
    current: Vec<Token>,
}

impl SentenceBuilder {
    /// Add a token, returning the sentence it completed, if any.
    pub fn push(&mut self, token: Token) -> Option<Vec<Token>> {
        let finished = match self.current.last() {
            Some(last) if is_sentence_break(last, &token) => {
                Some(std::mem::take(&mut self.current))
            }
            _ => None,
        };
        self.current.push(token);
        finished
    }

    /// The last sentence, which no following token can close.
    pub fn finish(&mut self) -> Option<Vec<Token>> {
        (!self.current.is_empty()).then(|| std::mem::take(&mut self.current))
    }
}

struct TokenStream<'a> {
    rest: &'a str,
}

impl<'a> TokenStream<'a> {
    fn new(text: &'a str) -> TokenStream<'a> {
        TokenStream { rest: text }
    }

    fn take_while(&mut self, set: &str) -> String {
        let end = self
            .rest
            .find(|c| !set.contains(c))
            .unwrap_or(self.rest.len());
        let (taken, rest) = self.rest.split_at(end);
        self.rest = rest;
        taken.to_string()
    }
}

impl Iterator for TokenStream<'_> {
    type Item = Token;

    fn next(&mut self) -> Option<Token> {
        let whitespace = self.take_while(WHITESPACE);
        if self.rest.is_empty() {
            return None;
        }
        let prepunctuation = self.take_while(PREPUNCTUATION);
        if self.rest.is_empty() {
            // Trailing quotes with nothing after them: emit them as the token
            // rather than losing them.
            return Some(Token {
                whitespace,
                name: prepunctuation,
                ..Token::default()
            });
        }

        let end = self
            .rest
            .find(|c| WHITESPACE.contains(c))
            .unwrap_or(self.rest.len());
        let (mut name, rest) = self.rest.split_at(end);
        self.rest = rest;

        // Peel trailing punctuation, but never the whole token: a bare "?" is
        // itself a token, and phrasing needs it to stay one.
        let mut split = name.len();
        while split > 1 {
            let c = name[..split].chars().next_back().expect("non-empty");
            if !POSTPUNCTUATION.contains(c) {
                break;
            }
            split -= c.len_utf8();
        }
        let punctuation = name[split..].to_string();
        name = &name[..split];

        Some(Token {
            whitespace,
            prepunctuation,
            name: name.to_string(),
            punctuation,
        })
    }
}

/// Tokens read from somewhere in fixed-size chunks.
///
/// A token can only be known to be complete once whitespace follows it, so
/// each chunk is tokenised up to the start of the last run of whitespace and
/// the remainder is carried forward. Carrying the whitespace itself, rather
/// than just the partial token, is what keeps a blank line visible to sentence
/// splitting across a chunk boundary.
///
/// Memory used is the chunk size plus the longest single token, whatever the
/// size of the input.
pub struct ChunkedTokens<R> {
    reader: R,
    /// Text read but not yet known to contain a complete token.
    pending: String,
    /// Trailing bytes of a character split across two reads.
    partial: Vec<u8>,
    ready: std::collections::VecDeque<Token>,
    finished: bool,
}

impl<R: std::io::Read> ChunkedTokens<R> {
    pub fn new(reader: R) -> ChunkedTokens<R> {
        ChunkedTokens {
            reader,
            pending: String::new(),
            partial: Vec::new(),
            ready: std::collections::VecDeque::new(),
            finished: false,
        }
    }

    /// The next token, reading more input if there is not one already.
    pub fn next_token(&mut self) -> std::io::Result<Option<Token>> {
        while self.ready.is_empty() && !self.finished {
            self.fill()?;
        }
        Ok(self.ready.pop_front())
    }

    fn fill(&mut self) -> std::io::Result<()> {
        let mut chunk = [0u8; 8192];
        let read = self.reader.read(&mut chunk)?;
        if read == 0 {
            // At the end of input the last token is complete by definition.
            self.finished = true;
            self.ready
                .extend(TokenStream::new(&std::mem::take(&mut self.pending)));
            return Ok(());
        }
        self.decode(&chunk[..read]);

        if let Some(cut) = last_whitespace_run(&self.pending) {
            let rest = self.pending.split_off(cut);
            self.ready.extend(TokenStream::new(&self.pending));
            self.pending = rest;
        }
        Ok(())
    }

    /// Append a chunk's text, holding back a character split across the
    /// boundary. Bytes that are not valid UTF-8 at all are dropped rather than
    /// stalling the read or inventing a character to pronounce.
    fn decode(&mut self, chunk: &[u8]) {
        self.partial.extend_from_slice(chunk);
        let (good, drop) = match std::str::from_utf8(&self.partial) {
            Ok(text) => (text.len(), 0),
            Err(e) => (e.valid_up_to(), e.error_len().unwrap_or(0)),
        };
        self.pending
            .push_str(std::str::from_utf8(&self.partial[..good]).expect("validated above"));
        self.partial.drain(..good + drop);
    }
}

/// Where the last run of whitespace in `text` begins, which is where a token
/// that may still be incomplete starts.
fn last_whitespace_run(text: &str) -> Option<usize> {
    // Every whitespace character here is ASCII, so byte positions are always
    // character boundaries.
    let bytes = text.as_bytes();
    let mut start = bytes
        .iter()
        .rposition(|b| WHITESPACE.contains(*b as char))?;
    while start > 0 && WHITESPACE.contains(bytes[start - 1] as char) {
        start -= 1;
    }
    Some(start)
}

/// Whether a sentence ends between `last` and `next`.
fn is_sentence_break(last: &Token, next: &Token) -> bool {
    // A blank line always breaks.
    if next.whitespace.matches('\n').count() > 1 {
        return true;
    }
    // Kept for compatibility with the reference implementation, which declines
    // to break after this one interjection.
    let lower = last.name.to_ascii_lowercase();
    if lower == "yahoo"
        && last.punctuation.contains('!')
        && next.name.starts_with(|c: char| c.is_ascii_lowercase())
    {
        return false;
    }
    if last.punctuation.contains([':', '?', '!']) {
        return true;
    }
    if !last.punctuation.contains('.') {
        return false;
    }
    if !next.name.starts_with(|c: char| c.is_ascii_uppercase()) {
        return false;
    }
    // Extra whitespace after a period is a strong signal on its own.
    if next.whitespace.len() > 1 {
        return true;
    }
    // Otherwise, only break when the preceding token does not look like an
    // abbreviation: "Dr. Smith" is one sentence, "ended. Then" is two.
    let looks_like_abbreviation = last
        .name
        .chars()
        .next_back()
        .is_some_and(|c| c.is_ascii_uppercase())
        || (last.name.len() < 4 && last.name.starts_with(|c: char| c.is_ascii_uppercase()));
    !looks_like_abbreviation
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn punctuation_is_separated_from_the_token() {
        let sentences = tokenize("Hello, world!");
        assert_eq!(sentences.len(), 1);
        let t = &sentences[0];
        assert_eq!(t[0].name, "Hello");
        assert_eq!(t[0].punctuation, ",");
        assert_eq!(t[1].name, "world");
        assert_eq!(t[1].punctuation, "!");
    }

    #[test]
    fn quotes_are_split_off_the_front() {
        let t = &tokenize("\"quoted\"")[0];
        assert_eq!(t[0].prepunctuation, "\"");
        assert_eq!(t[0].name, "quoted");
        assert_eq!(t[0].punctuation, "\"");
    }

    #[test]
    fn a_lone_question_mark_stays_a_token() {
        let t = &tokenize("really ?")[0];
        assert_eq!(t[1].name, "?");
        assert_eq!(t[1].punctuation, "");
    }

    #[test]
    fn sentences_split_on_terminal_punctuation() {
        let s = tokenize("One thing. Then another. And more!");
        assert_eq!(s.len(), 3);
    }

    #[test]
    fn abbreviations_do_not_split_sentences() {
        let s = tokenize("Dr. Smith arrived.");
        assert_eq!(s.len(), 1);
    }

    #[test]
    fn blank_lines_split_sentences() {
        let s = tokenize("one\n\ntwo");
        assert_eq!(s.len(), 2);
    }
}
