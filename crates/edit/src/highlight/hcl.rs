// Copyright (c) iEdit contributors.
// Licensed under the MIT License.

//! HCL/Terraform syntax highlighting tokenizer.
//!
//! Supports HashiCorp Configuration Language files (`.tf`, `.tfvars`, `.hcl`).
//!
//! Recognized constructs:
//! - Block types: `resource`, `variable`, `output`, `data`, `module`, `provider`,
//!   `terraform`, `locals`, `moved`, `import`, `check`, `removed`
//! - Attributes: `key = value`
//! - Strings: `"..."` with `${...}` interpolation
//! - Comments: `#`, `//` (line), `/* ... */` (block)
//! - Numbers, booleans (`true`/`false`), and `null`
//! - Heredocs: `<<EOF ... EOF`, `<<-EOF ... EOF`
//! - Braces, brackets, parentheses
//!
//! State encoding (u8):
//!   0 = normal
//!   1 = inside block comment (`/* ... */`)
//!   2..=255 = inside heredoc (state encodes a tag hash)

use super::{Token, TokenKind};

const STATE_NORMAL: u8 = 0;
const STATE_BLOCK_COMMENT: u8 = 1;
const STATE_HEREDOC: u8 = 2;

const HCL_KEYWORDS: &[&[u8]] = &[
    b"resource",
    b"variable",
    b"output",
    b"data",
    b"module",
    b"provider",
    b"terraform",
    b"locals",
    b"moved",
    b"import",
    b"check",
    b"removed",
];

const HCL_VALUE_KEYWORDS: &[&[u8]] = &[
    b"for", b"in", b"if", b"else", b"endif", b"endfor", b"dynamic", b"content", b"each", b"self",
    b"var", b"local", b"count", b"path",
];

pub fn tokenize_line(line: &[u8], state: u8, tokens: &mut Vec<Token>) -> u8 {
    if line.is_empty() {
        return state;
    }

    // Block comment continuation.
    if state == STATE_BLOCK_COMMENT {
        return tokenize_block_comment(line, 0, tokens);
    }

    // Heredoc continuation.
    if state >= STATE_HEREDOC {
        return tokenize_heredoc_body(line, tokens, state);
    }

    let len = line.len();
    let mut i = 0;

    skip_space(line, &mut i);
    if i >= len {
        return STATE_NORMAL;
    }

    // Line comment: # or //
    if line[i] == b'#' || (i + 1 < len && line[i] == b'/' && line[i + 1] == b'/') {
        tokens.push(Token { offset: i, len: len - i, kind: TokenKind::Comment });
        return STATE_NORMAL;
    }

    // Block comment start.
    if i + 1 < len && line[i] == b'/' && line[i + 1] == b'*' {
        return tokenize_block_comment(line, i, tokens);
    }

    // Try to match a top-level block keyword (e.g., `resource "aws_instance" "web" {`).
    if let Some(new_i) = try_tokenize_block_keyword(line, i, tokens) {
        return tokenize_expression(line, new_i, tokens);
    }

    // General expression parsing (attributes, values, etc.).
    tokenize_expression(line, i, tokens)
}

/// Tokenizes inside a block comment. Returns the state at end of line.
fn tokenize_block_comment(line: &[u8], start: usize, tokens: &mut Vec<Token>) -> u8 {
    let len = line.len();
    let mut i = start;

    while i + 1 < len {
        if line[i] == b'*' && line[i + 1] == b'/' {
            tokens.push(Token { offset: start, len: i + 2 - start, kind: TokenKind::Comment });
            let rest_start = i + 2;
            skip_space(line, &mut (i + 2));
            if i + 2 < len {
                return tokenize_expression(line, rest_start, tokens);
            }
            return STATE_NORMAL;
        }
        i += 1;
    }

    // No closing `*/` found, entire line is comment.
    tokens.push(Token { offset: start, len: len - start, kind: TokenKind::Comment });
    STATE_BLOCK_COMMENT
}

/// Computes a simple hash for the heredoc tag to store in state.
fn heredoc_tag_hash(tag: &[u8]) -> u8 {
    let mut h: u8 = 0;
    for &b in tag {
        h = h.wrapping_mul(31).wrapping_add(b);
    }
    // Ensure it maps into the heredoc state range (>= STATE_HEREDOC).
    if h < STATE_HEREDOC {
        h = h.wrapping_add(STATE_HEREDOC);
    }
    h
}

