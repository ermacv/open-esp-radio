use super::*;

/// The top-level shapes of `source`, as the compiler's tokens give them.
fn shapes(source: &str) -> Vec<Shape> {
    let mut shapes = Vec::new();
    let mut characters = source.chars().peekable();
    while let Some(character) = characters.next() {
        match character {
            ' ' | '\n' => {}
            '(' | '[' | '{' => {
                let (open, close, delim) = match character {
                    '(' => ('(', ')', Delim::Parenthesis),
                    '[' => ('[', ']', Delim::Bracket),
                    _ => ('{', '}', Delim::Brace),
                };
                let mut depth = 1;
                for inner in characters.by_ref() {
                    if inner == open {
                        depth += 1;
                    } else if inner == close {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                }
                shapes.push(Shape::Group(delim));
            }
            '"' => {
                let mut text = String::from('"');
                for inner in characters.by_ref() {
                    text.push(inner);
                    if inner == '"' {
                        break;
                    }
                }
                shapes.push(Shape::Literal(text));
            }
            '/' if characters.peek() == Some(&'/') => {
                // A doc comment reaches the attribute as `#[doc = "…"]`.
                for inner in characters.by_ref() {
                    if inner == '\n' {
                        break;
                    }
                }
                shapes.push(Shape::Punct('#'));
                shapes.push(Shape::Group(Delim::Bracket));
            }
            c if c.is_ascii_digit() => {
                let mut text = String::from(c);
                while let Some(&next) = characters
                    .peek()
                    .filter(|next| next.is_ascii_alphanumeric())
                {
                    text.push(next);
                    characters.next();
                }
                shapes.push(Shape::Literal(text));
            }
            c if c.is_alphabetic() || c == '_' => {
                let mut text = String::from(c);
                while let Some(&next) = characters
                    .peek()
                    .filter(|next| next.is_alphanumeric() || **next == '_' || **next == '#')
                {
                    text.push(next);
                    characters.next();
                }
                shapes.push(Shape::Ident(text));
            }
            c => shapes.push(Shape::Punct(c)),
        }
    }
    shapes
}

fn written(level: &str, item: &str) -> Result<String, Refusal> {
    plan(&shapes(level), &shapes(item)).map(|placement| source(&placement.attributes()))
}

#[test]
fn a_function_takes_its_level_and_a_section_of_its_own() {
    assert_eq!(
        written(
            "1",
            "/// One step.\n#[allow(dead_code)] pub(crate) fn step(x: u8) -> u8 { x }"
        ),
        Ok("# [cfg_attr (target_os = \"none\" , inline (never))] \
            # [cfg_attr (target_os = \"none\" , unsafe (link_section = \
            concat ! (\".hot.1.text.\" , module_path ! () , \"::step\")))]"
            .into())
    );
}

#[test]
fn a_generic_function_is_placed_by_its_name() {
    assert_eq!(
        written(
            "2",
            "fn poll<T: Copy>(self: Pin<&mut Self>, value: T) -> Poll<T> { value }"
        ),
        Ok("# [cfg_attr (target_os = \"none\" , inline (never))] \
            # [cfg_attr (target_os = \"none\" , unsafe (link_section = \
            concat ! (\".hot.2.text.\" , module_path ! () , \"::poll\")))]"
            .into())
    );
}

#[test]
fn qualifiers_and_raw_names_are_read_past() {
    for item in [
        "pub const unsafe fn checksum() {}",
        "pub async fn checksum() {}",
        "pub(super) extern \"C\" fn checksum() {}",
        "unsafe extern fn checksum() {}",
        "fn r#checksum() {}",
    ] {
        assert_eq!(
            plan(&shapes("3"), &shapes(item)),
            Ok(Placement {
                level: 3,
                name: "checksum".into()
            }),
            "{item}"
        );
    }
}

#[test]
fn anything_but_a_function_is_refused() {
    for item in [
        "static TABLE: [u8; 4] = [0; 4];",
        "pub const LIMIT: u8 = 4;",
        "struct Hot;",
        "impl Hot { fn step() {} }",
        "mod hot { fn step() {} }",
    ] {
        assert_eq!(written("1", item), Err(Refusal::NotAFunction), "{item}");
    }
    assert_eq!(
        Refusal::NotAFunction.message(),
        "#[hot] takes a function only"
    );
}

#[test]
fn a_level_outside_one_to_the_greatest_is_refused() {
    assert_eq!(written("", "fn step() {}"), Err(Refusal::NoLevel));
    for level in ["0", "4", "256", "1u8", "-1", "LEVEL", "1, 2"] {
        assert!(
            matches!(written(level, "fn step() {}"), Err(Refusal::Level(_))),
            "{level}"
        );
    }
    assert_eq!(
        written("LEVEL", "fn step() {}"),
        Err(Refusal::Level("LEVEL".into()))
    );
    assert_eq!(
        written("1, 2", "fn step() {}"),
        Err(Refusal::Level("1 , 2".into()))
    );
    assert_eq!(
        written("(1)", "fn step() {}"),
        Err(Refusal::Level("(…)".into()))
    );
    assert_eq!(GREATEST_LEVEL, 3);
    assert_eq!(
        Refusal::Level("0".into()).message(),
        "#[hot] takes a level from 1 to 3 as a decimal literal, not `0`"
    );
    assert_eq!(
        Refusal::NoLevel.message(),
        "#[hot] needs a level from 1 to 3: #[hot(1)]"
    );
}

#[test]
fn a_refusal_is_a_compile_error_naming_the_attribute() {
    assert_eq!(
        source(&Refusal::NotAFunction.tokens()),
        ": : core : : compile_error ! (\"#[hot] takes a function only\") ;"
    );
}
