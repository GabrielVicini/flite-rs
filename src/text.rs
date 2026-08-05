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
    let mut sentences: Vec<Vec<Token>> = Vec::new();
    let mut current: Vec<Token> = Vec::new();

    for token in TokenStream::new(text) {
        if let Some(last) = current.last() {
            if is_sentence_break(last, &token) {
                sentences.push(std::mem::take(&mut current));
            }
        }
        current.push(token);
    }
    if !current.is_empty() {
        sentences.push(current);
    }
    sentences
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