/// Tokenizes a heredoc body line.
fn tokenize_heredoc_body(line: &[u8], tokens: &mut Vec<Token>, state: u8) -> u8 {
    let len = line.len();

    // Check if this line is the closing delimiter.
    let trimmed_start = line.iter().position(|&b| b != b' ' && b != b'\t').unwrap_or(len);
    let trimmed = &line[trimmed_start..];
    // Trim trailing whitespace for comparison.
    let trimmed_end = trimmed.iter().rposition(|&b| b != b' ' && b != b'\t').map_or(0, |p| p + 1);
    let tag_candidate = &trimmed[..trimmed_end];

    if !tag_candidate.is_empty()
        && tag_candidate.iter().all(|&b| b.is_ascii_alphanumeric() || b == b'_')
        && heredoc_tag_hash(tag_candidate) == state
    {
        tokens.push(Token { offset: 0, len, kind: TokenKind::Punctuation });
        return STATE_NORMAL;
    }

    // Still inside heredoc, whole line is a string.
    tokens.push(Token { offset: 0, len, kind: TokenKind::String });
    state
}

/// Tries to match a top-level HCL block keyword at position `i`.
/// Returns `Some(new_pos)` after the keyword if matched, `None` otherwise.
fn try_tokenize_block_keyword(line: &[u8], i: usize, tokens: &mut Vec<Token>) -> Option<usize> {
    let len = line.len();

    for &kw in HCL_KEYWORDS {
        let end = i + kw.len();
        if end <= len && &line[i..end] == kw && (end >= len || !is_ident_char(line[end])) {
            tokens.push(Token { offset: i, len: kw.len(), kind: TokenKind::Keyword });
            return Some(end);
        }
    }

    None
}

/// Tokenizes an HCL expression (right-hand side of attributes, block bodies, etc.).
fn tokenize_expression(line: &[u8], mut i: usize, tokens: &mut Vec<Token>) -> u8 {
    let len = line.len();

    while i < len {
        skip_space(line, &mut i);
        if i >= len {
            break;
        }

        match line[i] {
            // Line comment.
            b'#' => {
                tokens.push(Token { offset: i, len: len - i, kind: TokenKind::Comment });
                return STATE_NORMAL;
            }
            b'/' if i + 1 < len && line[i + 1] == b'/' => {
                tokens.push(Token { offset: i, len: len - i, kind: TokenKind::Comment });
                return STATE_NORMAL;
            }
            // Block comment start.
            b'/' if i + 1 < len && line[i + 1] == b'*' => {
                return tokenize_block_comment(line, i, tokens);
            }
            // String.
            b'"' => {
                i = tokenize_string(line, i, tokens);
            }
            // Heredoc.
            b'<' if i + 1 < len && line[i + 1] == b'<' => {
                return tokenize_heredoc_start(line, i, tokens);
            }
            // Braces, brackets, parens.
            b'{' | b'}' | b'[' | b']' | b'(' | b')' | b',' | b':' => {
                tokens.push(Token { offset: i, len: 1, kind: TokenKind::Punctuation });
                i += 1;
            }
            // Operators and assignment.
            b'=' => {
                let op_len = if i + 1 < len && line[i + 1] == b'>' { 2 } else { 1 };
                tokens.push(Token { offset: i, len: op_len, kind: TokenKind::Punctuation });
                i += op_len;
            }
            b'!' if i + 1 < len && line[i + 1] == b'=' => {
                tokens.push(Token { offset: i, len: 2, kind: TokenKind::Punctuation });
                i += 2;
            }
            b'<' | b'>' => {
                let op_len = if i + 1 < len && line[i + 1] == b'=' { 2 } else { 1 };
                tokens.push(Token { offset: i, len: op_len, kind: TokenKind::Punctuation });
                i += op_len;
            }
            b'?' | b'!' | b'.' | b'*' => {
                tokens.push(Token { offset: i, len: 1, kind: TokenKind::Punctuation });
                i += 1;
            }
            b'&' if i + 1 < len && line[i + 1] == b'&' => {
                tokens.push(Token { offset: i, len: 2, kind: TokenKind::Punctuation });
                i += 2;
            }
            b'|' if i + 1 < len && line[i + 1] == b'|' => {
                tokens.push(Token { offset: i, len: 2, kind: TokenKind::Punctuation });
                i += 2;
            }
            b'%' if i + 1 < len && line[i + 1] == b'{' => {
                tokens.push(Token { offset: i, len: 2, kind: TokenKind::Punctuation });
                i += 2;
            }
            // Identifier or keyword/value.
            _ if is_ident_start(line[i]) => {
                let start = i;
                while i < len && is_ident_char(line[i]) {
                    i += 1;
                }
                let word = &line[start..i];
                let kind = classify_word(word);
                tokens.push(Token { offset: start, len: i - start, kind });
            }
            // Number.
            _ if line[i].is_ascii_digit() => {
                let start = i;
                i = scan_number(line, i);
                tokens.push(Token { offset: start, len: i - start, kind: TokenKind::Number });
            }
            // Anything else, skip.
            _ => {
                i += 1;
            }
        }
    }

    STATE_NORMAL
}

