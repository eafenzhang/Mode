//! Restricted CEL 的编译期可接受性检查（不求值）。
//!
//! 真源：`packages/model-option-map/src/tokenizer.ts` 与 `parser.ts`。
//! 只复刻 `compileModelOptionMap` 的失败条件：tokenize、parse、顶层（及条件分支）
//! 必须返回 JSON 对象。

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TokenKind {
    Identifier,
    String,
    Number,
    Operator,
    Punctuation,
    Eof,
}

#[derive(Debug, Clone)]
struct Token {
    kind: TokenKind,
    value: String,
    offset: usize,
}

const DOUBLE_OPERATORS: [&str; 6] = ["&&", "||", "==", "!=", "<=", ">="];
const SINGLE_OPERATORS: [char; 8] = ['+', '-', '*', '/', '%', '!', '<', '>'];
const PUNCTUATION: [char; 10] = ['{', '}', '[', ']', '(', ')', ',', ':', '?', '.'];

pub fn validate_option_map(source: &str, variable_name: &str) -> Result<(), String> {
    let trimmed = source.trim();
    if trimmed.is_empty() {
        return Err("expression must not be empty at offset 0".to_owned());
    }
    let tokens = tokenize(trimmed)?;
    let mut parser = Parser {
        tokens: &tokens,
        index: 0,
        variable_name,
    };
    let expression = parser.parse()?;
    if returns_object(&expression) {
        Ok(())
    } else {
        Err("model option map must return a JSON object".to_owned())
    }
}

#[derive(Debug, Clone)]
enum Expr {
    Literal,
    Input,
    Array,
    Object,
    Unary,
    Binary,
    Conditional(Box<Expr>, Box<Expr>),
}

fn returns_object(expression: &Expr) -> bool {
    match expression {
        Expr::Object => true,
        Expr::Conditional(when_true, when_false) => {
            returns_object(when_true) && returns_object(when_false)
        }
        _ => false,
    }
}

fn tokenize(source: &str) -> Result<Vec<Token>, String> {
    let chars = source.chars().collect::<Vec<_>>();
    let mut tokens = Vec::new();
    let mut offset = 0usize;

    while offset < chars.len() {
        let character = chars[offset];
        if character.is_whitespace() {
            offset += 1;
            continue;
        }

        if character == '\'' || character == '"' {
            let (token, next) = read_string(&chars, offset, character)?;
            tokens.push(token);
            offset = next;
            continue;
        }

        if character.is_ascii_digit() {
            let (token, next) = read_number(&chars, offset);
            tokens.push(token);
            offset = next;
            continue;
        }

        if character.is_ascii_alphabetic() || character == '_' {
            let mut end = offset + 1;
            while end < chars.len()
                && (chars[end].is_ascii_alphanumeric() || chars[end] == '_')
            {
                end += 1;
            }
            tokens.push(Token {
                kind: TokenKind::Identifier,
                value: chars[offset..end].iter().collect(),
                offset,
            });
            offset = end;
            continue;
        }

        let pair = if offset + 1 < chars.len() {
            format!("{}{}", character, chars[offset + 1])
        } else {
            character.to_string()
        };
        if DOUBLE_OPERATORS.contains(&pair.as_str()) {
            tokens.push(Token {
                kind: TokenKind::Operator,
                value: pair,
                offset,
            });
            offset += 2;
            continue;
        }
        if SINGLE_OPERATORS.contains(&character) {
            tokens.push(Token {
                kind: TokenKind::Operator,
                value: character.to_string(),
                offset,
            });
            offset += 1;
            continue;
        }
        if PUNCTUATION.contains(&character) {
            tokens.push(Token {
                kind: TokenKind::Punctuation,
                value: character.to_string(),
                offset,
            });
            offset += 1;
            continue;
        }

        return Err(format!("unsupported token {character:?} at offset {offset}"));
    }

    tokens.push(Token {
        kind: TokenKind::Eof,
        value: String::new(),
        offset: source.chars().count(),
    });
    Ok(tokens)
}

