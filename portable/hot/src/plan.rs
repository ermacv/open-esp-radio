//! What `#[hot(N)]` writes, decided from the shapes of its tokens.

/// The greatest level. The image pipeline takes the same
/// (`oer_tools_image_checks::linker::hot::GREATEST_LEVEL`; its test applies
/// this attribute at that level).
pub const GREATEST_LEVEL: u8 = 3;

/// A group's delimiter.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Delim {
    Parenthesis,
    Brace,
    Bracket,
    None,
}

/// The shape of one top-level token tree; a group's content is not read.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Shape {
    Ident(String),
    Punct(char),
    Literal(String),
    Group(Delim),
}

/// Whether a punctuation character joins the next one (`::`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Joint {
    Joint,
    Alone,
}

/// A token the attribute writes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Token {
    Ident(&'static str),
    Punct(char, Joint),
    /// A string literal.
    Str(String),
    Group(Delim, Vec<Token>),
}

impl core::fmt::Display for Token {
    /// The tokens as source, one space between two.
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Ident(text) => formatter.write_str(text),
            Self::Punct(character, _) => formatter.write_str(character.encode_utf8(&mut [0; 4])),
            Self::Str(text) => formatter.write_fmt(format_args!("{text:?}")),
            Self::Group(delim, inner) => {
                let (open, close) = match delim {
                    Delim::Parenthesis => ("(", ")"),
                    Delim::Brace => ("{", "}"),
                    Delim::Bracket => ("[", "]"),
                    Delim::None => ("", ""),
                };
                formatter.write_str(open)?;
                formatter.write_str(&source(inner))?;
                formatter.write_str(close)
            }
        }
    }
}

/// `tokens` as source, one space between two.
pub fn source(tokens: &[Token]) -> String {
    tokens
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(" ")
}

fn punct(character: char) -> Token {
    Token::Punct(character, Joint::Alone)
}

/// `#[cfg_attr(target_os = "none", <attribute>)]`
fn bare_metal(attribute: Vec<Token>) -> Vec<Token> {
    let mut inner = vec![
        Token::Ident("target_os"),
        punct('='),
        Token::Str("none".into()),
        punct(','),
    ];
    inner.extend(attribute);
    vec![
        punct('#'),
        Token::Group(
            Delim::Bracket,
            vec![
                Token::Ident("cfg_attr"),
                Token::Group(Delim::Parenthesis, inner),
            ],
        ),
    ]
}

/// `<name>!(<arguments>)`
fn macro_call(name: &'static str, arguments: Vec<Token>) -> Vec<Token> {
    vec![
        Token::Ident(name),
        punct('!'),
        Token::Group(Delim::Parenthesis, arguments),
    ]
}

/// A function's placement: its level and its name.
#[derive(Debug, Eq, PartialEq)]
pub struct Placement {
    level: u8,
    name: String,
}

/// Why the attribute refuses its input.
#[derive(Debug, Eq, PartialEq)]
pub enum Refusal {
    /// The attribute names no level.
    NoLevel,
    /// The level is not one decimal literal from 1 to the greatest.
    Level(String),
    /// The item is not a function.
    NotAFunction,
}

impl Refusal {
    /// `::core::compile_error!("<message>");`
    pub fn tokens(&self) -> Vec<Token> {
        let mut written = vec![
            Token::Punct(':', Joint::Joint),
            punct(':'),
            Token::Ident("core"),
            Token::Punct(':', Joint::Joint),
            punct(':'),
        ];
        written.extend(macro_call(
            "compile_error",
            vec![Token::Str(self.message())],
        ));
        written.push(punct(';'));
        written
    }

    pub fn message(&self) -> String {
        match self {
            Self::NoLevel => format!("#[hot] needs a level from 1 to {GREATEST_LEVEL}: #[hot(1)]"),
            Self::Level(given) => format!(
                "#[hot] takes a level from 1 to {GREATEST_LEVEL} as a decimal literal, not `{given}`"
            ),
            Self::NotAFunction => "#[hot] takes a function only".into(),
        }
    }
}

impl Placement {
    /// The attributes written before the function, on a bare-metal target
    /// only: a place of its own, and a section of its own named by level,
    /// module path and name.
    pub fn attributes(&self) -> Vec<Token> {
        let mut section = vec![Token::Str(format!(".hot.{}.text.", self.level)), punct(',')];
        section.extend(macro_call("module_path", Vec::new()));
        section.extend([punct(','), Token::Str(format!("::{}", self.name))]);
        let mut link_section = vec![Token::Ident("link_section"), punct('=')];
        link_section.extend(macro_call("concat", section));
        let mut written = bare_metal(vec![
            Token::Ident("inline"),
            Token::Group(Delim::Parenthesis, vec![Token::Ident("never")]),
        ]);
        written.extend(bare_metal(vec![
            Token::Ident("unsafe"),
            Token::Group(Delim::Parenthesis, link_section),
        ]));
        written
    }
}

/// The placement of `item` at the level `attribute` names.
pub fn plan(attribute: &[Shape], item: &[Shape]) -> Result<Placement, Refusal> {
    let level = level(attribute)?;
    let name = function_name(item).ok_or(Refusal::NotAFunction)?;
    Ok(Placement { level, name })
}

fn level(attribute: &[Shape]) -> Result<u8, Refusal> {
    match attribute {
        [] => Err(Refusal::NoLevel),
        [Shape::Literal(text)] => match text.parse::<u8>() {
            Ok(level)
                if (1..=GREATEST_LEVEL).contains(&level)
                    && text.bytes().all(|byte| byte.is_ascii_digit()) =>
            {
                Ok(level)
            }
            _ => Err(Refusal::Level(text.clone())),
        },
        _ => Err(Refusal::Level(
            attribute.iter().map(spelling).collect::<Vec<_>>().join(" "),
        )),
    }
}

fn spelling(shape: &Shape) -> String {
    match shape {
        Shape::Ident(text) | Shape::Literal(text) => text.clone(),
        Shape::Punct(character) => character.to_string(),
        Shape::Group(Delim::Parenthesis) => "(…)".into(),
        Shape::Group(Delim::Brace) => "{…}".into(),
        Shape::Group(Delim::Bracket) => "[…]".into(),
        Shape::Group(Delim::None) => "…".into(),
    }
}

/// The name of the function `item` declares: past its outer attributes, its
/// visibility and its qualifiers, `fn` and an identifier.
fn function_name(item: &[Shape]) -> Option<String> {
    let mut rest = item;
    // Outer attributes, doc comments included: `#` and a bracketed group.
    while let [Shape::Punct('#'), Shape::Group(Delim::Bracket), tail @ ..] = rest {
        rest = tail;
    }
    // Visibility: `pub`, with its restriction if any.
    if let [Shape::Ident(word), tail @ ..] = rest
        && word == "pub"
    {
        rest = match tail {
            [Shape::Group(Delim::Parenthesis), after @ ..] => after,
            _ => tail,
        };
    }
    // Qualifiers, and the ABI string after `extern`.
    loop {
        match rest {
            [Shape::Ident(word), tail @ ..]
                if matches!(
                    word.as_str(),
                    "const" | "async" | "unsafe" | "safe" | "default"
                ) =>
            {
                rest = tail;
            }
            [Shape::Ident(word), Shape::Literal(_), tail @ ..] if word == "extern" => rest = tail,
            [Shape::Ident(word), tail @ ..] if word == "extern" => rest = tail,
            _ => break,
        }
    }
    match rest {
        [Shape::Ident(word), Shape::Ident(name), ..] if word == "fn" => {
            Some(name.strip_prefix("r#").unwrap_or(name).to_owned())
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests;