/// Tokenizes a double-quoted string starting at `i`. Returns the new position.
fn tokenize_string(line: &[u8], start: usize, tokens: &mut Vec<Token>) -> usize {
    let len = line.len();
    let mut i = start + 1; // skip opening quote

    while i < len {
        match line[i] {
            b'\\' => {
                i += 2; // skip escape sequence
            }
            b'"' => {
                i += 1; // closing quote
                tokens.push(Token { offset: start, len: i - start, kind: TokenKind::String });
                return i;
            }
            _ => {
                i += 1;
            }
        }
    }

    // Unclosed string (rest of line).
    tokens.push(Token { offset: start, len: len - start, kind: TokenKind::String });
    len
}

/// Tokenizes a heredoc start (`<<EOF` or `<<-EOF`). Returns state.
fn tokenize_heredoc_start(line: &[u8], start: usize, tokens: &mut Vec<Token>) -> u8 {
    let len = line.len();
    let mut i = start + 2; // skip `<<`

    // Optional `-` for indented heredoc.
    if i < len && line[i] == b'-' {
        i += 1;
    }

    // The tag name.
    let tag_start = i;
    while i < len && is_ident_char(line[i]) {
        i += 1;
    }

    if i > tag_start {
        let tag = &line[tag_start..i];
        let hash = heredoc_tag_hash(tag);
        tokens.push(Token { offset: start, len: i - start, kind: TokenKind::Punctuation });
        return hash;
    }

    // Not a valid heredoc, treat `<<` as punctuation.
    tokens.push(Token { offset: start, len: 2, kind: TokenKind::Punctuation });
    tokenize_expression(line, start + 2, tokens)
}

/// Classifies an identifier word.
fn classify_word(word: &[u8]) -> TokenKind {
    if word == b"true" || word == b"false" {
        return TokenKind::Boolean;
    }

    if word == b"null" {
        return TokenKind::Null;
    }

    for &kw in HCL_KEYWORDS {
        if word == kw {
            return TokenKind::Keyword;
        }
    }

    for &kw in HCL_VALUE_KEYWORDS {
        if word == kw {
            return TokenKind::Keyword;
        }
    }

    TokenKind::Default
}

/// Scans a number literal (integer, float, hex). Returns new position.
fn scan_number(line: &[u8], mut i: usize) -> usize {
    let len = line.len();

    // Hex: 0x...
    if i + 1 < len && line[i] == b'0' && (line[i + 1] == b'x' || line[i + 1] == b'X') {
        i += 2;
        while i < len && line[i].is_ascii_hexdigit() {
            i += 1;
        }
        return i;
    }

    // Decimal digits.
    while i < len && line[i].is_ascii_digit() {
        i += 1;
    }

    // Fractional part.
    if i < len && line[i] == b'.' && i + 1 < len && line[i + 1].is_ascii_digit() {
        i += 1;
        while i < len && line[i].is_ascii_digit() {
            i += 1;
        }
    }

    // Exponent.
    if i < len && (line[i] == b'e' || line[i] == b'E') {
        i += 1;
        if i < len && (line[i] == b'+' || line[i] == b'-') {
            i += 1;
        }
        while i < len && line[i].is_ascii_digit() {
            i += 1;
        }
    }

    i
}