fn read_number(chars: &[char], start: usize) -> (Token, usize) {
    let mut index = start;
    // (0|[1-9][0-9]*)
    if chars[index] == '0' {
        index += 1;
    } else {
        index += 1;
        while index < chars.len() && chars[index].is_ascii_digit() {
            index += 1;
        }
    }
    // (\.[0-9]+)?
    if chars.get(index) == Some(&'.')
        && chars.get(index + 1).is_some_and(|c| c.is_ascii_digit())
    {
        index += 1;
        while index < chars.len() && chars[index].is_ascii_digit() {
            index += 1;
        }
    }
    // ([eE][+-]?[0-9]+)?
    if matches!(chars.get(index), Some('e') | Some('E')) {
        let mut cursor = index + 1;
        if matches!(chars.get(cursor), Some('+') | Some('-')) {
            cursor += 1;
        }
        if chars.get(cursor).is_some_and(|c| c.is_ascii_digit()) {
            while cursor < chars.len() && chars[cursor].is_ascii_digit() {
                cursor += 1;
            }
            index = cursor;
        }
    }
    (
        Token {
            kind: TokenKind::Number,
            value: chars[start..index].iter().collect(),
            offset: start,
        },
        index,
    )
}

fn read_string(chars: &[char], start: usize, quote: char) -> Result<(Token, usize), String> {
    let mut cursor = start + 1;
    let mut value = String::new();
    while cursor < chars.len() {
        let character = chars[cursor];
        if character == quote {
            return Ok((
                Token {
                    kind: TokenKind::String,
                    value,
                    offset: start,
                },
                cursor + 1,
            ));
        }
        if character == '\n' || character == '\r' {
            return Err(format!("unterminated string literal at offset {start}"));
        }
        if character != '\\' {
            value.push(character);
            cursor += 1;
            continue;
        }

        let escape_offset = cursor;
        cursor += 1;
        let Some(escaped) = chars.get(cursor).copied() else {
            return Err(format!("unterminated string escape at offset {escape_offset}"));
        };
        let simple = match escaped {
            '\'' => Some('\''),
            '"' => Some('"'),
            '\\' => Some('\\'),
            'b' => Some('\u{0008}'),
            'f' => Some('\u{000c}'),
            'n' => Some('\n'),
            'r' => Some('\r'),
            't' => Some('\t'),
            _ => None,
        };
        if let Some(character) = simple {
            value.push(character);
            cursor += 1;
            continue;
        }
        if escaped == 'u' {
            if cursor + 4 >= chars.len() {
                return Err(format!("invalid unicode escape at offset {escape_offset}"));
            }
            let digits = chars[cursor + 1..cursor + 5].iter().collect::<String>();
            if !digits.chars().all(|c| c.is_ascii_hexdigit()) {
                return Err(format!("invalid unicode escape at offset {escape_offset}"));
            }
            let code = u32::from_str_radix(&digits, 16).map_err(|_| {
                format!("invalid unicode escape at offset {escape_offset}")
            })?;
            value.push(char::from_u32(code).unwrap_or('\u{fffd}'));
            cursor += 5;
            continue;
        }
        return Err(format!(
            "unsupported string escape \\{escaped} at offset {escape_offset}"
        ));
    }
    Err(format!("unterminated string literal at offset {start}"))
}

struct Parser<'a> {
    tokens: &'a [Token],
    index: usize,
    variable_name: &'a str,
}

impl Parser<'_> {
    fn parse(&mut self) -> Result<Expr, String> {
        let expression = self.parse_conditional()?;
        let trailing = self.current().clone();
        if trailing.kind != TokenKind::Eof {
            if trailing.value == "." {
                return Err(format!(
                    "member access is not supported at offset {}",
                    trailing.offset
                ));
            }
            if trailing.value == "(" {
                return Err(format!(
                    "function calls are not supported at offset {}",
                    trailing.offset
                ));
            }
            return Err(format!(
                "unexpected token {:?} at offset {}",
                trailing.value, trailing.offset
            ));
        }
        Ok(expression)
    }

    fn parse_conditional(&mut self) -> Result<Expr, String> {
        let condition = self.parse_logical_or()?;
        if !self.consume("?") {
            return Ok(condition);
        }
        let when_true = self.parse_conditional()?;
        self.expect(":")?;
        let when_false = self.parse_conditional()?;
        Ok(Expr::Conditional(
            Box::new(when_true),
            Box::new(when_false),
        ))
    }

    fn parse_logical_or(&mut self) -> Result<Expr, String> {
        self.parse_binary(Self::parse_logical_and, &["||"])
    }

    fn parse_logical_and(&mut self) -> Result<Expr, String> {
        self.parse_binary(Self::parse_equality, &["&&"])
    }

    fn parse_equality(&mut self) -> Result<Expr, String> {
        self.parse_binary(Self::parse_relational, &["==", "!="])
    }

    fn parse_relational(&mut self) -> Result<Expr, String> {
        self.parse_binary(Self::parse_additive, &["<", "<=", ">", ">="])
    }

    fn parse_additive(&mut self) -> Result<Expr, String> {
        self.parse_binary(Self::parse_multiplicative, &["+", "-"])
    }

    fn parse_multiplicative(&mut self) -> Result<Expr, String> {
        self.parse_binary(Self::parse_unary, &["*", "/", "%"])
    }

    fn parse_binary(
        &mut self,
        operand: fn(&mut Self) -> Result<Expr, String>,
        operators: &[&str],
    ) -> Result<Expr, String> {
        let mut expression = operand(self)?;
        while self.current().kind == TokenKind::Operator
            && operators.contains(&self.current().value.as_str())
        {
            self.advance();
            operand(self)?;
            expression = Expr::Binary;
        }
        Ok(expression)
    }

    fn parse_unary(&mut self) -> Result<Expr, String> {
        let token = self.current().clone();
        if token.kind == TokenKind::Operator && matches!(token.value.as_str(), "!" | "-" | "+") {
            self.advance();
            self.parse_unary()?;
            return Ok(Expr::Unary);
        }
        self.parse_primary()
    }

    fn parse_primary(&mut self) -> Result<Expr, String> {
        let token = self.advance().clone();
        match token.kind {
            TokenKind::Number => {
                let value = token
                    .value
                    .parse::<f64>()
                    .map_err(|_| format!("invalid number literal at offset {}", token.offset))?;
                if !value.is_finite()
                    || (value.fract() == 0.0 && value.abs() > 9_007_199_254_740_991.0)
                {
                    return Err(format!(
                        "number literal is not JSON-safe at offset {}",
                        token.offset
                    ));
                }
                Ok(Expr::Literal)
            }
            TokenKind::String => Ok(Expr::Literal),
            TokenKind::Identifier => {
                if self.current().value == "(" {
                    return Err(format!(
                        "function calls are not supported at offset {}",
                        self.current().offset
                    ));
                }
                if token.value == self.variable_name {
                    return Ok(Expr::Input);
                }
                if matches!(token.value.as_str(), "true" | "false" | "null") {
                    return Ok(Expr::Literal);
                }
                Err(format!(
                    "unknown identifier {:?} at offset {}",
                    token.value, token.offset
                ))
            }
            _ if token.value == "(" => {
                let expression = self.parse_conditional()?;
                self.expect(")")?;
                Ok(expression)
            }
            _ if token.value == "[" => self.parse_array(token.offset),
            _ if token.value == "{" => self.parse_object(token.offset),
            _ => Err(format!(
                "unexpected token {:?} at offset {}",
                token.value, token.offset
            )),
        }
    }

    fn parse_array(&mut self, _offset: usize) -> Result<Expr, String> {
        if !self.consume("]") {
            loop {
                self.parse_conditional()?;
                if !self.consume(",") {
                    break;
                }
            }
            self.expect("]")?;
        }
        Ok(Expr::Array)
    }

    fn parse_object(&mut self, _offset: usize) -> Result<Expr, String> {
        let mut keys = std::collections::HashSet::new();
        if !self.consume("}") {
            loop {
                let key = self.advance().clone();
                if key.kind != TokenKind::String {
                    return Err(format!(
                        "object keys must be string literals at offset {}",
                        key.offset
                    ));
                }
                if !keys.insert(key.value.clone()) {
                    return Err(format!(
                        "duplicate object key {:?} at offset {}",
                        key.value, key.offset
                    ));
                }
                self.expect(":")?;
                self.parse_conditional()?;
                if !self.consume(",") {
                    break;
                }
            }
            self.expect("}")?;
        }
        Ok(Expr::Object)
    }

    fn consume(&mut self, value: &str) -> bool {
        if self.current().value != value {
            return false;
        }
        self.index += 1;
        true
    }

    fn expect(&mut self, value: &str) -> Result<(), String> {
        let token = self.current().clone();
        if token.value != value {
            return Err(format!(
                "expected {:?} at offset {}",
                value, token.offset
            ));
        }
        self.index += 1;
        Ok(())
    }

    fn advance(&mut self) -> Token {
        let token = self.current().clone();
        if token.kind != TokenKind::Eof {
            self.index += 1;
        }
        token
    }

    fn current(&self) -> &Token {
        self.tokens
            .get(self.index)
            .unwrap_or_else(|| self.tokens.last().expect("至少包含 eof"))
    }
}