fn is_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_'
}

fn is_ident_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'-'
}

fn skip_space(line: &[u8], i: &mut usize) {
    while *i < line.len() && (line[*i] == b' ' || line[*i] == b'\t') {
        *i += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokenize(input: &str) -> (Vec<Token>, u8) {
        let mut tokens = Vec::new();
        let state = tokenize_line(input.as_bytes(), STATE_NORMAL, &mut tokens);
        (tokens, state)
    }

    fn tokenize_with_state(input: &str, state: u8) -> (Vec<Token>, u8) {
        let mut tokens = Vec::new();
        let new_state = tokenize_line(input.as_bytes(), state, &mut tokens);
        (tokens, new_state)
    }

    #[test]
    fn test_line_comment_hash() {
        let (tokens, state) = tokenize("# This is a comment");
        assert_eq!(state, STATE_NORMAL);
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].kind, TokenKind::Comment);
    }

    #[test]
    fn test_line_comment_double_slash() {
        let (tokens, state) = tokenize("// Another comment");
        assert_eq!(state, STATE_NORMAL);
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].kind, TokenKind::Comment);
    }

    #[test]
    fn test_block_comment_single_line() {
        let (tokens, state) = tokenize("/* block comment */");
        assert_eq!(state, STATE_NORMAL);
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].kind, TokenKind::Comment);
    }

    #[test]
    fn test_block_comment_multi_line() {
        let (tokens, state) = tokenize("/* start of comment");
        assert_eq!(state, STATE_BLOCK_COMMENT);
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].kind, TokenKind::Comment);

        let (tokens2, state2) = tokenize_with_state("still commenting */", STATE_BLOCK_COMMENT);
        assert_eq!(state2, STATE_NORMAL);
        assert_eq!(tokens2.len(), 1);
        assert_eq!(tokens2[0].kind, TokenKind::Comment);
    }

    #[test]
    fn test_resource_block() {
        let (tokens, _) = tokenize("resource \"aws_instance\" \"web\" {");
        assert!(tokens.len() >= 4);
        assert_eq!(tokens[0].kind, TokenKind::Keyword); // resource
        assert_eq!(tokens[1].kind, TokenKind::String); // "aws_instance"
        assert_eq!(tokens[2].kind, TokenKind::String); // "web"
        assert_eq!(tokens[3].kind, TokenKind::Punctuation); // {
    }

    #[test]
    fn test_variable_block() {
        let (tokens, _) = tokenize("variable \"instance_type\" {");
        assert!(tokens.len() >= 3);
        assert_eq!(tokens[0].kind, TokenKind::Keyword);
        assert_eq!(tokens[1].kind, TokenKind::String);
        assert_eq!(tokens[2].kind, TokenKind::Punctuation);
    }

    #[test]
    fn test_attribute_string() {
        let (tokens, _) = tokenize("  ami = \"ami-12345\"");
        assert!(tokens.len() >= 3);
        assert_eq!(tokens[0].kind, TokenKind::Default); // ami (identifier)
        assert_eq!(tokens[1].kind, TokenKind::Punctuation); // =
        assert_eq!(tokens[2].kind, TokenKind::String); // "ami-12345"
    }

    #[test]
    fn test_attribute_number() {
        let (tokens, _) = tokenize("  count = 3");
        assert!(tokens.len() >= 3);
        assert_eq!(tokens[0].kind, TokenKind::Keyword); // count
        assert_eq!(tokens[1].kind, TokenKind::Punctuation); // =
        assert_eq!(tokens[2].kind, TokenKind::Number); // 3
    }

    #[test]
    fn test_attribute_boolean() {
        let (tokens, _) = tokenize("  enabled = true");
        assert!(tokens.len() >= 3);
        assert_eq!(tokens[0].kind, TokenKind::Default); // enabled
        assert_eq!(tokens[1].kind, TokenKind::Punctuation); // =
        assert_eq!(tokens[2].kind, TokenKind::Boolean); // true
    }

    #[test]
    fn test_attribute_null() {
        let (tokens, _) = tokenize("  value = null");
        assert!(tokens.len() >= 3);
        assert_eq!(tokens[0].kind, TokenKind::Default); // value
        assert_eq!(tokens[1].kind, TokenKind::Punctuation); // =
        assert_eq!(tokens[2].kind, TokenKind::Null); // null
    }

    #[test]
    fn test_for_expression() {
        let (tokens, _) = tokenize("  names = [for s in var.list : upper(s)]");
        assert!(tokens.iter().any(|t| t.kind == TokenKind::Keyword)); // for, in, var
        assert!(tokens.iter().any(|t| t.kind == TokenKind::Punctuation)); // [, ], :
    }

    #[test]
    fn test_heredoc() {
        let (tokens, state) = tokenize("  content = <<EOF");
        assert!(state >= STATE_HEREDOC);
        assert!(tokens.iter().any(|t| t.kind == TokenKind::Punctuation)); // <<EOF

        let (tokens2, state2) = tokenize_with_state("  hello world", state);
        assert_eq!(state2, state); // still in heredoc
        assert_eq!(tokens2[0].kind, TokenKind::String);

        let (tokens3, state3) = tokenize_with_state("EOF", state);
        assert_eq!(state3, STATE_NORMAL); // heredoc closed
        assert_eq!(tokens3[0].kind, TokenKind::Punctuation);
    }

    #[test]
    fn test_indented_heredoc() {
        let (tokens, state) = tokenize("  content = <<-POLICY");
        assert!(state >= STATE_HEREDOC);
        assert!(tokens.iter().any(|t| t.kind == TokenKind::Punctuation));

        let (_, state2) = tokenize_with_state("    some content", state);
        assert_eq!(state2, state);

        let (_, state3) = tokenize_with_state("    POLICY", state);
        assert_eq!(state3, STATE_NORMAL);
    }

    #[test]
    fn test_braces_and_brackets() {
        let (tokens, _) = tokenize("  tags = {");
        assert!(tokens.iter().any(|t| t.kind == TokenKind::Punctuation));

        let (tokens2, _) = tokenize("  ingress = []");
        assert!(tokens2.iter().filter(|t| t.kind == TokenKind::Punctuation).count() >= 3);
    }

    #[test]
    fn test_fat_arrow() {
        let (tokens, _) = tokenize("  name => value");
        assert!(tokens.iter().any(|t| t.kind == TokenKind::Punctuation && t.len == 2));
    }

    #[test]
    fn test_comparison_operators() {
        let (tokens, _) = tokenize("  condition = var.count >= 1");
        assert!(tokens.iter().any(|t| t.kind == TokenKind::Number));
    }

    #[test]
    fn test_inline_comment() {
        let (tokens, _) = tokenize("  ami = \"ami-123\" # The AMI ID");
        assert!(tokens.iter().any(|t| t.kind == TokenKind::String));
        assert!(tokens.iter().any(|t| t.kind == TokenKind::Comment));
    }

    #[test]
    fn test_empty_line() {
        let (tokens, state) = tokenize("");
        assert_eq!(state, STATE_NORMAL);
        assert!(tokens.is_empty());
    }

    #[test]
    fn test_closing_brace() {
        let (tokens, _) = tokenize("}");
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].kind, TokenKind::Punctuation);
    }

    #[test]
    fn test_hex_number() {
        let (tokens, _) = tokenize("  port = 0xFF");
        assert!(tokens.iter().any(|t| t.kind == TokenKind::Number));
    }

    #[test]
    fn test_float_number() {
        let (tokens, _) = tokenize("  ratio = 3.14");
        assert!(tokens.iter().any(|t| t.kind == TokenKind::Number));
    }

    #[test]
    fn test_data_block() {
        let (tokens, _) = tokenize("data \"aws_ami\" \"latest\" {");
        assert_eq!(tokens[0].kind, TokenKind::Keyword);
    }

    #[test]
    fn test_module_block() {
        let (tokens, _) = tokenize("module \"vpc\" {");
        assert_eq!(tokens[0].kind, TokenKind::Keyword);
    }

    #[test]
    fn test_locals_block() {
        let (tokens, _) = tokenize("locals {");
        assert_eq!(tokens[0].kind, TokenKind::Keyword);
    }

    #[test]
    fn test_terraform_block() {
        let (tokens, _) = tokenize("terraform {");
        assert_eq!(tokens[0].kind, TokenKind::Keyword);
    }
}
